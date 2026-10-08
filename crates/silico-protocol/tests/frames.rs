//! M0.6: every message survives the wire, and no byte stream, however broken, makes the
//! decoder panic or allocate what a frame's length claims before the bytes arrive.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use silico_protocol::{
    Blob, Failure, FailureKind, FrameError, FrameReader, MAX_FRAME_LEN, Mesh, Op, Reply, Request,
    Response, ShapeSummary, write_message,
};

/// One request per operation. The match gives each variant its own index and every
/// index must appear, so adding an `Op` without a case here fails: a missing arm doesn't
/// compile, and an arm without a case leaves its index out.
fn every_op() -> Vec<Op> {
    let ops = vec![
        Op::MakeBox {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        },
        Op::Summarize {
            shape: Blob::inline((0..=255).collect()),
        },
        Op::Fillet {
            shape: Blob::inline(vec![1, 2]),
            edges: vec![1, 3],
            radius: 2.5,
        },
        Op::Tessellate {
            shape: Blob::inline(vec![3]),
            deflection: 0.01,
        },
        Op::Echo {
            data: Blob::inline(vec![9; 3]),
        },
        Op::DebugSleep { millis: 60_000 },
        Op::DebugAbort,
        Op::DebugSegfault,
        Op::DebugGarbage,
        Op::DebugEnableFpTraps,
        Op::DebugReplyOutOfTurn { wrong_id: true },
    ];
    let indices: std::collections::BTreeSet<usize> = ops
        .iter()
        .map(|op| match op {
            Op::MakeBox { .. } => 0,
            Op::Summarize { .. } => 1,
            Op::Fillet { .. } => 2,
            Op::Tessellate { .. } => 3,
            Op::Echo { .. } => 4,
            Op::DebugSleep { .. } => 5,
            Op::DebugAbort => 6,
            Op::DebugSegfault => 7,
            Op::DebugGarbage => 8,
            Op::DebugEnableFpTraps => 9,
            Op::DebugReplyOutOfTurn { .. } => 10,
        })
        .collect();
    assert_eq!(indices, (0..11).collect(), "an operation has no case");
    ops
}

/// One response per reply and failure kind, checked the same way.
fn every_result() -> Vec<Result<Reply, Failure>> {
    let results = vec![
        Ok(Reply::Shape {
            brep: Blob::inline(vec![0, 1, 2, 255]),
        }),
        Ok(Reply::Summary(ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, -15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            valid: true,
        })),
        Ok(Reply::Echo {
            data: Blob::inline(vec![]),
        }),
        Ok(Reply::Mesh(Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edges: vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], vec![]],
        })),
        Ok(Reply::Done),
        Err(Failure {
            kind: FailureKind::InvalidInput,
            message: "box size dx must be a positive, finite length, got -1".to_owned(),
            element: None,
        }),
        Err(Failure {
            kind: FailureKind::KernelFailed,
            message: "Standard_DomainError".to_owned(),
            element: Some("Edge3".to_owned()),
        }),
        Err(Failure {
            kind: FailureKind::UnknownOp,
            message: String::new(),
            element: None,
        }),
        Err(Failure {
            kind: FailureKind::Internal,
            message: "floating-point traps were on".to_owned(),
            element: None,
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
            Err(Failure { kind, .. }) => match kind {
                FailureKind::InvalidInput => 5,
                FailureKind::KernelFailed => 6,
                FailureKind::UnknownOp => 7,
                FailureKind::Internal => 8,
            },
        })
        .collect();
    assert_eq!(
        indices,
        (0..9).collect(),
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
            id: i as u64,
            generation: u64::MAX - i as u64,
            trace: 1000 + i as u64,
            op,
        })
        .collect();
    let responses: Vec<Response> = every_result()
        .into_iter()
        .enumerate()
        .map(|(i, result)| Response {
            id: i as u64,
            generation: 7,
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
    // trace: 3, op: DebugAbort } is the varints 1, 2, and 3, then variant index 6. (Both ends ship
    // together, so indices may shift when operations are added.)
    let bytes = frame_of(&Request {
        id: 1,
        generation: 2,
        trace: 3,
        op: Op::DebugAbort,
    });
    assert_eq!(bytes, [4, 0, 0, 0, 1, 2, 3, 6]);
}

#[test]
fn truncated_frame_is_error() {
    let frame = frame_of(&Request {
        id: 7,
        generation: 1,
        trace: 0,
        op: Op::Summarize {
            shape: Blob::inline(vec![42; 100]),
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
        id: 1,
        generation: 1,
        trace: 0,
        op: Op::DebugGarbage,
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
        id: 1,
        generation: 1,
        trace: 0,
        op: Op::DebugAbort,
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
        id: 1,
        generation: 0,
        result: Err(Failure {
            kind: FailureKind::UnknownOp,
            message: "x".repeat(300),
            element: None,
        }),
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
    ) {
        for op in [
            Op::MakeBox { dx: dims[0], dy: dims[1], dz: dims[2] },
            Op::Summarize { shape: Blob::inline(blob.clone()) },
            Op::Fillet {
                shape: Blob::inline(blob.clone()),
                edges: blob.iter().map(|&b| u32::from(b) * 1_000_003).collect(),
                radius: dims[0],
            },
        ] {
            let request = Request { id, generation, trace: 0, op };
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
        id: 9,
        generation: 3,
        trace: 0,
        op: Op::Summarize {
            shape: Blob::inline(vec![7; 20]),
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
            "Summarize",
            "Fillet",
            "Tessellate",
            "Echo",
            "DebugSleep",
            "DebugAbort",
            "DebugSegfault",
            "DebugGarbage",
            "DebugEnableFpTraps",
            "DebugReplyOutOfTurn"
        ]
    );
}

#[test]
fn a_mesh_is_validated_before_use() {
    let good = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        face_triangles: vec![1],
        edges: vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]],
    };
    assert_eq!(good.validate(), Ok(()));
    let mut bad_index = good.clone();
    bad_index.triangles = vec![[0, 1, 3]];
    assert!(bad_index.validate().unwrap_err().contains("past the 3"));
    let mut bad_counts = good.clone();
    bad_counts.face_triangles = vec![2];
    assert!(bad_counts.validate().unwrap_err().contains("account for 2"));
    let mut not_finite = good.clone();
    not_finite.edges[0][1][2] = f64::NAN;
    assert!(not_finite.validate().unwrap_err().contains("finite"));
    let mut huge_counts = good;
    huge_counts.face_triangles = vec![u32::MAX, u32::MAX];
    assert!(huge_counts.validate().is_err());
}
