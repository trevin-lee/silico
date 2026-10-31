//! The render's meshes (docs/design/m4.6-render.md, *In the core*, the meshes): each body
//! meshed at about half a pixel, on a ladder of powers of two so views of nearly one size
//! share a mesh, and kept in memory, shared by the sessions of one kernel. An entry is keyed
//! by the body's step and the deflection the kernel actually used; the first mesh that met
//! a body's floor records it, so a later level under it maps to the floor's entry before
//! any request is sent, and zooming in past the floor meshes once. The floors are kept apart
//! from the meshes, in their own list, least recently used dropped first past its count
//! ([`MESH_FLOORS_KEPT`]): one number a step, so a step's floor outlives its meshes, and a
//! body held at its floor is known to be held without meshing it again. A floor dropped
//! from that list is learnt again by meshing once more, to the same answer: what the cache
//! has seen changes only how long a read takes, never what it says. Apart from the floors,
//! the finest level each step was meshed at as asked is kept too, in a list of its own of
//! the same count: the step's floor is no coarser, so a mesh held at that level or a coarser
//! one is known to be the one the kernel would make ([`MeshCache::known`]). Knowing neither,
//! a held mesh must be meshed again to tell.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

use silico_protocol::{Mesh, MeshAt};

use crate::worker::lock;

/// How much the cache keeps, by its meshes' size, before it drops the least recently used.
pub const MESH_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// How many steps' floors the cache keeps, before it drops the least recently used: each a
/// 32-byte step key, its floor, and when it was used, in two maps, about 5.5 MiB of data in
/// all (some 7 to 9 MiB with the maps' nodes), beside the meshes' 256 MiB. As many steps'
/// finest levels meshed as asked are kept again, apart, as much again.
pub const MESH_FLOORS_KEPT: usize = 1 << 16;

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
/// used dropped first past the bound; and the steps' floors, kept apart, least recently
/// used dropped first past their count.
pub struct MeshCache<K: Ord + Clone> {
    inner: Mutex<Inner<K>>,
}

struct Inner<K> {
    /// By (step, the deflection used's bits).
    entries: BTreeMap<(K, u64), Entry>,
    /// The same entries by when they were last used, oldest first.
    recency: BTreeMap<u64, (K, u64)>,
    /// Each step's floor, once a mesh has met it, and when it was last used.
    floors: BTreeMap<K, (f64, u64)>,
    /// The same floors by when they were last used, oldest first.
    floor_recency: BTreeMap<u64, K>,
    /// Each step's finest deflection a mesh was made at as asked (not at a floor it met),
    /// so its floor is no coarser, and when it was last used; as many as the floors.
    made: BTreeMap<K, (f64, u64)>,
    /// The same by when they were last used, oldest first.
    made_recency: BTreeMap<u64, K>,
    bytes: u64,
    bound: u64,
    floors_kept: usize,
    tick: u64,
}

struct Entry {
    mesh: Arc<Mesh>,
    bytes: u64,
    tick: u64,
}

impl<K: Ord + Clone> MeshCache<K> {
    /// A cache that keeps up to `bound` bytes of meshes, and [`MESH_FLOORS_KEPT`] floors.
    pub fn new(bound: u64) -> Self {
        Self::with_floors(bound, MESH_FLOORS_KEPT)
    }

