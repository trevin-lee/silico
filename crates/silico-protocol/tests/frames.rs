//! M0.6: every message survives the wire, and no byte stream, however broken, makes the
//! decoder panic or allocate what a frame's length claims before the bytes arrive.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolveStatus, Solved,
    SolverConstraint, Tag, Value,
};
use silico_protocol::{
    Axis, Blob, BooleanOp, BuiltData, ChamferSize, DebugOp, DressingKind, ElementCounts,
    ElementFate, ElementKind, ElementRef, Ends, Failure, FailureKind, FrameError, FrameReader,
    FreecadMeasures, Generation, History, InputHistory, MAX_FRAME_LEN, Mesh, Misbehavior, Op,
    Operation, OutOfTurn, Primitive, Profile, Refusal, Reply, Reported, Request, RequestId,
    Response, ShapeData, ShapeSummary, Topology, TraceId, write_message,
};

/// One request per operation. The match gives each variant its own index and every
/// index must appear, so adding an `Op` without a case here fails: a missing arm doesn't
/// compile, and an arm without a case leaves its index out.
fn every_op() -> Vec<Op> {
    let ops = vec![
        Op::MakePrimitive(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        }),
        Op::MakePrimitive(Primitive::Cylinder {
            radius: 5.0,
            height: 10.0,
        }),
        Op::MakePrimitive(Primitive::Sphere { radius: 5.0 }),
        Op::MakePrimitive(Primitive::Cone {
            radius1: 5.0,
            radius2: 0.0,
            height: 10.0,
        }),
        Op::MakePrimitive(Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        }),
        // Each state of a shape's validity: false, true, and unknown.
        Op::Summarize {
            shape: ShapeData::checked(Blob::inline((0..=255).collect()), false),
        },
        Op::Fillet {
            shape: ShapeData::checked(Blob::inline(vec![1, 2]), true),
            edges: vec![1, 3],
            radius: 2.5,
        },
        Op::Tessellate {
            shape: ShapeData::unchecked(Blob::inline(vec![3])),
            deflection: 0.01,
        },
        Op::Translate {
            shape: ShapeData::checked(Blob::inline(vec![4]), true),
            offset: [1.0, -2.0, 3.5],
        },
        Op::Boolean {
            op: BooleanOp::Fuse,
            base: ShapeData::checked(Blob::inline(vec![5]), true),
            tool: ShapeData::unchecked(Blob::inline(vec![6, 7])),
        },
        Op::Boolean {
            op: BooleanOp::Cut,
            base: ShapeData::unchecked(Blob::inline(vec![])),
            tool: ShapeData::checked(Blob::inline(vec![8]), false),
        },
        Op::Boolean {
            op: BooleanOp::Common,
            base: ShapeData::checked(Blob::inline(vec![9]), false),
            tool: ShapeData::checked(Blob::inline(vec![10]), true),
        },
        Op::Echo {
            data: Blob::inline(vec![9; 3]),
        },
        Op::Extrude {
            profile: Profile {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            },
            length: 2.0,
            direction: Some([0.0, 0.5, 1.0]),
        },
        Op::Revolve {
            profile: Profile {
                points: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 0.0, 1.0]],
            },
            axis: Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            angle: 90.0,
        },
        Op::Chamfer {
            shape: ShapeData::checked(Blob::inline(vec![11]), true),
            edges: vec![1, 3],
            size: ChamferSize::Equal { distance: 2.0 },
            flip: false,
        },
        Op::Chamfer {
            shape: ShapeData::unchecked(Blob::inline(vec![12])),
            edges: vec![2],
            size: ChamferSize::TwoDistances {
                distance: 1.0,
                other: 3.0,
            },
            flip: true,
        },
        Op::Chamfer {
            shape: ShapeData::checked(Blob::inline(vec![13]), false),
            edges: vec![],
            size: ChamferSize::DistanceAngle {
                distance: 1.0,
                angle: 30.0,
            },
            flip: false,
        },
        Op::MeasureAsFreecad {
            shape: ShapeData::unchecked(Blob::inline(vec![14])),
        },
        Op::Debug(DebugOp::Sleep { millis: 60_000 }),
        Op::Debug(DebugOp::Abort),
        Op::Debug(DebugOp::Segfault),
        Op::Debug(DebugOp::Garbage),
        Op::Debug(DebugOp::EnableFpTraps),
        Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongId)),
        Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongGeneration)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::BadMesh)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::WrongReply)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::BadHistory)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::HistoryForNoInputs)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::ForgedRefusal)),
        Op::Solve(SketchSystem {
            values: vec![0.0, 0.5, 10.0, -0.25, 10.0],
            unknowns: vec![Value(0), Value(1), Value(2), Value(3)],
            geometry: vec![SketchGeometry::Line {
                start: PointValues {
                    x: Value(0),
                    y: Value(1),
                },
                end: PointValues {
                    x: Value(2),
                    y: Value(3),
                },
            }],
            constraints: vec![
                SolverConstraint::Horizontal {
                    tag: Tag(1),
                    line: Geometry(0),
                },
                SolverConstraint::DistanceX {
                    tag: Tag(2),
                    a: PointOf::Start(Geometry(0)),
                    b: PointOf::End(Geometry(0)),
                    value: Value(4),
                },
            ],
            algorithms: vec![Algorithm::LevenbergMarquardt, Algorithm::DogLeg],
        }),
    ];
    let indices: std::collections::BTreeSet<usize> = ops
        .iter()
        .map(|op| match op {
            Op::MakePrimitive(Primitive::Box { .. }) => 0,
            Op::MakePrimitive(Primitive::Cylinder { .. }) => 1,
            Op::MakePrimitive(Primitive::Sphere { .. }) => 2,
            Op::MakePrimitive(Primitive::Cone { .. }) => 3,
            Op::MakePrimitive(Primitive::Torus { .. }) => 4,
            Op::Summarize { .. } => 5,
            Op::Fillet { .. } => 6,
            Op::Tessellate { .. } => 7,
            Op::Echo { .. } => 8,
            Op::Debug(DebugOp::Sleep { .. }) => 9,
            Op::Debug(DebugOp::Abort) => 10,
            Op::Debug(DebugOp::Segfault) => 11,
            Op::Debug(DebugOp::Garbage) => 12,
            Op::Debug(DebugOp::EnableFpTraps) => 13,
            Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongId)) => 14,
            Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongGeneration)) => 15,
            Op::Debug(DebugOp::Misbehave(Misbehavior::BadMesh)) => 16,
            Op::Debug(DebugOp::Misbehave(Misbehavior::WrongReply)) => 17,
            Op::Debug(DebugOp::Misbehave(Misbehavior::BadHistory)) => 18,
            Op::Debug(DebugOp::Misbehave(Misbehavior::HistoryForNoInputs)) => 19,
            Op::Translate { .. } => 20,
            Op::Boolean {
                op: BooleanOp::Fuse,
                ..
            } => 21,
            Op::Boolean {
                op: BooleanOp::Cut, ..
            } => 22,
            Op::Boolean {
                op: BooleanOp::Common,
                ..
            } => 23,
            Op::Extrude { .. } => 24,
            Op::Revolve { .. } => 25,
            Op::Chamfer {
                size: ChamferSize::Equal { .. },
                ..
            } => 26,
            Op::Chamfer {
                size: ChamferSize::TwoDistances { .. },
                ..
            } => 27,
            Op::Chamfer {
                size: ChamferSize::DistanceAngle { .. },
                ..
            } => 28,
            Op::MeasureAsFreecad { .. } => 29,
            Op::Debug(DebugOp::Misbehave(Misbehavior::ForgedRefusal)) => 30,
            Op::Solve(_) => 31,
        })
        .collect();
    assert_eq!(indices, (0..32).collect(), "an operation has no case");
    ops
}

