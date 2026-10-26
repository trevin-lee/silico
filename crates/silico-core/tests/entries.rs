//! M4.4: `elements` and `query` on a model (`silico_core::command::elements`;
//! docs/design/m4.1-command-api.md, section 7): their order, pages and cursors, refusals,
//! filters, and ties. On the fake kernel, which describes each element as its number
//! (`fake::described`: every face of type other and area 1, every edge 1 long), so the
//! expected values here are counts by hand and those numbers; an entry's geometry is tested
//! on a worker (silico-kernel's tests/entries.rs).

use super::describing::fused_boxes;
use super::example;
use super::listing::{TWO_BODIES, codes, model_of};
use super::model;

use example::EXAMPLE;
use model::{checked, id, published};
use silico_core::ErrorCode;
use silico_core::command::Revision;
use silico_core::command::elements::{
    Cursor, ENTRIES_AT_MOST, ElementType, Facing, MeasureName, Order, Quantity, Query, Range,
    entries, list, query,
};
use silico_core::document::{Angle, Length, Measure};
use silico_core::fake::FakeKernel;
use silico_core::kernel::ElementKind;
use silico_core::recompute::Recompute;

#[test]
fn every_body_is_listed_in_timeline_order_then_faces_edges_and_vertices_by_name() {
    let model = model_of(TWO_BODIES);
    let listed = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    // Two boxes, 26 elements each, by hand.
    assert_eq!(listed.total, 52);
    assert_eq!(listed.entries.len(), 52);
    assert!(listed.next.is_none());
    let features: Vec<&str> = listed.entries.iter().map(|e| e.feature.as_str()).collect();
    assert_eq!(features[..26], ["aaaaaaaa"; 26]);
    assert_eq!(features[26..], ["bbbbbbbb"; 26]);
    for body in listed.entries.chunks(26) {
        let kinds: Vec<ElementKind> = body.iter().map(|e| e.kind).collect();
        let want: Vec<ElementKind> = [
            (ElementKind::Face, 6),
            (ElementKind::Edge, 12),
            (ElementKind::Vertex, 8),
        ]
        .into_iter()
        .flat_map(|(kind, n)| std::iter::repeat_n(kind, n))
        .collect();
        assert_eq!(kinds, want);
        // By name, byte by byte, within each kind.
        for kind in body.chunk_by(|a, b| a.kind == b.kind) {
            let names: Vec<String> = kind.iter().map(|e| e.name.to_string()).collect();
            let mut sorted = names.clone();
            sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            assert_eq!(names, sorted);
        }
    }
    // A face's entry, as the fake describes it: of type other, area 1, its own name stable.
    let face = &listed.entries[0];
    assert_eq!(face.r#type, ElementType::Other);
    assert_eq!(face.area, Some(Measure::from_millionths(1_000_000)));
    assert!(face.stable);
    assert_eq!(face.normal, None);
    assert_eq!(face.length, None);
    // An edge: 1 long, not straight (the fake's are of type other), so no midpoint.
    let edge = &listed.entries[6];
    assert_eq!(edge.kind, ElementKind::Edge);
    assert_eq!(edge.length, Some(Length::from_nm(1_000_000)));
    assert_eq!(edge.straight, Some(false));
    assert_eq!(edge.midpoint, None);
    // A vertex: its point, and nothing else.
    let vertex = &listed.entries[18];
    assert_eq!(vertex.r#type, ElementType::Point);
    assert!(vertex.position.is_some());
    assert_eq!(
        (vertex.area, vertex.length, vertex.centroid),
        (None, None, None)
    );
}

#[test]
fn pages_follow_on_from_their_cursor_and_together_list_every_entry_once() {
    let model = model_of(TWO_BODIES);
    let whole = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    let mut paged = Vec::new();
    let mut after: Option<Cursor> = None;
    let mut pages = 0;
    loop {
        let page = list(&model, Revision::OPENED, None, after.as_ref(), 10).unwrap();
        assert_eq!(page.total, 52);
        pages += 1;
        paged.extend(page.entries.iter().cloned());
        match page.next {
            Some(next) => {
                // The cursor is the page's last entry, of the revision read.
                let last = page.entries.last().unwrap();
                assert_eq!(next.revision, Revision::OPENED);
                assert_eq!(
                    (next.feature.clone(), next.name.clone()),
                    (last.feature.clone(), last.name.to_string())
                );
                after = Some(next);
            }
            None => break,
        }
    }
    // 52 in pages of 10: six pages, the last of 2.
    assert_eq!(pages, 6);
    assert_eq!(paged, whole.entries);
    // A page that ends exactly at the end has no next.
    let first_body = list(&model, Revision::OPENED, Some(&id("aaaaaaaa")), None, 26).unwrap();
    assert_eq!((first_body.total, first_body.entries.len()), (26, 26));
    assert!(first_body.next.is_none());
}

#[test]
fn a_cursor_of_another_revision_is_a_conflict_said_with_every_other_reason() {
    // A cursor of revision 1, read against the model of revision 2, with a limit out of
    // range too: both refused, in the request's order (feature, after, limit).
    let model = model_of(TWO_BODIES);
    let stale = list(&model, Revision::OPENED, None, None, 1)
        .unwrap()
        .next
        .unwrap();
    let later = Revision::OPENED.next().unwrap();
    let refused = list(&model, later, None, Some(&stale), 0).unwrap_err();
    assert_eq!(
        codes(&refused),
        [
            (ErrorCode::Conflict, Some("after".to_owned())),
            (ErrorCode::InvalidInput, Some("limit".to_owned())),
        ]
    );
    assert_eq!(refused.total(), 2);
    assert_eq!(
        refused.errors()[0].message(),
        "the cursor is of revision 1, and the model read is of revision 2"
    );
    assert_eq!(
        refused.errors()[0].suggestion(),
        Some("start again with after set to null")
    );
    // Of the revision read, it goes on.
    let fresh = list(&model, later, None, None, 1).unwrap().next.unwrap();
    assert_eq!(fresh.revision, later);
    assert!(list(&model, later, None, Some(&fresh), 1).is_ok());
    // A cursor that names no entry of the listing is refused, naming after.
    let mut lost = fresh.clone();
    lost.name = "k7f2qx3m.box.Face99".to_owned();
    let refused = list(&model, later, None, Some(&lost), 1).unwrap_err();
    assert_eq!(
        codes(&refused),
        [(ErrorCode::InvalidInput, Some("after".to_owned()))]
    );
}

#[test]
fn a_limit_out_of_range_is_refused_naming_limit() {
    let model = model_of(TWO_BODIES);
    for limit in [0, 1001, u32::MAX] {
        let refused = list(&model, Revision::OPENED, None, None, limit).unwrap_err();
        assert_eq!(
            codes(&refused),
            [(ErrorCode::InvalidInput, Some("limit".to_owned()))]
        );
        assert_eq!(
            refused.errors()[0].message(),
            format!("limit must be from 1 to 1000, got {limit}")
        );
    }
    // 1 and 1000 are in range.
    assert_eq!(
        list(&model, Revision::OPENED, None, None, 1)
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(
        list(&model, Revision::OPENED, None, None, 1000)
            .unwrap()
            .entries
            .len(),
        52
    );
}

#[test]
fn a_feature_that_makes_no_solid_or_that_the_document_hasnt_is_refused_naming_feature() {
    let model = model_of(&sketch_and_box());
    let sketch = entries(&model, &id("k7f2qx3m")).unwrap_err();
    assert_eq!(
        codes(&sketch),
        [(ErrorCode::InvalidInput, Some("feature".to_owned()))]
    );
    assert_eq!(
        sketch.errors()[0].message(),
        "k7f2qx3m is a sketch, which makes no solid"
    );
    assert_eq!(sketch.errors()[0].feature(), Some("k7f2qx3m"));
    let missing = entries(&model, &id("zzzzzzzz")).unwrap_err();
    assert_eq!(
        missing.errors()[0].message(),
        "the document has no feature zzzzzzzz"
    );
    // The box beside it lists.
    assert_eq!(entries(&model, &id("bbbbbbbb")).unwrap().len(), 26);
}

/// The sketch test's rectangle (tests/sketch.rs), and a box after it.
fn sketch_and_box() -> String {
    r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {
      "id": "k7f2qx3m",
      "type": "sketch",
      "v": 1,
      "name": "Base",
      "plane": "xy",
      "points": [
        {"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false},
        {"id": "2p3q4r5s", "x": 0, "y": 20, "construction": false},
        {"id": "4e5f6g7h", "x": 40, "y": 0, "construction": false},
        {"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}
      ],
      "lines": [
        {"id": "6t7v8w9x", "from": "0a1b2c3d", "to": "4e5f6g7h", "construction": false},
        {"id": "a0b1c2d3", "from": "4e5f6g7h", "to": "8j9k0m1n", "construction": false},
        {"id": "j8k9m0n1", "from": "8j9k0m1n", "to": "2p3q4r5s", "construction": false},
        {"id": "p2q3r4s5", "from": "2p3q4r5s", "to": "0a1b2c3d", "construction": false}
      ],
      "circles": [],
      "arcs": [],
      "constraints": []
    },
    {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}
  ]
}
"#
    .to_owned()
}

#[test]
fn a_feature_that_failed_gives_its_failure_and_one_it_used_says_so() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no pins today"))
    });
    let model = published(&engine, checked(EXAMPLE));
    let failed = entries(&model, &id("a91cw0de")).unwrap_err();
    assert_eq!(failed.errors()[0].message(), "no pins today");
    let blocked = entries(&model, &id("m3tq8v0z")).unwrap_err();
    assert_eq!(
        blocked.errors()[0].message(),
        "m3tq8v0z didn't build, since it uses a91cw0de, which failed"
    );
    assert_eq!(blocked.errors()[0].parameter(), Some("feature"));
}

