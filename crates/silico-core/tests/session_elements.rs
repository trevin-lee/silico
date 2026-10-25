//! M4.4: `elements` and `query` on a session (docs/design/m4.1-command-api.md, section 7):
//! of the current model, waited for, or the newest published one (decision 33), each answer
//! saying which model it read, and a published read asking the kernel nothing, its solids
//! described as they were rebuilt. On the fake kernel, which describes each element as its
//! number; the entries themselves are tested in tests/entries.rs.

use super::listing::{answer, codes, fixture};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use silico_core::ErrorCode;
use silico_core::command::elements::{Cursor, MeasureName, Order, Query};
use silico_core::command::{ModelChoice, ModelRead, Revision};
use silico_core::document::FeatureId;
use silico_core::kernel::{BooleanOp, ElementKind, Op};

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

fn faces(limit: u32) -> Query {
    Query::of(ElementKind::Face, limit)
}

#[test]
fn elements_of_the_current_model_wait_for_its_rebuild_and_say_which_model_they_read() {
    let f = fixture();
    let s = f.open("box.silico", BOX);
    let listed = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 1000),
    );
    assert_eq!(
        listed.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current
        })
    );
    let listed = listed.result.unwrap();
    // A box: 6 faces, 12 edges, 8 vertices.
    assert_eq!(listed.total, 26);
    assert!(listed.next.is_none());
    // Of one feature, by its ID, in pages: the cursor holds the revision read.
    let page = answer(f.sessions.elements(
        &s,
        ModelChoice::Current,
        Some(FeatureId::new("k7f2qx3m").unwrap()),
        None,
        10,
    ))
    .result
    .unwrap();
    assert_eq!(page.entries.len(), 10);
    let next = page.next.unwrap();
    assert_eq!(next.revision, Revision::OPENED);
    let rest = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, Some(next), 1000),
    )
    .result
    .unwrap();
    assert_eq!(rest.entries.len(), 16);
    assert_eq!(rest.entries, listed.entries[10..]);
}

#[test]
fn a_published_read_asks_the_kernel_nothing_its_solids_described_as_they_were_rebuilt() {
    let f = fixture();
    let s = f.open("box.silico", BOX);
    // Nothing published yet: refused, naming model.
    let none = answer(
        f.sessions
            .elements(&s, ModelChoice::Published, None, None, 10),
    );
    let problems = none.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("model"));
    // Once the current model is had, the published read lists it, and asks nothing more.
    let current = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 1000),
    )
    .result
    .unwrap();
    let asked = f.kernel.calls().len();
    let published = answer(
        f.sessions
            .elements(&s, ModelChoice::Published, None, None, 1000),
    );
    assert_eq!(
        published.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published
        })
    );
    assert_eq!(published.result.unwrap().entries, current.entries);
    let queried = answer(f.sessions.query(&s, ModelChoice::Published, faces(2)));
    assert_eq!(queried.result.unwrap().matched, 6);
    assert_eq!(f.kernel.calls().len(), asked);
}

#[test]
fn a_query_on_a_session_ties_as_on_a_model_and_a_bad_one_is_refused_in_the_response() {
    let f = fixture();
    let s = f.open("box.silico", BOX);
    // Every face of area 1 on the fake: the largest is a tie of all six, returned whole.
    let mut largest = faces(1);
    largest.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = answer(f.sessions.query(&s, ModelChoice::Current, largest))
        .result
        .unwrap();
    assert_eq!(got.entries.len(), 6);
    let tied = got.tied.unwrap();
    assert_eq!((tied.count, tied.included), (6, true));
    // A limit of 0 is refused, the session still answering.
    let refused = answer(f.sessions.query(&s, ModelChoice::Current, faces(0)));
    assert_eq!(refused.session.map(|info| info.id), Some(s));
    let problems = refused.result.unwrap_err();
    assert_eq!(
        (
            problems.errors()[0].code(),
            problems.errors()[0].parameter()
        ),
        (ErrorCode::InvalidInput, Some("limit"))
    );
}

/// A box, a pin, and their fusion `ffffffff`: one body, the fusion, once it builds.
const FUSED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "a91cw0de", "type": "box", "v": 1, "name": "Pin", "position": [50, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "ffffffff", "type": "fuse", "v": 1, "name": "Both", "base": "k7f2qx3m", "tool": "a91cw0de"}]}"#;

#[test]
fn a_model_that_holds_a_fault_gives_no_cursor_and_refuses_one_as_a_conflict() {
    // The format review's: a worker crashes on the fuse, so the model of revision 1 holds a
    // fault, and its bodies are the two boxes (a failed feature consumes nothing). The
    // next read of the current model rebuilds it, the fuse builds, and the body is the
    // fusion: another model of the same revision. A page of the faulted one gives no
    // cursor, and a cursor read against it is a conflict, so pages of the two are never
    // joined; once rebuilt, the listing goes on by cursor as any does.
    let f = fixture();
    let fault = Arc::new(AtomicBool::new(true));
    let faulting = Arc::clone(&fault);
    f.kernel.refuse(move |op| {
        (matches!(
            op,
            Op::Boolean {
                op: BooleanOp::Fuse,
                ..
            }
        ) && faulting.load(Ordering::SeqCst))
        .then(|| silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed"))
    });
    let s = f.open("fused.silico", FUSED);
    let page = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 10),
    );
    assert_eq!(page.model.map(|read| read.revision), Some(Revision::OPENED));
    let page = page.result.unwrap();
    // Two boxes' 26 entries each, by hand; ten listed, and no cursor though more follow.
    assert_eq!((page.total, page.entries.len(), page.next), (52, 10, None));
    // A cursor made from the page's last entry, of the revision read.
    let last = page.entries.last().unwrap();
    let cursor = Cursor {
        revision: Revision::OPENED,
        feature: last.feature.clone(),
        name: last.name.to_string(),
    };
    fault.store(false, Ordering::SeqCst);
    // The published model is still the faulted one: a conflict, naming after.
    let refused =
        answer(
            f.sessions
                .elements(&s, ModelChoice::Published, None, Some(cursor.clone()), 10),
        )
        .result
        .unwrap_err();
    assert_eq!(
        codes(&refused),
        [(ErrorCode::Conflict, Some("after".to_owned()))]
    );
    assert_eq!(
        refused.errors()[0].suggestion(),
        Some("check again, then list from the start")
    );
    // The current model is rebuilt, the fault passed: the fusion alone, its cursor given,
    // and the box's cursor names nothing it holds.
    let rebuilt = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 10),
    )
    .result
    .unwrap();
    assert_eq!(
        rebuilt.entries[0].feature,
        FeatureId::new("ffffffff").unwrap()
    );
    let next = rebuilt.next.unwrap();
    assert_eq!(next.revision, Revision::OPENED);
    let on = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, Some(next), 10),
    )
    .result
    .unwrap();
    assert_eq!(on.total, rebuilt.total);
    let stale = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, Some(cursor), 10),
    )
    .result
    .unwrap_err();
    assert_eq!(
        codes(&stale),
        [(ErrorCode::InvalidInput, Some("after".to_owned()))]
    );
}
