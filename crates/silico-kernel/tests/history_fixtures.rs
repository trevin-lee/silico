//! The real kernel's histories that silico-core's naming tests read
//! (`crates/silico-core/tests/fixtures/histories/`, digested with the naming scheme in its
//! `tests/naming_scheme.rs`): each case built here through a kernel worker, and held to the
//! file, so the files say what this kernel reports. `SILICO_RECORD_HISTORIES=1` writes
//! them instead, after a change to what a history holds or to OCCT.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use silico_core::kernel::{BooleanOp, ChamferSize, History, KernelWorker, Primitive};

/// An input: its feature's ID, its primitive, and where it's moved to.
struct Input {
    id: &'static str,
    primitive: Primitive,
    at: [f64; 3],
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../silico-core/tests/fixtures/histories")
}

/// The file's JSON for a case: the operation, its inputs, and the history.
fn json(op: &str, inputs: &[Input], history: &History) -> serde_json::Value {
    serde_json::json!({
        "op": op,
        "inputs": inputs
            .iter()
            .map(|i| serde_json::json!({"id": i.id, "primitive": i.primitive, "at": i.at}))
            .collect::<Vec<_>>(),
        "history": history,
    })
}

/// Each input made and moved.
fn made(kernel: &mut KernelWorker, inputs: &[Input]) -> Vec<silico_core::kernel::ShapeData> {
    inputs
        .iter()
        .map(|input| {
            let shape = kernel.make(input.primitive).unwrap();
            if input.at == [0.0; 3] {
                shape
            } else {
                kernel.translate(&shape, input.at).unwrap().shape
            }
        })
        .collect()
}

#[test]
fn the_recorded_histories_are_the_kernels() {
    let mut kernel =
        KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    let at = |id, primitive, at| Input { id, primitive, at };
    let cube10 = Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    };
    let unit = Primitive::Box {
        dx: 1.0,
        dy: 1.0,
        dz: 1.0,
    };
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
    // A chamfer of a 10 mm cube's Edge7 and Edge8, size 1, and a fillet of its Edge1, 1.
    let inputs = [at("aaaaaaaa", cube10, [0.0; 3])];
    let shapes = made(&mut kernel, &inputs);
    let chamfer = kernel
        .chamfer(
            &shapes[0],
            &[7, 8],
            ChamferSize::Equal { distance: 1.0 },
            false,
        )
        .unwrap();
    cases.push((
        "chamfer-edge7-edge8",
        json("Chamfer", &inputs, &chamfer.history),
    ));
    let fillet = kernel.fillet(&shapes[0], &[1], 1.0).unwrap();
    cases.push(("fillet-edge1", json("Fillet", &inputs, &fillet.history)));
    // The booleans.
    for (case, op, inputs) in [
        (
            "ball-through-hole",
            BooleanOp::Cut,
            [
                at("bbbbbbbb", Primitive::Sphere { radius: 10.0 }, [0.0; 3]),
                at(
                    "pppppppp",
                    Primitive::Cylinder {
                        radius: 2.0,
                        height: 30.0,
                    },
                    [0.0, 0.0, -15.0],
                ),
            ],
        ),
        (
            "plate-slot",
            BooleanOp::Cut,
            [
                at(
                    "pppppppp",
                    Primitive::Box {
                        dx: 40.0,
                        dy: 20.0,
                        dz: 5.0,
                    },
                    [0.0; 3],
                ),
                at(
                    "tttttttt",
                    Primitive::Box {
                        dx: 2.0,
                        dy: 22.0,
                        dz: 2.0,
                    },
                    [19.0, -1.0, 4.0],
                ),
            ],
        ),
        (
            "two-cubes-fuse",
            BooleanOp::Fuse,
            [
                at("aaaaaaaa", unit, [0.0; 3]),
                at("bbbbbbbb", unit, [0.5, 0.5, 0.5]),
            ],
        ),
    ] {
        let shapes = made(&mut kernel, &inputs);
        let built = kernel.boolean(op, &shapes[0], &shapes[1]).unwrap();
        cases.push((case, json(&format!("{op:?}"), &inputs, &built.history)));
    }
    let mut named: Vec<String> = cases
        .iter()
        .map(|(case, _)| format!("{case}.json"))
        .collect();
    let record = std::env::var_os("SILICO_RECORD_HISTORIES").is_some_and(|v| v == "1");
    for (case, value) in cases {
        let path = fixtures().join(format!("{case}.json"));
        if record {
            let text = serde_json::to_string_pretty(&value).unwrap() + "\n";
            std::fs::write(&path, text).unwrap();
            continue;
        }
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            file == value,
            "{case}: the kernel's history isn't {}: record them again \
             (SILICO_RECORD_HISTORIES=1), and bump NAMING_SCHEME if names change",
            path.display()
        );
    }
    // Every file is a case here, so none is digested without being held to the kernel
    // (checked after recording, so a new case can be recorded).
    let mut files: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    named.sort();
    assert_eq!(files, named, "files in {}", fixtures().display());
}