#[test]
fn a_query_without_an_order_lists_in_elements_order_and_counts_every_match() {
    let model = model_of(TWO_BODIES);
    let whole = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    let faces = query(&model, &Query::of(ElementKind::Face, 5)).unwrap();
    // 12 faces matched; the first 5, as elements lists them; no order, so no tie.
    assert_eq!(faces.matched, 12);
    let want: Vec<_> = whole
        .entries
        .iter()
        .filter(|e| e.kind == ElementKind::Face)
        .take(5)
        .cloned()
        .collect();
    assert_eq!(faces.entries, want);
    assert_eq!(faces.tied, None);
    // Of one body only.
    let mut one = Query::of(ElementKind::Vertex, 100);
    one.feature = Some(id("bbbbbbbb"));
    let vertices = query(&model, &one).unwrap();
    assert_eq!(vertices.matched, 8);
    assert!(
        vertices
            .entries
            .iter()
            .all(|e| e.feature.as_str() == "bbbbbbbb")
    );
}

#[test]
fn a_tie_at_the_cut_is_returned_whole_past_the_limit_when_it_fits() {
    // Every face's area is 1: ordered by area, the largest is a tie of all 12, which fits,
    // so all 12 come back for a limit of 1.
    let model = model_of(TWO_BODIES);
    let mut largest = Query::of(ElementKind::Face, 1);
    largest.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = query(&model, &largest).unwrap();
    assert_eq!(got.matched, 12);
    assert_eq!(got.entries.len(), 12);
    let tied = got.tied.unwrap();
    assert_eq!((tied.count, tied.included), (12, true));
    // A limit past the matches cuts nothing: no tie.
    largest.limit = 12;
    assert_eq!(query(&model, &largest).unwrap().tied, None);
}

