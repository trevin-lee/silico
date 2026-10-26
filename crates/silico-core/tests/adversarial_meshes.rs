//! Hostile use of the render's mesh ladder and cache (`silico_core::meshes`): every bit
//! pattern of a half pixel through `mesh_level`, checked against the definition (a power of
//! two, no coarser than the half pixel, and the largest such); and the cache driven by long
//! random runs of puts, gets, and level lookups, bounds near the meshes' sizes, floors and
//! re-puts included, against a plain list kept in recency order.

use std::sync::Arc;

use proptest::prelude::*;
use silico_core::meshes::{MeshCache, mesh_level};
use silico_protocol::{Mesh, MeshAt};

// ---- The ladder ----

/// Whether `x` is a power of two, from its bits: a normal number with its mantissa clear,
/// or a subnormal with one bit set.
fn is_power_of_two(x: f64) -> bool {
    let bits = x.to_bits();
    let (exponent, mantissa) = (bits >> 52, bits & ((1 << 52) - 1));
    match exponent {
        0 => mantissa.count_ones() == 1,
        0x7ff => false,
        _ => mantissa == 0,
    }
}

fn half_pixel() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<u64>().prop_map(f64::from_bits),
        (0u64..(1 << 52)).prop_map(f64::from_bits),
        (1u64..=2046, 0u64..4).prop_map(|(e, m)| f64::from_bits((e << 52) | m)),
        (1u64..=2046).prop_map(|e| f64::from_bits(((e << 52) - 1) & !(1 << 63))),
        Just(f64::MAX),
        Just(f64::MIN_POSITIVE),
        Just(f64::MIN_POSITIVE.next_down()),
        Just(5e-324),
        Just(0.0),
        Just(-0.0),
        Just(f64::NAN),
        Just(-f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 20_000, ..ProptestConfig::default() })]

    #[test]
    fn the_level_is_the_largest_power_of_two_no_coarser_than_the_half_pixel(h in half_pixel()) {
        match mesh_level(h) {
            None => prop_assert!(!(h.is_finite() && h > 0.0), "{h:e} refused"),
            Some(level) => {
                prop_assert!(h.is_finite() && h > 0.0, "{h:e} taken");
                prop_assert!(is_power_of_two(level), "{h:e}: {level:e} isn't a power of two");
                prop_assert!(level <= h, "{h:e}: {level:e} is coarser");
                // The next power of two up is coarser (2 × level, exact, or past f64::MAX).
                let next = level * 2.0;
                prop_assert!(next > h, "{h:e}: {next:e} isn't coarser, so {level:e} isn't the largest");
            }
        }
    }
}

// ---- The cache against a plain list ----

/// A mesh whose size (by its lists' lengths, as the cache counts) is `3 × 36 + 12t + 4`
/// bytes, and whose triangle count tells it from every other.
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

fn size(triangles: usize) -> u64 {
    3 * 36 + 12 * triangles as u64 + 4
}

#[derive(Debug, Clone)]
enum Step {
    /// A mesh of `triangles` for `key`, asked at `asked` and made at `used`.
    Put {
        key: u8,
        asked: f64,
        used: f64,
        triangles: usize,
    },
    Get {
        key: u8,
        deflection: f64,
    },
    Level {
        key: u8,
        level: f64,
    },
}

fn deflection() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.25),
        Just(0.5),
        Just(1.0),
        Just(0.0173),
        Just(-0.0),
        Just(0.0),
        Just(f64::NAN),
        Just(f64::MIN_POSITIVE),
        Just(1e300),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0u8..4, deflection(), deflection(), 0usize..12).prop_map(
            |(key, asked, used, triangles)| Step::Put { key, asked, used, triangles }
        ),
        3 => (0u8..4, deflection()).prop_map(|(key, deflection)| Step::Get { key, deflection }),
        1 => (0u8..4, deflection()).prop_map(|(key, level)| Step::Level { key, level }),
    ]
}

/// The cache as a list, least recently used first: what the design says it keeps.
#[derive(Default)]
struct Model {
    kept: Vec<(u8, u64, usize)>,
    floors: Vec<(u8, f64)>,
}

impl Model {
    fn bytes(&self) -> u64 {
        self.kept.iter().map(|&(_, _, t)| size(t)).sum()
    }