/// One response per reply and failure kind, checked the same way.
fn every_result() -> Vec<Result<Reply, Failure>> {
    let results = vec![
        Ok(Reply::Built(BuiltData {
            shape: ShapeData::checked(Blob::inline(vec![9, 8]), true),
            history: History {
                inputs: vec![InputHistory {
                    faces: vec![
                        ElementFate {
                            kept: Some(2),
                            ..ElementFate::default()
                        },
                        ElementFate {
                            kept: None,
                            modified: vec![Reported {
                                element: ElementRef {
                                    kind: ElementKind::Face,
                                    index: 1,
                                },
                                position: 1,
                            }],
                            generated: vec![Reported {
                                element: ElementRef {
                                    kind: ElementKind::Edge,
                                    index: 3,
                                },
                                position: 2,
                            }],
                        },
                    ],
                    edges: vec![ElementFate::default()],
                    vertices: vec![],
                }],
                result: ElementCounts {
                    faces: 2,
                    edges: 3,
                    vertices: 0,
                    solids: 1,
                },
                ends: Ends {
                    start: vec![1],
                    end: vec![2, 3],
                },
                topology: Topology {
                    face_edges: vec![vec![1, 3], vec![2, 3]],
                    outer_edges: vec![vec![1, 3], vec![]],
                    edge_vertices: vec![vec![], vec![], vec![]],
                },
            },
        })),
        Ok(Reply::Shape(ShapeData::checked(
            Blob::inline(vec![0, 1, 2, 255]),
            false,
        ))),
        Ok(Reply::Summary(ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, -15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0, 0.0, -30.0], [10.0, 20.0, 0.0]]),
            valid: true,
        })),
        Ok(Reply::FreecadMeasures(FreecadMeasures {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, -15.0],
            bounds: Some([[0.0, 0.0, -30.0], [10.0, 20.0, 0.0]]),
        })),
        Ok(Reply::FreecadMeasures(FreecadMeasures {
            volume: 0.0,
            area: 0.0,
            center: [0.0; 3],
            bounds: None,
        })),
        Ok(Reply::Echo {
            data: Blob::inline(vec![]),
        }),
        Ok(Reply::Mesh(Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edge_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            edge_point_counts: vec![2, 0],
            valid: None,
        })),
        Ok(Reply::Done),
        Ok(Reply::Solved(Solved {
            dofs: -1,
            conflicting: vec![Tag(2), Tag(5)],
            redundant: vec![Tag(3)],
            partially_redundant: vec![],
            free: vec![Value(0), Value(7)],
            no_equations: false,
            status: SolveStatus::Converged,
            algorithm: None,
            values: vec![0.0, -1.5, 1e-300, f64::MAX],
            residuals: vec![
                (Tag(2), Some(-2.5e-11)),
                (Tag(3), None),
                (Tag(5), Some(0.0)),
            ],
        })),
        Err(Failure::new(Refusal::InvalidParameter {
            parameter: "dx".to_owned(),
            message: "box dx must be a finite length above 1e-7, got -1".to_owned(),
        })),
        Err(Failure {
            refusal: Refusal::Occt {
                operation: Operation::Dressing {
                    dressing: DressingKind::Fillet,
                },
                edges: vec![3],
            },
            cause: Some("Standard_DomainError".to_owned()),
        }),
        Err(Failure::new(Refusal::ResultInvalid {
            operation: Operation::Dressing {
                dressing: DressingKind::Fillet,
            },
            edges: vec![1, 2],
        })),
        Err(Failure::new(Refusal::TestOperation { op: String::new() })),
        Err(Failure::new(Refusal::FpTrapsOn {
            mask: 0x200,
            op: "Fillet".to_owned(),
        })),
        Err(Failure {
            refusal: Refusal::OutOfMemory,
            cause: Some("Standard_OutOfMemory".to_owned()),
        }),
    ];
    let indices: std::collections::BTreeSet<usize> = results
        .iter()
        .map(|result| match result {
            Ok(Reply::Shape { .. }) => 0,
            Ok(Reply::Summary(_)) => 1,
            Ok(Reply::Mesh(_)) => 2,
            Ok(Reply::Echo { .. }) => 3,
            Ok(Reply::Done) => 4,
            Ok(Reply::Built(_)) => 10,
            Ok(Reply::FreecadMeasures(_)) => 11,
            Ok(Reply::Solved(_)) => 13,
            Err(failure) => match failure.kind() {
                FailureKind::InvalidInput => 5,
                FailureKind::KernelFailed => 6,
                FailureKind::InvalidResult => 7,
                FailureKind::UnknownOp => 8,
                FailureKind::Internal => 9,
                FailureKind::ResourceExhausted => 12,
            },
        })
        .collect();
    assert_eq!(
        indices,
        (0..14).collect(),
        "a reply or failure kind has no case"
    );
    results
}