#[test]
fn a_tie_too_large_to_return_returns_none_of_it_and_says_how_large() {
    // 170 boxes fused: 1,020 faces of area 1, a tie past 1,000.
    let model = model_of(&fused_boxes(170));
    let mut largest = Query::of(ElementKind::Face, 1);
    largest.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = query(&model, &largest).unwrap();
    assert_eq!(got.matched, 1020);
    assert!(got.entries.is_empty());
    let tied = got.tied.unwrap();
    assert_eq!((tied.count, tied.included), (1020, false));
}

#[test]
fn ranges_keep_what_they_include_at_both_ends_and_nothing_else() {
    let model = model_of(TWO_BODIES);
    let area = |millionths| Some(Quantity::Area(Measure::from_millionths(millionths)));
    for (at_least, at_most, matched) in [
        (area(1_000_000), area(1_000_000), 12),
        (area(1_000_001), None, 0),
        (None, area(999_999), 0),
        (None, area(1_000_000), 12),
    ] {
        let mut q = Query::of(ElementKind::Face, 100);
        q.measures = vec![Range {
            measure: MeasureName::Area,
            at_least,
            at_most,
        }];
        assert_eq!(
            query(&model, &q).unwrap().matched,
            matched,
            "{at_least:?} to {at_most:?}"
        );
    }
    // A face has no length; ordering by radius keeps only faces that have one (none here).
    let mut by_radius = Query::of(ElementKind::Face, 100);
    by_radius.order = Some(Order {
        by: MeasureName::Radius,
        descending: false,
    });
    let none = query(&model, &by_radius).unwrap();
    assert_eq!((none.matched, none.entries.len()), (0, 0));
}