    /// A mesh too large is never kept, nor its floor; one kept replaces its own entry
    /// (the step's floor staying), then the oldest go until it fits, each step's floor with
    /// its last mesh; a mesh kept made coarser than asked records its step's floor.
    fn put(&mut self, bound: u64, key: u8, asked: f64, used: f64, triangles: usize) {
        if size(triangles) > bound {
            return;
        }
        let at_floor = self
            .floors
            .iter()
            .any(|&(k, f)| k == key && f.to_bits() == used.to_bits());
        self.kept
            .retain(|&(k, d, _)| (k, d) != (key, used.to_bits()));
        while self.bytes() + size(triangles) > bound && !self.kept.is_empty() {
            let (gone, _, _) = self.kept.remove(0);
            if !self.kept.iter().any(|&(k, _, _)| k == gone) {
                self.floors.retain(|&(k, _)| k != gone);
            }
        }
        self.kept.push((key, used.to_bits(), triangles));
        if used > asked || at_floor {
            self.floors.retain(|&(k, _)| k != key);
            self.floors.push((key, used));
        }
    }

    fn get(&mut self, key: u8, deflection: f64) -> Option<usize> {
        let at = self
            .kept
            .iter()
            .position(|&(k, d, _)| (k, d) == (key, deflection.to_bits()))?;
        let entry = self.kept.remove(at);
        self.kept.push(entry);
        Some(entry.2)
    }

    fn level(&self, key: u8, level: f64) -> f64 {
        // The larger of the two (a NaN level, which the ladder never gives, the floor).
        match self.floors.iter().find(|&&(k, _)| k == key) {
            Some(&(_, floor)) => level.max(floor),
            None => level,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1_000, ..ProptestConfig::default() })]

    #[test]
    fn the_cache_keeps_what_a_plain_lru_list_keeps(
        bound in prop_oneof![Just(0u64), Just(size(0)), Just(size(3) * 2), 0u64..2_000],
        steps in proptest::collection::vec(step(), 1..120),
    ) {
        let cache = MeshCache::<u8>::new(bound);
        let mut model = Model::default();
        for (i, s) in steps.iter().enumerate() {
            match *s {
                Step::Put { key, asked, used, triangles } => {
                    let given = cache.put(&key, asked, MeshAt { mesh: mesh(triangles), deflection: used });
                    prop_assert_eq!(given.triangles.len(), triangles, "step {}", i);
                    model.put(bound, key, asked, used, triangles);
                }
                Step::Get { key, deflection } => {
                    let got = cache.get(&key, deflection).map(|m| m.triangles.len());
                    prop_assert_eq!(got, model.get(key, deflection), "step {}: {:?}", i, s);
                }
                Step::Level { key, level } => {
                    let got = cache.deflection_for(&key, level);
                    let want = model.level(key, level);
                    prop_assert_eq!(got.to_bits(), want.to_bits(), "step {}: {:?}", i, s);
                }
            }
            prop_assert_eq!(cache.bytes(), model.bytes(), "step {}: {:?}", i, s);
            prop_assert!(cache.bytes() <= bound, "step {}: over the bound", i);
        }
    }
}

#[test]
fn a_mesh_given_back_is_shared_not_copied() {
    // Two gets of one entry, and the put's own, are one mesh.
    let cache = MeshCache::new(1 << 20);
    let put = cache.put(&1u8, 0.5, at(0.5, mesh(4)));
    let a = cache.get(&1, 0.5).unwrap();
    let b = cache.get(&1, 0.5).unwrap();
    assert!(Arc::ptr_eq(&put, &a) && Arc::ptr_eq(&a, &b));
}

#[test]
fn many_threads_put_and_get_and_the_bound_holds() {
    // Eight threads, each a thousand puts and gets over a few keys, on a cache with room for
    // a few meshes: the count stays within the bound and matches what's kept.
    let bound = size(10) * 3;
    let cache = Arc::new(MeshCache::<u8>::new(bound));
    let threads: Vec<_> = (0..8u8)
        .map(|t| {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                for i in 0..1000usize {
                    let key = u8::try_from((usize::from(t) + i) % 5).unwrap();
                    let d = [0.25, 0.5, 1.0][i % 3];
                    if i % 2 == 0 {
                        cache.put(&key, d, at(d, mesh(i % 11)));
                    } else if let Some(m) = cache.get(&key, d) {
                        assert!(m.triangles.len() <= 10);
                    }
                    assert!(cache.bytes() <= bound);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    // What's left is all gettable and adds up.
    let mut total = 0;
    for key in 0..5u8 {
        for d in [0.25, 0.5, 1.0] {
            if let Some(m) = cache.get(&key, d) {
                total += size(m.triangles.len());
            }
        }
    }
    assert_eq!(total, cache.bytes());
}
