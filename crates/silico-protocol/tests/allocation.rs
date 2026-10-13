//! A frame's length is never trusted: reading allocates only for bytes that actually
//! arrive. Measured with a counting allocator, in its own test binary so it counts nothing
//! else. The allocator counts every thread, so each test needs a process of its own, as
//! nextest (`pixi run test`) gives it; under `cargo test` they share one and count each
//! other's. (On macOS a large allocation is lazy, so time or memory alone can't show this.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::System;

use silico_protocol::{
    FrameError, FrameReader, Generation, MAX_FRAME_LEN, Mesh, Reply, RequestId, Response,
    write_message,
};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// Bytes allocated (growth by reallocation included) while reading one frame from `stream`.
fn bytes_allocated_reading(stream: &[u8]) -> (Result<Option<Vec<u8>>, FrameError>, usize) {
    let region = Region::new(GLOBAL);
    let result = FrameReader::new(stream).read_frame();
    (result, region.change().bytes_allocated)
}

#[test]
fn a_lying_length_allocates_only_what_arrives() {
    // The header claims the maximum (512 MiB); 10 bytes follow.
    let mut stream = MAX_FRAME_LEN.to_le_bytes().to_vec();
    stream.extend([0u8; 10]);
    let (result, allocated) = bytes_allocated_reading(&stream);
    assert!(
        matches!(result, Err(FrameError::Truncated { .. })),
        "{result:?}"
    );
    assert!(
        allocated < 64 * 1024,
        "allocated {allocated} bytes for 10 that arrived"
    );
}

#[test]
fn an_oversized_length_allocates_nothing_for_the_payload() {
    let (result, allocated) = bytes_allocated_reading(&u32::MAX.to_le_bytes());
    assert!(
        matches!(result, Err(FrameError::TooLarge { .. })),
        "{result:?}"
    );
    assert!(allocated < 1024, "allocated {allocated} bytes");
}

#[test]
fn a_real_frame_allocates_about_its_size() {
    // The bound above isn't vacuous: reading a 1 MiB frame allocates at least 1 MiB.
    let len = 1024 * 1024u32;
    let mut stream = len.to_le_bytes().to_vec();
    stream.extend(vec![0u8; len as usize]);
    let (result, allocated) = bytes_allocated_reading(&stream);
    assert_eq!(result.unwrap().unwrap().len(), len as usize);
    assert!(allocated >= len as usize, "allocated {allocated} bytes");
}

/// The memory a decoded `response` holds (allocated while decoding and not freed), and
/// the size of its frame.
fn bytes_held_after_decoding(response: &Response) -> (usize, usize) {
    let mut frame = Vec::new();
    write_message(&mut frame, response).unwrap();
    let region = Region::new(GLOBAL);
    let decoded: Response = FrameReader::new(frame.as_slice())
        .read_message()
        .unwrap()
        .unwrap();
    let change = region.change();
    drop(decoded);
    // Growth by reallocation is already in `bytes_allocated`.
    (
        change.bytes_allocated - change.bytes_deallocated,
        frame.len(),
    )
}

#[test]
fn a_reply_costs_at_most_a_few_times_its_size_to_read() {
    // A worker's reply can't make the core hold much more memory than the reply's size.
    // The worst case is a number: one byte on the wire, four in memory, in a list that may
    // have room for up to twice what it holds. So no more than 8 times. (A list of lists
    // was 24 times: one byte for an empty list on the wire, 24 in memory.)
    let n = 1 << 20;
    let mesh = |f: &dyn Fn(&mut Mesh)| {
        let mut mesh = Mesh {
            positions: vec![],
            normals: vec![],
            triangles: vec![],
            face_triangles: vec![],
            edge_points: vec![],
            edge_point_counts: vec![],
            valid: None,
        };
        f(&mut mesh);
        Response {
            id: RequestId(0),
            generation: Generation(0),
            result: Ok(Reply::Mesh(mesh)),
        }
    };
    let cases = [
        ("empty edges", mesh(&|m| m.edge_point_counts = vec![0; n])),
        ("empty faces", mesh(&|m| m.face_triangles = vec![0; n])),
        ("triangles", mesh(&|m| m.triangles = vec![[0; 3]; n])),
        ("positions", mesh(&|m| m.positions = vec![[0.0; 3]; n])),
        ("normals", mesh(&|m| m.normals = vec![[0.0; 3]; n])),
    ];
    for (what, response) in cases {
        let (held, wire) = bytes_held_after_decoding(&response);
        assert!(
            held <= 8 * wire,
            "{what}: {wire} bytes on the wire, {held} held after reading"
        );
    }
}

#[test]
fn refusing_a_mesh_of_unmeshed_faces_costs_a_few_times_its_size() {
    // A worker's mesh of a million faces, none meshed, on a shape it says isn't valid:
    // reading it and deciding, as the core does (the refusal, its words, its elements),
    // costs no more than reading any reply does, however many faces it has.
    let mesh = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![0; 1_000_000],
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: Some(false),
    };
    let wire = postcard::to_allocvec(&Reply::Mesh(mesh)).unwrap();
    let region = Region::new(GLOBAL);
    let Reply::Mesh(mesh) = silico_protocol::decode::<Reply>(&wire).unwrap() else {
        panic!("not a mesh")
    };
    assert_eq!(mesh.validate(), Ok(()));
    let read = region.change().bytes_allocated;
    let deciding = Region::new(GLOBAL);
    let refusal = mesh.refusal().unwrap();
    let name = |e: silico_protocol::ElementRef| e.to_string();
    let said = refusal.describe(None, &name);
    let elements: Vec<String> = refusal.elements().into_iter().map(name).collect();
    let decided = deciding.change().bytes_allocated;
    let spent = region.change().bytes_allocated;
    assert!(
        spent <= 8 * wire.len(),
        "{spent} bytes for a {}-byte reply",
        wire.len()
    );
    // Deciding costs what naming a hundred faces does, nothing like the million read.
    assert!(
        decided <= 16 * 1024,
        "{decided} bytes to decide, after {read} to read"
    );
    assert_eq!(elements.len(), silico_protocol::FACES_NAMED);
    assert!(
        said.message
            .ends_with("and 999992 other faces couldn't be meshed")
    );
    drop((mesh, said, elements));
}
