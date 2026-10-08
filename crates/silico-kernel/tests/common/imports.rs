//! Shared by every test binary of silico-kernel (included with `#[path]`): each one links
//! OCCT and the bridge, so each must import no inexact platform math (ARCHITECTURE.md,
//! Deterministic kernel).

/// Runs `check-math-imports.sh` on the running test binary. Calls `sin` first, so that
/// without the libm shim build.rs links into every executable, the binary would import
/// the platform's `sin` and the check would fail.
#[allow(clippy::disallowed_methods)] // the call to f64::sin is the point
pub fn assert_this_binary_imports_no_platform_math() {
    // Rust's sin is a call to the C library's `sin`; black_box keeps it from being folded.
    let s = std::hint::black_box(0.5_f64).sin();
    assert!((s - 0.479_425_538_604_203).abs() < 1e-15, "sin(0.5) = {s}");
    let exe = std::env::current_exe().unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/occt/scripts/check-math-imports.sh"
    );
    let out = std::process::Command::new("bash")
        .arg(script)
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