fn frame_of<T: serde::Serialize>(message: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_message(&mut bytes, message).unwrap();
    bytes
}

#[test]
fn roundtrip_every_op() {
    // Every message, back to back on one stream, then a clean end.
    let requests: Vec<Request> = every_op()
        .into_iter()
        .enumerate()
        .map(|(i, op)| Request {
            id: RequestId(i as u64),
            generation: Generation(u64::MAX - i as u64),
            trace: TraceId(1000 + i as u64),
            op,
        })
        .collect();
    let responses: Vec<Response> = every_result()
        .into_iter()
        .enumerate()
        .map(|(i, result)| Response {
            id: RequestId(i as u64),
            generation: Generation(7),
            result,
        })
        .collect();

    let mut stream = Vec::new();
    for r in &requests {
        write_message(&mut stream, r).unwrap();
    }
    let mut reader = FrameReader::new(stream.as_slice());
    for r in &requests {
        assert_eq!(reader.read_message::<Request>().unwrap().as_ref(), Some(r));
    }
    assert!(reader.read_message::<Request>().unwrap().is_none());

    let mut stream = Vec::new();
    for r in &responses {
        write_message(&mut stream, r).unwrap();
    }
    let mut reader = FrameReader::new(stream.as_slice());
    for r in &responses {
        assert_eq!(reader.read_message::<Response>().unwrap().as_ref(), Some(r));
    }
    assert!(reader.read_message::<Response>().unwrap().is_none());
}

