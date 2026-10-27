//! Features by ID: each one's place in a list of features, found in logarithmic time, so a
//! read that looks up every feature of a long timeline costs N log N, not N² (#182's review:
//! 20,000 features took 1.45 s to read one by one, by a scan from the start each).

use super::FeatureId;

/// Each ID's place in a list of features, sorted by ID and found by binary search: built
/// once, for a list whose IDs don't change after (a checked document's features, a model's
/// unmeasured solids). Deterministic: the order is the IDs', never a hash's, and no two entries are
/// alike (their places differ), so even an unstable sort orders them one way.
///
/// A sorted list rather than a `BTreeMap`, as the gate's `by_id` is: as deterministic (its
/// order is the IDs' own), as fast to search (⌈log₂ n⌉ + 1 comparisons a lookup), and
/// smaller, one entry of an ID and a place each, in one allocation, with no tree nodes'
/// slack and pointers, which a model holds as long as it's read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeatureIndex(Vec<(FeatureId, usize)>);

impl FeatureIndex {
    /// The places of `ids`, from 0 in the order given. An ID given twice (no checked
    /// document has one) is found at its first place, as a scan from the start finds it.
    pub(crate) fn new<'a>(ids: impl IntoIterator<Item = &'a FeatureId>) -> Self {
        let mut places: Vec<(FeatureId, usize)> = ids
            .into_iter()
            .enumerate()
            .map(|(place, id)| (id.clone(), place))
            .collect();
        // By ID, then place: an ID's first place leads its run, and the dedup keeps it.
        places.sort_unstable();
        places.dedup_by(|later, first| later.0 == first.0);
        FeatureIndex(places)
    }

    /// The place of the feature `id`, if the list has it: at most ⌈log₂ n⌉ + 1 comparisons of
    /// IDs, for n IDs.
    pub(crate) fn get(&self, id: &FeatureId) -> Option<usize> {
        #[cfg(test)]
        LOOKED_UP.with(|looked_up| looked_up.set(looked_up.get() + 1));
        let at = self.0.binary_search_by(|(held, _)| held.cmp(id)).ok()?;
        self.0.get(at).map(|&(_, place)| place)
    }
}

#[cfg(test)]
thread_local! {
    /// How many lookups this thread has made in indexes.
    static LOOKED_UP: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many lookups ([`FeatureIndex::get`]) this thread has made so far, found or not: so a
/// test that bounds a read's comparisons of IDs ([`compared`](super::compared)) can say how
/// many lookups it made too, and notice one lost.
#[cfg(test)]
pub(crate) fn looked_up() -> u64 {
    LOOKED_UP.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::compared;

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    #[test]
    fn each_id_is_found_at_its_place_and_an_id_held_twice_at_its_first() {
        let ids = [
            id("k0000003"),
            id("a0000001"),
            id("z0000009"),
            id("a0000001"),
            id("m0000000"),
        ];
        let index = FeatureIndex::new(&ids);
        assert_eq!(index.get(&id("k0000003")), Some(0));
        assert_eq!(index.get(&id("a0000001")), Some(1));
        assert_eq!(index.get(&id("z0000009")), Some(2));
        assert_eq!(index.get(&id("m0000000")), Some(4));
        assert_eq!(index.get(&id("b0000000")), None);
        assert_eq!(FeatureIndex::default().get(&id("a0000001")), None);
    }

    #[test]
    fn a_lookup_compares_at_most_the_ceiling_of_log2_n_plus_one_ids() {
        // A binary search halves what's left with each comparison: of n IDs, ⌈log₂ n⌉
        // comparisons leave one, and one more decides it. Every ID of 1,000, and one
        // missing: 1,000 is between 2⁹ and 2¹⁰, so at most 11 each.
        let ids: Vec<FeatureId> = (0..1000).map(|i| id(&format!("f{i:07}"))).collect();
        let index = FeatureIndex::new(&ids);
        for (place, feature) in ids.iter().enumerate() {
            let before = compared();
            assert_eq!(index.get(feature), Some(place));
            let made = compared() - before;
            assert!((1..=11).contains(&made), "{feature}: {made}");
        }
        let before = compared();
        assert_eq!(index.get(&id("g0000000")), None);
        assert!(compared() - before <= 11);
    }
}
