//! break-it: `Refusal::BooleanPairsSkipped` as data and words (PR #171): how many pairs it
//! names and how it counts the rest, elements OCCT made (neither input's), the base's
//! elements wherever they sit in a pair, and refusals a misbehaving worker could forge (no
//! pairs, a huge index, a frame's worth of pairs). Every expected value is by hand, from the
//! refusal's documented words ("the base's Face2 with the tool's face 3", the first
//! `PAIRS_NAMED` pairs and a count of the rest) and English.

use std::sync::mpsc;
use std::time::Duration;

use silico_protocol::{BooleanOp, ElementKind, ElementRef, PAIRS_NAMED, Refusal, Role, Skipped};

fn base(kind: ElementKind, index: u32) -> Skipped {
    Skipped {
        kind,
        of: Some((Role::Base, index)),
    }
}

fn tool(kind: ElementKind, index: u32) -> Skipped {
    Skipped {
        kind,
        of: Some((Role::Tool, index)),
    }
}

fn made(kind: ElementKind) -> Skipped {
    Skipped { kind, of: None }
}

fn message(refusal: &Refusal) -> String {
    refusal.describe(None, &|e| e.to_string()).message
}

fn faces(n: u32) -> Refusal {
    Refusal::BooleanPairsSkipped {
        op: BooleanOp::Cut,
        pairs: (1..=n)
            .map(|i| [base(ElementKind::Face, i), tool(ElementKind::Face, i)])
            .collect(),
    }
}

#[test]
fn three_pairs_are_each_named_and_none_counted() {
    assert_eq!(PAIRS_NAMED, 3);
    assert_eq!(
        message(&faces(3)),
        "OCCT skipped intersecting the base's Face1 with the tool's face 1, the base's Face2 \
         with the tool's face 2, and the base's Face3 with the tool's face 3 in the cut, so its \
         result would take them as not meeting"
    );
}

#[test]
fn four_skipped_pairs_count_the_one_left_as_one_more_pair() {
    // BREAKS: the fourth pair is counted as "1 more pairs".
    let said = message(&faces(4));
    assert!(
        said.contains("the base's Face3 with the tool's face 3, and 1 more pair in the cut"),
        "{said}"
    );
}

#[test]
fn five_skipped_pairs_count_two_more_pairs() {
    let said = message(&faces(5));
    assert!(
        said.contains("the base's Face3 with the tool's face 3, and 2 more pairs in the cut"),
        "{said}"
    );
    // Every base face is an element, though only three pairs are named.
    assert_eq!(faces(5).elements().len(), 5);
    assert_eq!(faces(5).elements_total(), 5);
}

#[test]
fn elements_occt_made_are_worded_with_their_article_and_arent_elements() {
    let refusal = Refusal::BooleanPairsSkipped {
        op: BooleanOp::Fuse,
        pairs: vec![
            [made(ElementKind::Vertex), tool(ElementKind::Face, 7)],
            [made(ElementKind::Edge), made(ElementKind::Face)],
        ],
    };
    assert_eq!(
        message(&refusal),
        "OCCT skipped intersecting a vertex OCCT made of one with the tool's face 7 and an edge OCCT \
         made of one with a face OCCT made of one in the fuse, so its result would \
         take them as not meeting"
    );
    assert_eq!(refusal.elements(), Vec::<ElementRef>::new());
    assert_eq!(refusal.elements_total(), 0);
}

#[test]
fn the_bases_elements_are_named_once_each_wherever_they_sit_in_a_pair() {
    // The base's Face2 second in its first pair, first in its second; its Edge5 with an
    // element OCCT made; the tool's never.
    let refusal = Refusal::BooleanPairsSkipped {
        op: BooleanOp::Common,
        pairs: vec![
            [tool(ElementKind::Edge, 1), base(ElementKind::Face, 2)],
            [base(ElementKind::Face, 2), tool(ElementKind::Face, 3)],
            [base(ElementKind::Edge, 5), made(ElementKind::Vertex)],
            [tool(ElementKind::Face, 2), tool(ElementKind::Face, 9)],
        ],
    };
    assert_eq!(
        refusal.elements(),
        vec![ElementRef::face(2), ElementRef::edge(5)]
    );
    assert_eq!(refusal.elements_total(), 2);
    let said = message(&refusal);
    assert!(
        said.starts_with(
            "OCCT skipped intersecting the tool's edge 1 with the base's Face2, the base's \
             Face2 with the tool's face 3, the base's Edge5 with a vertex OCCT made of one, and \
             1 more pair"
        ),
        "{said}"
    );
}

#[test]
fn a_forged_refusal_of_the_largest_index_and_elements_occt_made_crosses_the_wire() {
    let refusal = Refusal::BooleanPairsSkipped {
        op: BooleanOp::Cut,
        pairs: vec![[base(ElementKind::Vertex, u32::MAX), made(ElementKind::Edge)]],
    };
    let bytes = postcard::to_allocvec(&refusal).unwrap();
    let back: Refusal = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, refusal);
    assert_eq!(
        back.elements(),
        vec![ElementRef {
            kind: ElementKind::Vertex,
            index: u32::MAX
        }]
    );
    assert!(message(&back).contains("the base's Vertex4294967295 with an edge OCCT made of one"));
}

#[test]
fn a_forged_refusal_of_no_pairs_still_says_what_it_skipped() {
    // BREAKS: a worker's refusal of no pairs reads "OCCT skipped intersecting  in the cut":
    // nothing where the pairs go. The kernel never sends one; a misbehaving worker can.
    let said = message(&Refusal::BooleanPairsSkipped {
        op: BooleanOp::Cut,
        pairs: Vec::new(),
    });
    assert!(!said.contains("intersecting  in"), "{said}");
}

#[test]
fn a_refusal_of_a_frames_worth_of_pairs_names_its_elements_in_linear_time() {
    // BREAKS: `elements` keeps each base element once by searching every one before it, so
    // a worker's refusal of 200,000 pairs of distinct base faces (about 2.4 MB, far under a
    // frame's 512 MiB) takes 2·10^10 comparisons, and the core asks for them twice (elements
    // and their total). Within 5 s, or it's a hang.
    let refusal = faces(200_000);
    let bytes = postcard::to_allocvec(&refusal).unwrap();
    assert!(bytes.len() < 4 * 1024 * 1024, "{}", bytes.len());
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let elements = refusal.elements();
        let _ = send.send(elements.len());
    });
    assert_eq!(receive.recv_timeout(Duration::from_secs(5)), Ok(200_000));
}
