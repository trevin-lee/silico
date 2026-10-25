//! The render's meshes (docs/design/m4.6-render.md, *In the core*, the meshes): each body
//! meshed at about half a pixel, on a ladder of powers of two so views of nearly one size
//! share a mesh, and kept in memory, shared by the sessions of one kernel. An entry is keyed
//! by the body's step and the deflection the kernel actually used; the first mesh that met
//! a body's floor records it, so a later level under it maps to the floor's entry before
//! any request is sent, and zooming in past the floor meshes once. A step's floor is kept
//! while any of its meshes is.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

use silico_protocol::{Mesh, MeshAt};

use crate::worker::lock;

/// How much the cache keeps, by its meshes' size, before it drops the least recently used.
pub const MESH_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// The deflection a body is meshed at for a view whose pixel is `2 × half_pixel` mm: the
/// largest power of two no coarser than `half_pixel`, taken exactly from its bits (its
/// exponent, its mantissa cleared), so a mesh is never coarser than half a pixel. None for
/// a half pixel that isn't finite and positive.
pub fn mesh_level(half_pixel: f64) -> Option<f64> {
    if !(half_pixel.is_finite() && half_pixel > 0.0) {
        return None;
    }
    let bits = half_pixel.to_bits();
    let level = if bits >> 52 == 0 {
        // Subnormal: its highest set bit is its power of two.
        1u64 << (63 - bits.leading_zeros())
    } else {
        bits & (0x7ff << 52)
    };
    Some(f64::from_bits(level))
}

/// What a mesh holds in memory, by its lists' lengths and their elements' sizes.
fn mesh_bytes(mesh: &Mesh) -> u64 {
    fn n<T>(list: &[T]) -> u64 {
        let each = u64::try_from(size_of::<T>()).unwrap_or(u64::MAX);
        u64::try_from(list.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(each)
    }
    [
        n(&mesh.positions),
        n(&mesh.normals),
        n(&mesh.triangles),
        n(&mesh.face_triangles),
        n(&mesh.edge_points),
        n(&mesh.edge_point_counts),
    ]
    .into_iter()
    .fold(0u64, u64::saturating_add)
}

/// The meshes kept, by each body's step key `K` and the deflection used, least recently
/// used dropped first past the bound.
pub struct MeshCache<K: Ord + Clone> {
    inner: Mutex<Inner<K>>,
}

struct Inner<K> {
    /// By (step, the deflection used's bits).
    entries: BTreeMap<(K, u64), Entry>,
    /// The same entries by when they were last used, oldest first.
    recency: BTreeMap<u64, (K, u64)>,
    /// Each step's floor, once a mesh kept has met it, while any of its meshes is kept.
    floors: BTreeMap<K, f64>,
    bytes: u64,
    bound: u64,
    tick: u64,
}

struct Entry {
    mesh: Arc<Mesh>,
    bytes: u64,
    tick: u64,
}

impl<K: Ord + Clone> MeshCache<K> {
    /// A cache that keeps up to `bound` bytes of meshes.
    pub fn new(bound: u64) -> Self {
        MeshCache {
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                recency: BTreeMap::new(),
                floors: BTreeMap::new(),
                bytes: 0,
                bound,
                tick: 0,
            }),
        }
    }

    /// The deflection to ask the step's mesh at for `level`: `level`, or the step's floor
    /// if it's known and coarser (the kernel would mesh at it anyway).
    pub fn deflection_for(&self, key: &K, level: f64) -> f64 {
        match lock(&self.inner).floors.get(key) {
            Some(&floor) if level < floor => floor,
            _ => level,
        }
    }

    /// The step's mesh at `deflection`, if it's kept (and now the most recently used).
    pub fn get(&self, key: &K, deflection: f64) -> Option<Arc<Mesh>> {
        let mut inner = lock(&self.inner);
        let id = (key.clone(), deflection.to_bits());
        inner.tick += 1;
        let tick = inner.tick;
        let entry = inner.entries.get_mut(&id)?;
        let old = std::mem::replace(&mut entry.tick, tick);
        let mesh = Arc::clone(&entry.mesh);
        inner.recency.remove(&old);
        inner.recency.insert(tick, id);
        Some(mesh)
    }

    /// Keeps the step's mesh, asked at `asked` and made at `made.deflection` (coarser only
    /// at the floor, which is then recorded), and gives it back shared. The least recently
    /// used are dropped until the cache is within its bound, each step's floor with its last
    /// mesh; a mesh larger than the bound alone is given back but not kept, nor its floor.
    pub fn put(&self, key: &K, asked: f64, made: MeshAt) -> Arc<Mesh> {
        let used = made.deflection;
        let mesh = Arc::new(made.mesh);
        let bytes = mesh_bytes(&mesh);
        let mut inner = lock(&self.inner);
        if bytes > inner.bound {
            return mesh;
        }
        let id = (key.clone(), used.to_bits());
        // Replaced, it's the step's still: its floor stays.
        inner.remove(&id, false);
        while inner.bytes.saturating_add(bytes) > inner.bound {
            let Some((_, oldest)) = inner.recency.pop_first() else {
                break;
            };
            inner.remove(&oldest, true);
        }
        inner.tick += 1;
        let tick = inner.tick;
        inner.recency.insert(tick, id.clone());
        inner.entries.insert(
            id,
            Entry {
                mesh: Arc::clone(&mesh),
                bytes,
                tick,
            },
        );
        inner.bytes = inner.bytes.saturating_add(bytes);
        if used > asked {
            inner.floors.insert(key.clone(), used);
        }
        mesh
    }

    /// How many bytes of meshes are kept.
    pub fn bytes(&self) -> u64 {
        lock(&self.inner).bytes
    }
}