#[test]
fn frame_layout_is_length_then_payload() {
    // A 4-byte little-endian length, then postcard: Request { id: 1, generation: 2,
    // trace: 3, op: Debug(Abort) } is the varints 1, 2, and 3, then Op's variant index 11
    // (Debug) and DebugOp's 1 (Abort). (Both ends ship together, so indices may shift when
    // operations are added.)
    let bytes = frame_of(&Request {
        id: RequestId(1),
        generation: Generation(2),
        trace: TraceId(3),
        op: Op::Debug(DebugOp::Abort),
    });
    assert_eq!(bytes, [5, 0, 0, 0, 1, 2, 3, 11, 1]);
}

#[test]
fn truncated_frame_is_error() {
    let frame = frame_of(&Request {
        id: RequestId(7),
        generation: Generation(1),
        trace: TraceId(0),
        op: Op::Summarize {
            shape: ShapeData::unchecked(Blob::inline(vec![42; 100])),
        },
    });
    assert!(
        FrameReader::new(&frame[..0])
            .read_frame()
            .unwrap()
            .is_none()
    );
    for cut in 1..frame.len() {
        match FrameReader::new(&frame[..cut]).read_message::<Request>() {
            Err(FrameError::Truncated { expected, got }) => {
                assert!(got < expected, "cut {cut}: got {got} of {expected}");
            }
            other => panic!("cut at {cut} of {}: {other:?}", frame.len()),
        }
    }
}