#[test]
fn a_query_that_cant_mean_anything_is_refused_with_every_reason() {
    let model = model_of(TWO_BODIES);
    let mut q = Query::of(ElementKind::Edge, 0);
    q.r#type = Some(ElementType::Plane);
    q.facing = Some(Facing {
        direction: [0.0, 0.0, 0.0],
        within: Angle::from_ndeg(181_000_000_000),
    });
    q.measures = vec![Range {
        measure: MeasureName::Area,
        at_least: Some(Quantity::Length(Length::from_nm(2))),
        at_most: Some(Quantity::Length(Length::from_nm(1))),
    }];
    q.order = Some(Order {
        by: MeasureName::HalfAngle,
        descending: false,
    });
    let refused = query(&model, &q).unwrap_err();
    let params: Vec<Option<String>> = codes(&refused).into_iter().map(|(_, p)| p).collect();
    assert_eq!(
        params,
        [
            "type",
            "facing",
            "facing.direction",
            "facing.within",
            "measures.0.measure",
            "measures.0.at_least",
            "measures.0.at_most",
            "measures.0.at_least",
            "order.by",
            "limit",
        ]
        .map(|p| Some(p.to_owned()))
    );
    assert_eq!(
        refused.errors()[0].message(),
        "an edge is never of type plane"
    );
    // The types an edge can be, and the measures it can have, by hand from the design's
    // *Element entries*.
    assert_eq!(
        refused.errors()[0].candidates(),
        [
            "bezier",
            "bspline",
            "offset",
            "other",
            "line",
            "circle",
            "ellipse",
            "hyperbola",
            "parabola"
        ]
    );
    assert_eq!(
        refused.errors()[4].candidates(),
        ["length", "radius", "diameter"]
    );
    assert_eq!(refused.errors()[4].message(), "an edge has no area");
    assert_eq!(
        refused.errors()[5].message(),
        "area is an area, and at_least isn't"
    );
    assert_eq!(
        refused.errors()[1].message(),
        "only planar faces face a direction, and kind is edge"
    );
    assert_eq!(
        refused.errors()[2].message(),
        "facing.direction must be a direction, got [0, 0, 0]"
    );
    // A solid is no kind to query, with the kinds as candidates.
    let solid = query(&model, &Query::of(ElementKind::Solid, 1)).unwrap_err();
    assert_eq!(
        solid.errors()[0].message(),
        "kind must be face, edge, or vertex, got solid"
    );
    assert_eq!(solid.errors()[0].candidates(), ["face", "edge", "vertex"]);
}