    /// A cache that keeps up to `bound` bytes of meshes and `floors_kept` steps' floors.
    pub fn with_floors(bound: u64, floors_kept: usize) -> Self {
        MeshCache {
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                recency: BTreeMap::new(),
                floors: BTreeMap::new(),
                floor_recency: BTreeMap::new(),
                made: BTreeMap::new(),
                made_recency: BTreeMap::new(),
                bytes: 0,
                bound,
                floors_kept,
                tick: 0,
            }),
        }
    }

    /// The deflection to ask the step's mesh at for `level`: `level`, or the step's floor
    /// if it's known and coarser (the kernel would mesh at it anyway). The floor, if known,
    /// is now the most recently used.
    pub fn deflection_for(&self, key: &K, level: f64) -> f64 {
        let mut inner = lock(&self.inner);
        match inner.floors.get(key).map(|&(floor, _)| floor) {
            Some(floor) => {
                inner.keep_floor(key, floor);
                level.max(floor)
            }
            None => level,
        }
    }

    /// The deflection the kernel would use for the step at `level`, if the cache knows it
    /// without asking: the floor's, if a mesh met it (or `level`, if coarser); `level`, if a
    /// mesh was made at `level` or finer as asked, so the floor is no coarser. None if it
    /// knows neither: only meshing says. What it knew by is now the most recently used.
    pub fn known(&self, key: &K, level: f64) -> Option<f64> {
        let mut inner = lock(&self.inner);
        if let Some(&(floor, _)) = inner.floors.get(key) {
            inner.keep_floor(key, floor);
            return Some(level.max(floor));
        }
        let &(made, _) = inner.made.get(key)?;
        if level >= made {
            inner.keep_made(key, made);
            Some(level)
        } else {
            None
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
    /// at the floor, which is then recorded, or used again), and gives it back shared. The
    /// least recently used meshes are dropped until the cache is within its bound; the
    /// floors are kept apart, so a step's floor outlives its meshes. A mesh larger than the
    /// bound alone is given back but not kept; its floor is.
    pub fn put(&self, key: &K, asked: f64, made: MeshAt) -> Arc<Mesh> {
        let used = made.deflection;
        let mesh = Arc::new(made.mesh);
        let bytes = mesh_bytes(&mesh);
        let mut inner = lock(&self.inner);
        let at_floor = inner
            .floors
            .get(key)
            .is_some_and(|&(floor, _)| floor == used);
        if used > asked || at_floor {
            inner.keep_floor(key, used);
        } else if used == asked {
            // Made as asked: the floor is no coarser than the finest made so.
            let finest = inner
                .made
                .get(key)
                .map_or(used, |&(made, _)| made.min(used));
            inner.keep_made(key, finest);
        }
        if bytes > inner.bound {
            return mesh;
        }
        let id = (key.clone(), used.to_bits());
        inner.remove(&id);
        while inner.bytes.saturating_add(bytes) > inner.bound {
            let Some((_, oldest)) = inner.recency.pop_first() else {
                break;
            };
            inner.remove(&oldest);
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
        mesh
    }

    /// Drops the step's mesh at `deflection`, if it's kept: a mesh found not to be the
    /// step's, so the next read asks the kernel again. Its step's floor goes too if it's
    /// that deflection, since the reply that was refused is what reported it.
    pub fn forget(&self, key: &K, deflection: f64) {
        let mut inner = lock(&self.inner);
        inner.remove(&(key.clone(), deflection.to_bits()));
        if let Some(&(floor, tick)) = inner.floors.get(key)
            && floor.to_bits() == deflection.to_bits()
        {
            inner.floors.remove(key);
            inner.floor_recency.remove(&tick);
        }
        // So too what a mesh made as asked at it said.
        if let Some(&(made, tick)) = inner.made.get(key)
            && made.to_bits() == deflection.to_bits()
        {
            inner.made.remove(key);
            inner.made_recency.remove(&tick);
        }
    }

    /// How many bytes of meshes are kept.
    pub fn bytes(&self) -> u64 {
        lock(&self.inner).bytes
    }
}

impl<K: Ord + Clone> Inner<K> {
    /// Drops the entry `id`, if it's kept.
    fn remove(&mut self, id: &(K, u64)) {
        let Some(old) = self.entries.remove(id) else {
            return;
        };
        self.recency.remove(&old.tick);
        self.bytes -= old.bytes;
    }

    /// Records the finest deflection the step was made at as asked, now the most recently
    /// used, the least recently used dropped past the floors' count.
    fn keep_made(&mut self, key: &K, made: f64) {
        self.tick += 1;
        let tick = self.tick;
        if let Some((_, old)) = self.made.insert(key.clone(), (made, tick)) {
            self.made_recency.remove(&old);
        }
        self.made_recency.insert(tick, key.clone());
        while self.made.len() > self.floors_kept {
            let Some((_, oldest)) = self.made_recency.pop_first() else {
                break;
            };
            self.made.remove(&oldest);
        }
    }

    /// Records the step's floor, now the most recently used, the least recently used
    /// dropped past the floors' count.
    fn keep_floor(&mut self, key: &K, floor: f64) {
        self.tick += 1;
        let tick = self.tick;
        if let Some((_, old)) = self.floors.insert(key.clone(), (floor, tick)) {
            self.floor_recency.remove(&old);
        }
        self.floor_recency.insert(tick, key.clone());
        while self.floors.len() > self.floors_kept {
            let Some((_, oldest)) = self.floor_recency.pop_first() else {
                break;
            };
            self.floors.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MESH_FLOORS_KEPT, MeshCache, mesh_bytes, mesh_level};
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
    fn the_cache_holds_256_mib() {
        // The design's bound: 256 × 2²⁰ bytes.
        assert_eq!(super::MESH_CACHE_BYTES, 268_435_456);
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
    fn a_steps_floor_outlives_its_meshes() {
        // Room for one mesh: step 1's floor mesh, then step 2's drops it, but not its floor:
        // a level under the floor still asks at it (and meshes again, at it).
        let cache = MeshCache::new(mesh_bytes(&mesh(1)));
        cache.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1.0 / 256.0), 0.0173);
        cache.put(&2, 0.5, at(0.5, mesh(1)));
        assert!(cache.get(&1, 0.0173).is_none());
        assert_eq!(cache.deflection_for(&1, 1.0 / 256.0), 0.0173);
        // Forgetting another step's mesh, or this step's at another deflection, keeps the
        // floor; forgetting the mesh made at the floor drops it, the reply that said it.
        cache.forget(&2, 0.5);
        cache.forget(&1, 0.5);
        assert_eq!(cache.deflection_for(&1, 1e-9), 0.0173);
        cache.forget(&1, 0.0173);
        assert_eq!(cache.deflection_for(&1, 1e-9), 1e-9);
    }

    #[test]
    fn the_floors_kept_are_bounded_by_count_the_least_recently_used_dropped() {
        // Room for two floors. Steps 1 and 2 meet theirs; step 1's is used again, so step 3's
        // drops step 2's, which a level under it no longer finds: it asks the level itself.
        let cache = MeshCache::with_floors(1 << 20, 2);
        cache.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        cache.put(&2, 1.0 / 128.0, at(0.0291, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1e-9), 0.0173);
        cache.put(&3, 1.0 / 128.0, at(0.0412, mesh(1)));
        assert_eq!(cache.deflection_for(&2, 1e-9), 1e-9);
        assert_eq!(cache.deflection_for(&1, 1e-9), 0.0173);
        assert_eq!(cache.deflection_for(&3, 1e-9), 0.0412);
        // Its mesh is still kept, at the floor: asked at the floor, it's found.
        assert!(cache.get(&2, 0.0291).is_some());
        // A cache that keeps no floors never maps a level.
        let none = MeshCache::with_floors(1 << 20, 0);
        none.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        assert_eq!(none.deflection_for(&1, 1e-9), 1e-9);
        assert_eq!(MESH_FLOORS_KEPT, 65_536);
    }

    #[test]
    fn a_level_is_known_by_the_floor_a_mesh_met_or_a_finer_level_made_as_asked() {
        // Nothing seen: nothing known.
        let cache = MeshCache::with_floors(0, 2);
        assert_eq!(cache.known(&1u32, 0.25), None);
        // Made at 2⁻⁵ as asked: that level and coarser are known, finer not.
        cache.put(&1, 1.0 / 32.0, at(1.0 / 32.0, mesh(1)));
        assert_eq!(cache.known(&1, 1.0 / 32.0), Some(1.0 / 32.0));
        assert_eq!(cache.known(&1, 0.5), Some(0.5));
        assert_eq!(cache.known(&1, 1.0 / 64.0), None);
        // Made finer, 2⁻⁶, as asked: now that too; a coarser made as asked changes nothing.
        cache.put(&1, 1.0 / 64.0, at(1.0 / 64.0, mesh(1)));
        cache.put(&1, 0.5, at(0.5, mesh(1)));
        assert_eq!(cache.known(&1, 1.0 / 64.0), Some(1.0 / 64.0));
        assert_eq!(cache.known(&1, 1.0 / 128.0), None);
        // Then the floor met, 0.0123: every level is known, the finer ones at the floor.
        cache.put(&1, 1.0 / 128.0, at(0.0123, mesh(1)));
        assert_eq!(cache.known(&1, 1e-9), Some(0.0123));
        assert_eq!(cache.known(&1, 0.5), Some(0.5));
        // Forgetting a mesh made at the level drops what it said; another step's is its own.
        let other = MeshCache::with_floors(0, 2);
        other.put(&2u32, 0.25, at(0.25, mesh(1)));
        other.forget(&2, 0.5);
        assert_eq!(other.known(&2, 0.25), Some(0.25));
        other.forget(&2, 0.25);
        assert_eq!(other.known(&2, 0.25), None);
        // Kept as many as the floors, apart from them: a third step's drops the oldest's, and
        // no floor.
        let few = MeshCache::with_floors(0, 2);
        few.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        for key in [2, 3, 4] {
            few.put(&key, 0.25, at(0.25, mesh(1)));
        }
        assert_eq!(few.known(&2, 0.25), None);
        assert_eq!(few.known(&4, 0.25), Some(0.25));
        assert_eq!(few.deflection_for(&1, 1e-9), 0.0173);
        // A cache that keeps no floors knows nothing.
        let none = MeshCache::with_floors(0, 0);
        none.put(&1u32, 0.25, at(0.25, mesh(1)));
        assert_eq!(none.known(&1, 0.25), None);
    }

    #[test]
    fn a_mesh_larger_than_the_bound_is_given_back_but_not_kept() {
        let cache = MeshCache::new(100);
        let big = cache.put(&1u32, 1.0, at(1.0, mesh(10)));
        assert_eq!(big.triangles.len(), 10);
        assert!(cache.get(&1, 1.0).is_none());
        assert_eq!(cache.bytes(), 0);
        // But its floor is: a fact of the step, one number.
        cache.put(&1, 0.5, at(1.0, mesh(10)));
        assert_eq!(cache.deflection_for(&1, 0.5), 1.0);
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn a_forgotten_floor_leaves_no_trace_in_the_floors_recency() {
        // Room for two floors. Step 1's floor is forgotten, then steps 2 and 1 meet theirs
        // (1 the more recent); step 3's drops the least recently used, step 2's, not step
        // 1's by a stale entry of its forgotten floor.
        let cache = MeshCache::with_floors(1 << 20, 2);
        cache.put(&1u32, 1.0 / 128.0, at(0.0173, mesh(1)));
        cache.forget(&1, 0.0173);
        cache.put(&2, 1.0 / 128.0, at(0.0291, mesh(1)));
        cache.put(&1, 1.0 / 128.0, at(0.0173, mesh(1)));
        cache.put(&3, 1.0 / 128.0, at(0.0412, mesh(1)));
        assert_eq!(cache.deflection_for(&1, 1e-9), 0.0173);
        assert_eq!(cache.deflection_for(&2, 1e-9), 1e-9);
    }
}