#[test]
fn oversized_frame_rejected() {
    // Only a header, claiming more than the maximum: rejected as too large from the
    // header alone. (tests/allocation.rs proves nothing is allocated for it.)
    for len in [MAX_FRAME_LEN + 1, u32::MAX] {
        match FrameReader::new(&len.to_le_bytes()[..]).read_frame() {
            Err(FrameError::TooLarge { len: got, max }) => {
                assert_eq!((got, max), (u64::from(len), MAX_FRAME_LEN));
            }
            other => panic!("{len}: {other:?}"),
        }
    }
    // A reader's own lower maximum applies too, and the maximum itself is allowed.
    let frame = frame_of(&Request {
        id: RequestId(1),
        generation: Generation(1),
        trace: TraceId(0),
        op: Op::Debug(DebugOp::Garbage),
    });
    let payload = frame.len() as u32 - 4;
    assert!(matches!(
        FrameReader::with_max_len(frame.as_slice(), payload - 1).read_frame(),
        Err(FrameError::TooLarge { .. })
    ));
    assert!(
        FrameReader::with_max_len(frame.as_slice(), payload)
            .read_frame()
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_lying_length_is_truncation() {
    // The header claims the maximum but the stream holds 10 bytes. (tests/allocation.rs
    // proves the reader doesn't allocate the claimed size first.)
    let mut stream = MAX_FRAME_LEN.to_le_bytes().to_vec();
    stream.extend([0u8; 10]);
    match FrameReader::new(stream.as_slice()).read_frame() {
        Err(FrameError::Truncated { expected, got }) => {
            assert_eq!((expected, got), (u64::from(MAX_FRAME_LEN) + 4, 14));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn trailing_bytes_are_an_error() {
    let mut frame = frame_of(&Request {
        id: RequestId(1),
        generation: Generation(1),
        trace: TraceId(0),
        op: Op::Debug(DebugOp::Abort),
    });
    frame.push(0);
    frame[0] += 1; // the length now covers the extra byte
    assert!(matches!(
        FrameReader::new(frame.as_slice()).read_message::<Request>(),
        Err(FrameError::TrailingBytes { extra: 1 })
    ));
}

#[test]
fn a_frame_of_the_wrong_message_is_a_decode_error() {
    // A response read where a request is expected doesn't silently decode.
    let frame = frame_of(&Response {
        id: RequestId(1),
        generation: Generation(0),
        result: Err(Failure::new(Refusal::Internal {
            what: "x".repeat(300),
        })),
    });
    assert!(matches!(
        FrameReader::new(frame.as_slice()).read_message::<Request>(),
        Err(FrameError::Decode(_) | FrameError::TrailingBytes { .. })
    ));
}

/// Reads messages until the stream ends or errors. Never panics, which is the property.
fn drain<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> usize {
    let mut reader = FrameReader::with_max_len(bytes, 1 << 16);
    let mut read = 0;
    while let Ok(Some(_)) = reader.read_message::<T>() {
        read += 1;
    }
    read
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// Random streams, with each one's first length kept under the reader's maximum so
    /// most cases get past the header (an unclamped random length almost never would).
    #[test]
    fn decoder_never_panics(mut bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        if let Some(header) = bytes.get_mut(..4) {
            let len = u32::from_le_bytes(header.try_into().unwrap()) % ((1 << 16) + 1);
            header.copy_from_slice(&len.to_le_bytes());
        }
        drain::<Request>(&bytes);
        drain::<Response>(&bytes);
    }

    /// Arbitrary bytes behind a valid header reach the message decoder itself.
    #[test]
    fn message_decoder_never_panics(payload in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
        bytes.extend(&payload);
        drain::<Request>(&bytes);
        drain::<Response>(&bytes);
    }

    #[test]
    fn roundtrip_arbitrary_requests(
        id in any::<u64>(),
        generation in any::<u64>(),
        dims in proptest::array::uniform3(any::<f64>()),
        blob in proptest::collection::vec(any::<u8>(), 0..2048),
        valid in proptest::option::of(any::<bool>()),
    ) {
        let shape = || match valid {
            Some(valid) => ShapeData::checked(Blob::inline(blob.clone()), valid),
            None => ShapeData::unchecked(Blob::inline(blob.clone())),
        };
        for op in [
            Op::MakePrimitive(Primitive::Box { dx: dims[0], dy: dims[1], dz: dims[2] }),
            Op::Summarize { shape: shape() },
            Op::MeasureAsFreecad { shape: shape() },
            Op::Fillet {
                shape: shape(),
                edges: blob.iter().map(|&b| u32::from(b) * 1_000_003).collect(),
                radius: dims[0],
            },
            Op::Translate { shape: shape(), offset: dims },
            Op::Extrude {
                profile: Profile { points: vec![dims; blob.len() % 5] },
                length: dims[1],
                direction: valid.map(|_| dims),
            },
            Op::Revolve {
                profile: Profile { points: vec![dims; 3] },
                axis: Axis { origin: dims, direction: dims },
                angle: dims[2],
            },
            Op::Boolean { op: BooleanOp::Cut, base: shape(), tool: shape() },
        ] {
            let request = Request {
                id: RequestId(id),
                generation: Generation(generation),
                trace: TraceId(0),
                op,
            };
            let frame = frame_of(&request);
            let back = FrameReader::new(frame.as_slice()).read_message::<Request>().unwrap().unwrap();
            // Compare bit patterns, so NaN round-trips count as equal.
            prop_assert_eq!(frame_of(&back), frame);
        }
    }
}

#[test]
fn max_frame_len_is_512_mib() {
    assert_eq!(MAX_FRAME_LEN, 512 * 1024 * 1024);
}

#[test]
fn blob_gives_back_its_bytes_and_prints_its_length() {
    let blob = Blob::inline(vec![1, 2, 3]);
    assert_eq!(blob.as_bytes(), [1, 2, 3]);
    assert_eq!(format!("{blob:?}"), "Blob::Inline(3 bytes)");
}

/// A reader that's interrupted (EINTR) before every chunk it hands out.
struct Interrupting<'a> {
    data: &'a [u8],
    interrupt_next: bool,
}

impl std::io::Read for Interrupting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::mem::take(&mut self.interrupt_next) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        self.interrupt_next = true;
        let n = buf.len().min(self.data.len()).min(1);
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

#[test]
fn interrupted_reads_are_retried() {
    let request = Request {
        id: RequestId(9),
        generation: Generation(3),
        trace: TraceId(0),
        op: Op::Summarize {
            shape: ShapeData::unchecked(Blob::inline(vec![7; 20])),
        },
    };
    let frame = frame_of(&request);
    let mut reader = FrameReader::new(Interrupting {
        data: &frame,
        interrupt_next: true,
    });
    assert_eq!(reader.read_message::<Request>().unwrap(), Some(request));
    assert!(reader.read_message::<Request>().unwrap().is_none());
}

/// A reader that fails.
struct Failing(std::io::ErrorKind);

impl std::io::Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(self.0.into())
    }
}

#[test]
fn other_read_errors_are_io_errors() {
    match FrameReader::new(Failing(std::io::ErrorKind::ConnectionReset)).read_frame() {
        Err(FrameError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset),
        other => panic!("{other:?}"),
    }
}

#[test]
fn frame_errors_explain_themselves() {
    use std::error::Error as _;
    let io = FrameError::Io(std::io::ErrorKind::BrokenPipe.into());
    assert!(io.to_string().starts_with("frame I/O failed: "), "{io}");
    assert!(io.source().is_some());
    let truncated = FrameError::Truncated {
        expected: 10,
        got: 4,
    };
    assert_eq!(
        truncated.to_string(),
        "stream ended partway through a frame (4 of 10 bytes)"
    );
    assert!(truncated.source().is_none());
    assert_eq!(
        FrameError::TooLarge { len: 9, max: 8 }.to_string(),
        "frame of 9 bytes is over the 8-byte maximum"
    );
    assert_eq!(
        FrameError::TrailingBytes { extra: 2 }.to_string(),
        "frame has 2 bytes after the message"
    );
    let decode = FrameReader::new(&[1u8, 0, 0, 0, 0xff][..])
        .read_message::<Request>()
        .unwrap_err();
    assert!(
        decode
            .to_string()
            .starts_with("frame isn't a valid message: "),
        "{decode}"
    );
    assert!(decode.source().is_some());
}

#[test]
fn every_op_has_its_own_name() {
    let names: Vec<&str> = every_op().iter().map(Op::name).collect();
    assert_eq!(
        names,
        [
            "MakeBox",
            "MakeCylinder",
            "MakeSphere",
            "MakeCone",
            "MakeTorus",
            "Summarize",
            "Fillet",
            "Tessellate",
            "Translate",
            "Fuse",
            "Cut",
            "Common",
            "Echo",
            "Extrude",
            "Revolve",
            "Chamfer",
            "Chamfer",
            "Chamfer",
            "MeasureAsFreecad",
            "DebugSleep",
            "DebugAbort",
            "DebugSegfault",
            "DebugGarbage",
            "DebugEnableFpTraps",
            "DebugReplyWithWrongId",
            "DebugReplyWithWrongGeneration",
            "DebugMisbehaveBadMesh",
            "DebugMisbehaveWrongReply",
            "DebugMisbehaveBadHistory",
            "DebugMisbehaveHistoryForNoInputs",
            "DebugMisbehaveForgedRefusal",
            "Solve"
        ]
    );
}

#[test]
fn each_triangle_belongs_to_the_face_its_counts_say() {
    let mesh = |triangles: usize, face_triangles: Vec<u32>| Mesh {
        positions: vec![[0.0; 3]],
        normals: vec![[0.0, 0.0, 1.0]],
        triangles: vec![[0, 0, 0]; triangles],
        face_triangles,
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: None,
    };
    // Two triangles on face 0, none on face 1, three on face 2.
    assert_eq!(mesh(5, vec![2, 0, 3]).triangle_faces(), [0, 0, 2, 2, 2]);
    assert_eq!(mesh(0, vec![]).triangle_faces(), Vec::<u32>::new());
    // Counts that don't add up (a mesh that fails `validate`): no more faces than
    // triangles, and triangles left over get none.
    assert_eq!(mesh(3, vec![2, u32::MAX]).triangle_faces(), [0, 0, 1]);
    assert_eq!(mesh(4, vec![1, 1]).triangle_faces(), [0, 1]);
}

#[test]
fn a_mesh_is_validated_before_use() {
    let good = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        edge_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
        edge_point_counts: vec![2, 0, 1],
        valid: None,
    };
    assert_eq!(
        good.edges().collect::<Vec<_>>(),
        [
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]][..],
            &[],
            &[[1.0, 1.0, 0.0]]
        ]
    );
    assert_eq!(good.validate(), Ok(()));
    let mut bad_index = good.clone();
    bad_index.triangles = vec![[0, 1, 3]];
    assert!(bad_index.validate().unwrap_err().contains("past the 3"));
    let mut bad_counts = good.clone();
    bad_counts.face_triangles = vec![2];
    assert!(bad_counts.validate().unwrap_err().contains("account for 2"));
    // A face with no triangles comes with the shape's validity, and only then.
    let mut unmeshed = good.clone();
    unmeshed.face_triangles = vec![1, 0];
    assert!(unmeshed.validate().unwrap_err().contains("wasn't checked"));
    for valid in [true, false] {
        unmeshed.valid = Some(valid);
        assert_eq!(unmeshed.validate(), Ok(()), "{valid}");
        assert_eq!(unmeshed.unmeshed_faces(), [2]);
    }
    let mut checked = good.clone();
    checked.valid = Some(true);
    assert!(
        checked
            .validate()
            .unwrap_err()
            .contains("every face has triangles")
    );
    assert!(good.unmeshed_faces().is_empty());
    let mut not_finite = good.clone();
    not_finite.edge_points[1][2] = f64::NAN;
    assert!(not_finite.validate().unwrap_err().contains("finite"));
    let mut normal_not_finite = good.clone();
    normal_not_finite.normals[2][0] = f32::INFINITY;
    assert!(
        normal_not_finite
            .validate()
            .unwrap_err()
            .contains("a normal isn't finite")
    );
    // A unit vector, give or take the 1e-6 allowed, and no more.
    for length in [1.0 + 2e-6, 1.0 - 2e-6, 1.01, 0.9, 1.5] {
        let mut off = good.clone();
        off.normals[1] = [length, 0.0, 0.0];
        assert!(
            off.validate().unwrap_err().contains("isn't a unit vector"),
            "length {length}"
        );
    }
    for length in [1.0 + 5e-7, 1.0 - 5e-7] {
        let mut close = good.clone();
        close.normals[1] = [length, 0.0, 0.0];
        assert_eq!(close.validate(), Ok(()), "length {length}");
    }
    for normals in [vec![], vec![[0.0, 0.0, 1.0]; 2], vec![[0.0, 0.0, 1.0]; 4]] {
        let mut bad_normals = good.clone();
        bad_normals.normals = normals;
        assert!(
            bad_normals
                .validate()
                .unwrap_err()
                .contains("normals for 3 vertices"),
            "{:?}",
            bad_normals.normals
        );
    }
    for counts in [vec![2, 0], vec![2, 0, 2], vec![u32::MAX, u32::MAX, 3]] {
        let mut bad_edges = good.clone();
        bad_edges.edge_point_counts = counts;
        assert!(
            bad_edges
                .validate()
                .unwrap_err()
                .contains("edges account for"),
            "{:?}",
            bad_edges.edge_point_counts
        );
        // Unvalidated, the edges still can't index past the points.
        assert!(bad_edges.edges().flatten().count() <= 3);
    }
    let mut huge_counts = good;
    huge_counts.face_triangles = vec![u32::MAX, u32::MAX];
    assert!(huge_counts.validate().is_err());
}