/// A document of `2^levels` boxes 20 apart along x, fused pairwise, then those fusions
/// pairwise, and so on, into one, `f{levels}00000`: one body of every box's elements (the
/// fake keeps each box's), named in `levels + 1` passes over them, not a chain's square.
fn fused_tree(levels: u32) -> String {
    let n = 1usize << levels;
    let mut features = Vec::new();
    for i in 0..n {
        features.push(format!(
            r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
            i * 20
        ));
    }
    let id = |level: u32, i: usize| {
        if level == 0 {
            format!("b{i:07}")
        } else {
            format!("f{level:02}{i:05}")
        }
    };
    for level in 1..=levels {
        for i in 0..(n >> level) {
            features.push(format!(
                r#"{{"id": "{}", "type": "fuse", "v": 1, "name": "Fuse", "base": "{}", "tool": "{}"}}"#,
                id(level, i),
                id(level - 1, 2 * i),
                id(level - 1, 2 * i + 1)
            ));
        }
    }
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

#[test]
fn walking_a_large_body_in_pages_costs_one_build_and_a_copy_of_each_page() {
    // The architect's M1: 2,048 boxes fused into one body of 12,288 faces, 24,576 edges, and
    // 16,384 vertices (53,248 entries, by hand), walked in pages of 1,000: 54 pages, the
    // last of 248. Its entries are built once, by the first page, so the 53 pages after it
    // take less than its time ten times over; built again for each page, as they were, they
    // would take 53 times it. Compared with the first page's own time on this machine, so
    // no wall clock decides it.
    let model = model_of(&fused_tree(11));
    let body = id("f1100000");
    assert_eq!(model.bodies(), std::slice::from_ref(&body));
    // Named first, so the first page's time is the entries' build, not the naming's.
    assert!(model.element_map(&body).is_some());
    // Each timed by the test thread's own CPU time, which a loaded machine doesn't change
    // (a gate starting mid-test takes wall time, not this thread's: common/within.rs).
    let start = cpu_time::ThreadTime::now();
    let first = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    let page_one = start.elapsed();
    assert_eq!((first.total, first.entries.len()), (53_248, 1000));
    let start = cpu_time::ThreadTime::now();
    let (mut pages, mut seen) = (1, first.entries.len());
    let mut after = first.next;
    let mut last = None;
    while let Some(cursor) = after {
        let page = list(
            &model,
            Revision::OPENED,
            None,
            Some(&cursor),
            ENTRIES_AT_MOST,
        )
        .unwrap();
        assert_eq!(page.total, 53_248);
        pages += 1;
        seen += page.entries.len();
        after = page.next;
        last = page.entries.last().cloned();
    }
    let walk = start.elapsed();
    assert_eq!((pages, seen), (54, 53_248));
    // The walk ends on the body's last vertex, as a whole listing does.
    let whole = entries(&model, &body).unwrap();
    assert_eq!(last.as_ref(), whole.last());
    assert!(
        walk < page_one * 10,
        "53 pages took {walk:?}, and the first {page_one:?}: each page builds them again"
    );
}

#[test]
fn a_feature_whose_references_broke_is_refused_with_every_one_counted() {
    // A fillet of 150 edges the box hasn't (Edge13 to Edge162): its refusal lists its first
    // 100 and counts all 150, as `check` does; with a limit of 0 too, 100 listed (the most
    // a refusal lists) and 151 counted.
    let edges: Vec<String> = (13..163)
        .map(|n| format!("\"k7f2qx3m.box.Edge{n}\""))
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}, {{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{}], "radius": 1}}]}}"#,
        edges.join(", ")
    );
    let model = model_of(&text);
    let fillet = id("f0000001");
    let refused = entries(&model, &fillet).unwrap_err();
    assert_eq!((refused.errors().len(), refused.total()), (100, 150));
    assert!(
        refused
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::ReferenceBroken && e.feature() == Some("f0000001"))
    );
    let listed = list(&model, Revision::OPENED, Some(&fillet), None, 0).unwrap_err();
    assert_eq!((listed.errors().len(), listed.total()), (100, 151));
    assert_eq!(listed.errors()[0].code(), ErrorCode::ReferenceBroken);
    let mut q = Query::of(ElementKind::Face, 0);
    q.feature = Some(fillet);
    let queried = query(&model, &q).unwrap_err();
    assert_eq!((queried.errors().len(), queried.total()), (100, 151));
    // A fillet that failed consumes nothing: the box is the body, and lists.
    assert_eq!(
        list(&model, Revision::OPENED, None, None, 1000)
            .unwrap()
            .total,
        26
    );
}

/// A triangle in xy, padded 5 up: its faces, edges, and vertices named from the sketch's
/// lines and points (`Face(G s0000001.sketch.Edge.na000000)`).
const PADDED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "w5mb4c2n", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}]}"#;

#[test]
fn a_cursor_on_an_element_named_from_a_sketch_pages_on() {
    // A pad's elements are named from its sketch's (a feature that makes no solid of its
    // own): a cursor holding one is read as a name, as its rebuild reads them, and pages on.
    // A prism of a triangle: 5 faces, 9 edges, 6 vertices.
    let model = model_of(PADDED);
    let all = list(&model, Revision::OPENED, None, None, 1000).unwrap();
    assert_eq!(all.total, 20);
    assert!(
        all.entries
            .iter()
            .any(|e| e.name.to_string().contains("s0000001.sketch")),
        "{:?}",
        all.entries
            .iter()
            .map(|e| e.name.to_string())
            .collect::<Vec<_>>()
    );
    let mut paged = Vec::new();
    let mut after = None;
    loop {
        let page = list(&model, Revision::OPENED, None, after.as_ref(), 1).unwrap();
        paged.extend(page.entries);
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(paged, all.entries);
}
