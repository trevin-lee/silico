//! Features by ID: each one's place in a list of features, found in logarithmic time, so a
//! read that looks up every feature of a long timeline costs N log N, not N² (#182's review:
//! 20,000 features took 1.45 s to read one by one, by a scan from the start each).

use super::FeatureId;

/// Each ID's place in a list of features, the places sorted by the IDs at them and found by
/// binary search: built once, for a list whose IDs don't change after (a checked document's
/// features, a model's unmeasured solids, a render's bodies), and searched with that list.
/// Deterministic: the order is the IDs', never a hash's, and no two entries are alike (their
/// places differ), so even an unstable sort orders them one way.
///
/// A sorted list rather than a `BTreeMap`, as the gate's `by_id` is: as deterministic (its
/// order is the IDs' own), as fast to search (⌈log₂ n⌉ + 1 comparisons a lookup), and
/// smaller, a place an entry, in one allocation, with no tree nodes' slack and pointers,
/// which a model holds as long as it's read. The IDs stay in the list they're the IDs of,
/// held once: an index holds no copy of one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeatureIndex(Vec<usize>);

impl FeatureIndex {
    /// The places of `ids`, from 0 in the order given. An ID given twice (no checked
    /// document has one) is found at its first place, as a scan from the start finds it.
    pub(crate) fn new<'a>(ids: impl IntoIterator<Item = &'a FeatureId>) -> Self {
        let mut places: Vec<(&FeatureId, usize)> = ids
            .into_iter()
            .enumerate()
            .map(|(place, id)| (id, place))
            .collect();
        // By ID, then place: an ID's first place leads its run, and the dedup keeps it.
        places.sort_unstable();
        places.dedup_by(|later, first| later.0 == first.0);
        FeatureIndex(places.into_iter().map(|(_, place)| place).collect())
    }

    /// The place of the feature `id`, if the list has it, where `at` gives the ID at a place
    /// of the list this index was built from: at most ⌈log₂ n⌉ + 1 comparisons of IDs, for
    /// n IDs. A place `at` has no ID for (the list isn't the one indexed) orders before
    /// every ID, so the search goes past it.
    pub(crate) fn get<'a>(
        &self,
        id: &FeatureId,
        at: impl Fn(usize) -> Option<&'a FeatureId>,
    ) -> Option<usize> {
        #[cfg(test)]
        LOOKED_UP.with(|looked_up| looked_up.set(looked_up.get() + 1));
        let found = self
            .0
            .binary_search_by(|&place| match at(place) {
                Some(held) => held.cmp(id),
                None => std::cmp::Ordering::Less,
            })
            .ok()?;
        self.0.get(found).copied()
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
    use crate::document::{cloned, compared};

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
        let at = |place: usize| ids.get(place);
        assert_eq!(index.get(&id("k0000003"), at), Some(0));
        assert_eq!(index.get(&id("a0000001"), at), Some(1));
        assert_eq!(index.get(&id("z0000009"), at), Some(2));
        assert_eq!(index.get(&id("m0000000"), at), Some(4));
        assert_eq!(index.get(&id("b0000000"), at), None);
        assert_eq!(FeatureIndex::default().get(&id("a0000001"), at), None);
    }

    #[test]
    fn a_lookup_compares_at_most_the_ceiling_of_log2_n_plus_one_ids() {
        // A binary search halves what's left with each comparison: of n IDs, ⌈log₂ n⌉
        // comparisons leave one, and one more decides it. Every ID of 1,000, and one
        // missing: 1,000 is between 2⁹ and 2¹⁰, so at most 11 each.
        let ids: Vec<FeatureId> = (0..1000).map(|i| id(&format!("f{i:07}"))).collect();
        let index = FeatureIndex::new(&ids);
        let at = |place: usize| ids.get(place);
        for (place, feature) in ids.iter().enumerate() {
            let before = compared();
            assert_eq!(index.get(feature, at), Some(place));
            let made = compared() - before;
            assert!((1..=11).contains(&made), "{feature}: {made}");
        }
        let before = compared();
        assert_eq!(index.get(&id("g0000000"), at), None);
        assert!(compared() - before <= 11);
    }

    #[test]
    fn an_index_holds_no_copy_of_an_id() {
        // Its entries are places in the list it was built from, whose IDs it reads there: a
        // thousand features' index clones none of their IDs (it held a clone of each).
        let ids: Vec<FeatureId> = (0..1000).map(|i| id(&format!("f{i:07}"))).collect();
        // Nor does a lookup: every ID found, and one missing, copies none.
        let before = cloned();
        let index = FeatureIndex::new(&ids);
        assert_eq!(index.0.len(), 1000);
        for (place, feature) in ids.iter().enumerate() {
            assert_eq!(index.get(feature, |place| ids.get(place)), Some(place));
        }
        assert_eq!(index.get(&id("g0000000"), |place| ids.get(place)), None);
        assert_eq!(cloned() - before, 0);
    }

    #[test]
    fn searched_with_a_list_other_than_its_own_it_finds_only_ids_at_their_places() {
        // A place the list doesn't have finds nothing; a place holding another ID doesn't
        // pass for the one asked.
        let ids = [id("a0000001"), id("b0000002"), id("c0000003")];
        let index = FeatureIndex::new(&ids);
        let short = [id("a0000001")];
        assert_eq!(index.get(&id("c0000003"), |place| short.get(place)), None);
        let other = [id("a0000001"), id("x0000002"), id("c0000003")];
        assert_eq!(index.get(&id("b0000002"), |place| other.get(place)), None);
        assert_eq!(
            index.get(&id("a0000001"), |place| other.get(place)),
            Some(0)
        );
    }
}
