//! M5.9, hostile face frames for the fake kernel (`/break-it`): frames whose numbers are at
//! the extremes the shape's bytes must carry (negative zeros, subnormals, a kilometre),
//! frame lines that aren't what the fake writes, a frame the protocol refuses, and the
//! fake's measures of faces on a tilted frame against their closed forms (the box of a
//! tilted circle and rectangle, a pad's volume).

use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Blob, Curve, FaceRegions, Kernel, KernelRequest, Region, ShapeData, SketchPlane,
};
use silico_core::pool::Outcome;
use silico_protocol::FaceFrame;

fn run<K: Kernel, T: Send + std::fmt::Debug + 'static>(kernel: &K, request: KernelRequest<T>) -> T {
    match kernel.submit(kernel.generation(), request).wait() {
        Outcome::Done(result) => result.unwrap(),
        Outcome::Superseded => panic!("superseded"),
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The XY frame tilted 30° about X, from `origin`.
fn tilted(origin: [f64; 3]) -> FaceFrame {
    let (c, s) = (libm::cos(30f64.to_radians()), libm::sin(30f64.to_radians()));
    FaceFrame {
        origin,
        x: [1.0, 0.0, 0.0],
        y: [0.0, c, s],
        normal: [0.0, -s, c],
    }
}

fn on(frame: FaceFrame, outer: Vec<Curve>) -> FaceRegions {
    FaceRegions {
        plane: SketchPlane::Face(frame),
        regions: vec![Region {
            outer,
            holes: vec![],
        }],
    }
}

fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    (0..4)
        .map(|i| Curve::Line {
            start: p[i],
            end: p[(i + 1) % 4],
        })
        .collect()
}

fn circle(r: f64) -> Vec<Curve> {
    vec![Curve::Circle {
        center: [0.0, 0.0],
        radius: r,
    }]
}

fn text(shape: &ShapeData) -> String {
    String::from_utf8(shape.brep().as_bytes().to_vec()).unwrap()
}

fn shape(text: &str) -> ShapeData {
    ShapeData::checked(Blob::inline(text.as_bytes().to_vec()), true)
}

#[test]
fn a_frame_of_extreme_numbers_is_written_and_read_back_as_the_same_shape() {
    // Subnormals, a kilometre, and negative zeros in every part of the frame: the bytes
    // read back (the fake describes them), and a frame of −0s writes the bytes of +0s.
    let kernel = FakeKernel::new(1).unwrap();
    let extreme = FaceFrame {
        origin: [-0.0, 1e6, 5e-324],
        x: [1.0, -0.0, 5e-324],
        y: [-0.0, 1.0, 1e-300],
        normal: [-5e-324, -1e-300, 1.0],
    };
    assert_eq!(extreme.check(), Ok(()));
    let built = run(
        &kernel,
        KernelRequest::make_faces(&on(extreme, circle(2.0))).unwrap(),
    );
    assert_eq!(
        FakeKernel::describe(&built.shape).as_deref(),
        Some("faces(frame, 1 regions, 1 curves)"),
        "{}",
        text(&built.shape)
    );
    let summary = run(&kernel, KernelRequest::summarize(&built.shape));
    assert!((summary.area - 4.0 * std::f64::consts::PI).abs() <= 1e-12);
    // Summarized again from the bytes alone, on another kernel: the same.
    let other = FakeKernel::new(1).unwrap();
    assert_eq!(run(&other, KernelRequest::summarize(&built.shape)), summary);
    let zeros = |sign: f64| FaceFrame {
        origin: [sign * 0.0, sign * 0.0, 7.0],
        x: [1.0, sign * 0.0, sign * 0.0],
        y: [sign * 0.0, 1.0, sign * 0.0],
        normal: [sign * 0.0, sign * 0.0, 1.0],
    };
    let plus = run(
        &kernel,
        KernelRequest::make_faces(&on(zeros(1.0), circle(1.0))).unwrap(),
    );
    let minus = run(
        &kernel,
        KernelRequest::make_faces(&on(zeros(-1.0), circle(1.0))).unwrap(),
    );
    assert_eq!(text(&plus.shape), text(&minus.shape));
}

#[test]
fn frame_lines_the_fake_doesnt_write_dont_read_back() {
    let good =
        "silico fake shape 1\nfaces frame 0 0 7 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n";
    assert!(FakeKernel::describe(&shape(good)).is_some());
    for bad in [
        // A number written otherwise than Rust writes it.
        "silico fake shape 1\nfaces frame 0 0 7.0 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        "silico fake shape 1\nfaces frame 0 0 7 -1e0 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        // A zero written −0, which the fake writes 0.
        "silico fake shape 1\nfaces frame -0 0 7 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        // Thirteen numbers.
        "silico fake shape 1\nfaces frame 0 0 7 -1 0 0 0 1 0 0 0 -1 0 region loop circle 0 0 1\n",
        // No regions.
        "silico fake shape 1\nfaces frame 0 0 7 -1 0 0 0 1 0 0 0 -1\n",
        // Just the word.
        "silico fake shape 1\nfaces frame\n",
        // An infinity, and a number that overflows to one.
        "silico fake shape 1\nfaces frame 0 0 inf -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        "silico fake shape 1\nfaces frame 0 0 1e400 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        // An origin past a kilometre.
        "silico fake shape 1\nfaces frame 0 0 1000001 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        // An origin plane's word with a frame's numbers after it.
        "silico fake shape 1\nfaces xy 0 0 7 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
        // Two spaces.
        "silico fake shape 1\nfaces frame  0 0 7 -1 0 0 0 1 0 0 0 -1 region loop circle 0 0 1\n",
    ] {
        assert_eq!(FakeKernel::describe(&shape(bad)), None, "{bad}");
    }
}