impl<K: Ord + Clone> Inner<K> {
    /// Drops the entry `id`, if it's kept, and if `prune`, its step's floor if it was the
    /// step's last.
    fn remove(&mut self, id: &(K, u64), prune: bool) {
        let Some(old) = self.entries.remove(id) else {
            return;
        };
        self.recency.remove(&old.tick);
        self.bytes -= old.bytes;
        if !prune {
            return;
        }
        let step = &id.0;
        let more = self
            .entries
            .range((step.clone(), 0)..=(step.clone(), u64::MAX))
            .next()
            .is_some();
        if !more {
            self.floors.remove(step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MeshCache, mesh_bytes, mesh_level};
    use silico_protocol::{Mesh, MeshAt};

    /// 2^k, exactly, from its bits (a normal number's exponent, biased by 1023).
    fn pow2(k: i32) -> f64 {
        f64::from_bits(u64::try_from(k + 1023).unwrap() << 52)
    }

    /// A mesh of `triangles` triangles on three positions, its size by hand:
    /// 3 × (24 + 12) + 12t + 4.
    fn mesh(triangles: usize) -> Mesh {
        Mesh {
            positions: vec![[0.0; 3]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]; triangles],
            face_triangles: vec![u32::try_from(triangles).unwrap()],
            edge_points: vec![],
            edge_point_counts: vec![],
            valid: None,
        }
    }

    fn at(deflection: f64, mesh: Mesh) -> MeshAt {
        MeshAt { mesh, deflection }
    }

    #[test]
    fn the_level_is_the_largest_power_of_two_no_coarser_than_half_a_pixel() {
        for k in [-30, -7, -1, 0, 1, 10] {
            let p = pow2(k);
            assert_eq!(mesh_level(p), Some(p), "2^{k}");
            assert_eq!(mesh_level(p.next_up()), Some(p), "an ulp over 2^{k}");
            assert_eq!(
                mesh_level(p.next_down()),
                Some(p / 2.0),
                "an ulp under 2^{k}"
            );
        }
        assert_eq!(mesh_level(0.75), Some(0.5));
        assert_eq!(mesh_level(1e-3), Some(0.000_976_562_5));
        assert_eq!(mesh_level(f64::MAX), Some(pow2(1023)));
        assert_eq!(pow2(-7), 1.0 / 128.0);
        assert_eq!(mesh_level(f64::MIN_POSITIVE), Some(f64::MIN_POSITIVE));
        // Subnormal: its highest bit.
        assert_eq!(mesh_level(3e-320), Some(f64::from_bits(1 << 12)));
        for not in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(mesh_level(not), None, "{not}");
        }
    }