/// A mesh with these faces' triangle counts (no triangles drawn: only the counts matter here).
fn counted(face_triangles: Vec<u32>, valid: Option<bool>) -> Mesh {
    Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles,
        edge_points: vec![],
        edge_point_counts: vec![],
        valid,
    }
}

#[test]
fn only_the_faces_without_triangles_are_named_and_counted() {
    // Faces 2 and 4 have no triangles; 1 and 3 do.
    assert_eq!(
        counted(vec![2, 0, 1, 0], Some(false)).refusal(),
        Some(Refusal::FacesUnmeshed {
            faces: vec![2, 4],
            count: 2
        })
    );
    assert_eq!(
        counted(vec![2, 0, 1, 0], Some(false)).unmeshed_faces(),
        [2, 4]
    );
    // A valid shape's are left out, and a mesh of every face refuses nothing.
    assert_eq!(counted(vec![2, 0, 1, 0], Some(true)).refusal(), None);
    assert_eq!(counted(vec![2, 1], None).refusal(), None);
}

#[test]
fn a_refusal_of_many_unmeshed_faces_names_the_first_hundred_in_order() {
    // 500 faces, every other one empty: faces 2, 4, ..., 1000 are unmeshed, 500 of them.
    let counts = (0..1000).map(|i| if i % 2 == 0 { 3 } else { 0 }).collect();
    let refusal = counted(counts, Some(false)).refusal().unwrap();
    let Refusal::FacesUnmeshed { faces, count } = &refusal else {
        panic!("{refusal:?}")
    };
    assert_eq!(*faces, (1..=100).map(|i| 2 * i).collect::<Vec<u32>>());
    assert_eq!(*count, 500);
    assert_eq!(refusal.elements_total(), 500);
    assert_eq!(refusal.elements().len(), 100);
    assert_eq!(refusal.elements()[99], ElementRef::face(200));
}

#[test]
fn every_failure_counts_the_elements_it_names() {
    let refusal = Refusal::EdgeTwice { edge: 4 };
    assert_eq!(refusal.elements_total(), 1);
    assert_eq!(Refusal::NoShape.elements_total(), 0);
    let few = Refusal::FacesUnmeshed {
        faces: vec![1, 9],
        count: 2,
    };
    assert_eq!(few.elements_total(), 2);
}