#[test]
fn a_frame_the_protocol_refuses_is_refused_by_the_fake_naming_the_plane() {
    let bad = FaceFrame {
        normal: [0.0, 0.0, f64::NAN],
        ..tilted([0.0; 3])
    };
    let Err(error) = KernelRequest::make_faces(&on(bad, circle(1.0))) else {
        panic!("a NaN frame was taken")
    };
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.parameter(), Some("plane"), "{error:?}");
}

#[test]
fn a_tilted_rectangles_box_and_pad_are_their_closed_forms() {
    // A 4 × 2 rectangle from sketch (0, 0) on the frame tilted 30° about X from (1, 2, 3):
    // its corners (1, 2, 3), (5, 2, 3), (5, 2 + 2c, 3 + 2s), (1, 2 + 2c, 3 + 2s), with c =
    // cos 30°, s = sin 30°. Its box: x 1..5, y 2..2 + 2c, z 3..4. Padded 3 along n
    // (0, −s, c): 8 · 3 = 24 mm³, its box from the faces' and the moved faces' corners: y
    // down to 2 − 3s, z up to 4 + 3c.
    let kernel = FakeKernel::new(1).unwrap();
    let frame = tilted([1.0, 2.0, 3.0]);
    let (c, s) = (frame.y[1], frame.y[2]);
    let built = run(
        &kernel,
        KernelRequest::make_faces(&on(frame, rectangle(0.0, 0.0, 4.0, 2.0))).unwrap(),
    );
    let summary = run(&kernel, KernelRequest::summarize(&built.shape));
    assert_eq!(summary.area, 8.0);
    let [low, high] = summary.bounds.unwrap();
    let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-12);
    assert!(close(low, [1.0, 2.0, 3.0]), "{low:?}");
    assert!(close(high, [5.0, 2.0 + 2.0 * c, 3.0 + 2.0 * s]), "{high:?}");
    let pad = run(
        &kernel,
        KernelRequest::prism(&built.shape, [0.0; 3], frame.normal.map(|k| 3.0 * k)).unwrap(),
    );
    let summary = run(&kernel, KernelRequest::summarize(&pad.shape));
    assert!((summary.volume - 24.0).abs() <= 1e-12, "{}", summary.volume);
    let [low, high] = summary.bounds.unwrap();
    assert!(close(low, [1.0, 2.0 - 3.0 * s, 3.0]), "{low:?}");
    assert!(
        close(high, [5.0, 2.0 + 2.0 * c, 3.0 + 2.0 * s + 3.0 * c]),
        "{high:?}"
    );
}

#[test]
fn a_tilted_circles_box_is_the_box_of_a_tilted_circle() {
    // Broke (break-it), before the fix: the fake's box of faces on a face frame was the box
    // of the sketch box's four corners placed. For a circle of r on a frame whose x and y
    // both lean out of an axis k, that reaches r(|x_k| + |y_k|), past the circle's
    // r·√(1 − n_k²), which the kernel's summary gives (silico-kernel's
    // adversarial_face_frames). Tilted 30° about X the two agree (each axis gets one of x
    // and y), so the fake's own test couldn't see it; tilted 30° about (1, 1, 0)/√2 the
    // fake's box was wider on X and Y, and so were its summaries and the through-all
    // lengths it sizes from them.
    let kernel = FakeKernel::new(1).unwrap();
    let k = [1.0 / 2f64.sqrt(), 1.0 / 2f64.sqrt(), 0.0];
    let (s, c) = (libm::sin(30f64.to_radians()), libm::cos(30f64.to_radians()));
    let column = |e: [f64; 3]| -> [f64; 3] {
        let kxe = [
            k[1] * e[2] - k[2] * e[1],
            k[2] * e[0] - k[0] * e[2],
            k[0] * e[1] - k[1] * e[0],
        ];
        let ke = dot(k, e);
        [0, 1, 2].map(|i| e[i] * c + kxe[i] * s + k[i] * ke * (1.0 - c))
    };
    let frame = FaceFrame {
        origin: [0.0; 3],
        x: column([1.0, 0.0, 0.0]),
        y: column([0.0, 1.0, 0.0]),
        normal: column([0.0, 0.0, 1.0]),
    };
    assert_eq!(frame.check(), Ok(()));
    let built = run(
        &kernel,
        KernelRequest::make_faces(&on(frame, circle(5.0))).unwrap(),
    );
    let [_, high] = run(&kernel, KernelRequest::summarize(&built.shape))
        .bounds
        .unwrap();
    let reach = frame.normal.map(|n| 5.0 * libm::sqrt(1.0 - n * n));
    assert!(
        (0..3).all(|k| (high[k] - reach[k]).abs() <= 1e-9),
        "the fake's box reaches {high:?}; a tilted circle's reaches {reach:?}"
    );
}