    #[test]
    fn a_mesh_is_kept_by_its_step_and_deflection() {
        let cache = MeshCache::new(1 << 20);
        let kept = cache.put(&1u32, 0.25, at(0.25, mesh(2)));
        assert_eq!(*kept, mesh(2));
        assert_eq!(cache.get(&1, 0.25).as_deref(), Some(&mesh(2)));
        assert!(cache.get(&1, 0.5).is_none());
        assert!(cache.get(&2, 0.25).is_none());
        assert_eq!(cache.bytes(), 3 * 36 + 2 * 12 + 4);
        assert_eq!(cache.bytes(), mesh_bytes(&mesh(2)));
    }

    #[test]
    fn a_mesh_made_at_the_floor_maps_every_finer_level_to_it() {
        // Asked at 2⁻⁷, made at its floor, 0.0173: every level under the floor is asked at
        // the floor and found; coarser ones as they are.
        let cache = MeshCache::new(1 << 20);
        assert_eq!(cache.deflection_for(&1u32, 1.0 / 128.0), 1.0 / 128.0);
        cache.put(&1, 1.0 / 128.0, at(0.0173, mesh(1)));
        for finer in [1.0 / 128.0, 1.0 / 256.0, 1e-9] {
            assert_eq!(cache.deflection_for(&1, finer), 0.0173);
            assert!(cache.get(&1, cache.deflection_for(&1, finer)).is_some());
        }
        assert_eq!(cache.deflection_for(&1, 0.5), 0.5);
        // Another step's floor is its own.
        assert_eq!(cache.deflection_for(&2, 1.0 / 256.0), 1.0 / 256.0);
    }

    #[test]
    fn past_its_bound_the_least_recently_used_mesh_is_dropped() {
        // Room for three 136-byte meshes.
        let size = mesh_bytes(&mesh(1));
        let cache = MeshCache::new(3 * size);
        for key in 1u32..=3 {
            cache.put(&key, 1.0, at(1.0, mesh(1)));
        }
        // Step 1 used again, so step 2 is the least recently used.
        assert!(cache.get(&1, 1.0).is_some());
        cache.put(&4, 1.0, at(1.0, mesh(1)));
        assert!(cache.get(&2, 1.0).is_none(), "the least recently used kept");
        for key in [1, 3, 4] {
            assert!(cache.get(&key, 1.0).is_some(), "{key} dropped");
        }
        assert_eq!(cache.bytes(), 3 * size);
        // The same mesh put again replaces itself.
        cache.put(&4, 1.0, at(1.0, mesh(1)));
        assert_eq!(cache.bytes(), 3 * size);
    }

    #[test]
    fn a_steps_floor_goes_with_its_last_mesh() {
        // Room for one mesh: step 1's floor mesh, then step 2's drops it, and its floor.
        let cache = MeshCache::new(mesh_bytes(&mesh(1)));
        cache.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1.0 / 256.0), 0.0173);
        // Put again at its floor, as asked: still the step's, its floor kept.
        cache.put(&1, 0.0173, at(0.0173, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1.0 / 256.0), 0.0173);
        cache.put(&2, 0.5, at(0.5, mesh(1)));
        assert!(cache.get(&1, 0.0173).is_none());
        assert_eq!(cache.deflection_for(&1, 1.0 / 256.0), 1.0 / 256.0);
        // A step with two meshes keeps its floor until both go.
        let cache = MeshCache::new(2 * mesh_bytes(&mesh(1)));
        cache.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        cache.put(&1, 0.5, at(0.5, mesh(1)));
        cache.put(&2, 0.5, at(0.5, mesh(1)));
        assert!(
            cache.get(&1, 0.0173).is_none(),
            "the floor's mesh is the oldest"
        );
        assert_eq!(cache.deflection_for(&1, 1e-9), 0.0173);
        cache.put(&3, 0.5, at(0.5, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1e-9), 1e-9);
    }

    #[test]
    fn a_mesh_larger_than_the_bound_is_given_back_but_not_kept() {
        let cache = MeshCache::new(100);
        let big = cache.put(&1u32, 1.0, at(1.0, mesh(10)));
        assert_eq!(big.triangles.len(), 10);
        assert!(cache.get(&1, 1.0).is_none());
        assert_eq!(cache.bytes(), 0);
        // Nor its floor.
        cache.put(&1, 0.5, at(1.0, mesh(10)));
        assert_eq!(cache.deflection_for(&1, 0.5), 0.5);
    }
}
