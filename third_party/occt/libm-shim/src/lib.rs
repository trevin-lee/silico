//! C-callable replacements for the platform math library, backed by the
//! pure-Rust `libm` crate, so OCCT computes identical bits on every CPU.
// no_std for the linked object; tests build with std so they can run.
#![cfg_attr(not(test), no_std)]

#[cfg(all(not(test), not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
compile_error!("the libm shim's panic handler supports only aarch64 and x86_64");

/// A panic would mean a bug in `libm`. Trap immediately, so the worker crashes
/// (and the core reports a structured error) instead of hanging.
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `brk` raises a breakpoint exception and never returns.
    unsafe {
        core::arch::asm!("brk #1", options(noreturn))
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: `ud2` raises an invalid-opcode exception and never returns.
    unsafe {
        core::arch::asm!("ud2", options(noreturn))
    }
}

/// Defines C math functions, each backed by a `libm` function.
///
/// Every export carries `#[cfg_attr(not(test), unsafe(no_mangle))]`, applied here once:
/// the C symbol exists only in real builds. In a test build, an exported `sin` would
/// replace the platform `sin` that the tests compare against, and they would compare
/// the shim with itself. Hand-written exports below must carry the same attribute.
macro_rules! export {
    ($($name:ident = $libm:ident($($arg:ident),+): $ty:ty;)*) => {
        $(
            #[cfg_attr(not(test), unsafe(no_mangle))]
            pub extern "C" fn $name($($arg: $ty),+) -> $ty {
                libm::$libm($($arg),+)
            }
        )*

        /// Every function this macro exports, so tests can prove each one is tested.
        #[cfg(test)]
        const MACRO_EXPORTS: &[&str] = &[$(stringify!($name)),*];
    };
}

export! {
    acos = acos(x): f64;
    acosh = acosh(x): f64;
    asin = asin(x): f64;
    asinh = asinh(x): f64;
    atan = atan(x): f64;
    atanh = atanh(x): f64;
    cbrt = cbrt(x): f64;
    cos = cos(x): f64;
    cosh = cosh(x): f64;
    exp = exp(x): f64;
    exp10 = exp10(x): f64;
    // Apple's name for exp10, which clang emits for pow(10, x) on macOS.
    __exp10 = exp10(x): f64;
    exp2 = exp2(x): f64;
    expm1 = expm1(x): f64;
    log = log(x): f64;
    log10 = log10(x): f64;
    log1p = log1p(x): f64;
    log2 = log2(x): f64;
    sin = sin(x): f64;
    sinh = sinh(x): f64;
    tan = tan(x): f64;
    tanh = tanh(x): f64;
    atan2 = atan2(y, x): f64;
    hypot = hypot(x, y): f64;
    pow = pow(x, y): f64;

    acosf = acosf(x): f32;
    acoshf = acoshf(x): f32;
    asinf = asinf(x): f32;
    asinhf = asinhf(x): f32;
    atanf = atanf(x): f32;
    atanhf = atanhf(x): f32;
    cbrtf = cbrtf(x): f32;
    cosf = cosf(x): f32;
    coshf = coshf(x): f32;
    expf = expf(x): f32;
    exp10f = exp10f(x): f32;
    __exp10f = exp10f(x): f32;
    exp2f = exp2f(x): f32;
    expm1f = expm1f(x): f32;
    logf = logf(x): f32;
    log10f = log10f(x): f32;
    log1pf = log1pf(x): f32;
    log2f = log2f(x): f32;
    sinf = sinf(x): f32;
    sinhf = sinhf(x): f32;
    tanf = tanf(x): f32;
    tanhf = tanhf(x): f32;
    atan2f = atan2f(y, x): f32;
    hypotf = hypotf(x, y): f32;
    powf = powf(x, y): f32;
}

/// Apple's combined sine and cosine, which clang emits on macOS when code needs both.
#[repr(C)]
pub struct SinCos {
    sin: f64,
    cos: f64,
}

#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn __sincos_stret(x: f64) -> SinCos {
    SinCos {
        sin: libm::sin(x),
        cos: libm::cos(x),
    }
}

#[repr(C)]
pub struct SinCosF {
    sin: f32,
    cos: f32,
}

#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn __sincosf_stret(x: f32) -> SinCosF {
    SinCosF {
        sin: libm::sinf(x),
        cos: libm::cosf(x),
    }
}

/// glibc's combined sine and cosine, which compilers emit on Linux.
///
/// # Safety
/// `s` and `c` must be valid for writes, as in the C library contract.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn sincos(x: f64, s: *mut f64, c: *mut f64) {
    // SAFETY: the caller guarantees both pointers are valid for writes.
    unsafe {
        *s = libm::sin(x);
        *c = libm::cos(x);
    }
}

/// # Safety
/// `s` and `c` must be valid for writes, as in the C library contract.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn sincosf(x: f32, s: *mut f32, c: *mut f32) {
    // SAFETY: the caller guarantees both pointers are valid for writes.
    unsafe {
        *s = libm::sinf(x);
        *c = libm::cosf(x);
    }
}

#[cfg(test)]
mod tests {
    // Comparing against the platform's math library is the point of these tests.
    #![allow(clippy::disallowed_methods)]

    use core::f64::consts::{FRAC_PI_2, FRAC_PI_6, PI, SQRT_2};

    fn ulps(a: f64, b: f64) -> u64 {
        if a == b || (a.is_nan() && b.is_nan()) {
            return 0;
        }
        (a.to_bits() as i64)
            .wrapping_sub(b.to_bits() as i64)
            .unsigned_abs()
    }

    fn ulps_f32(a: f32, b: f32) -> u32 {
        if a == b || (a.is_nan() && b.is_nan()) {
            return 0;
        }
        (a.to_bits() as i32)
            .wrapping_sub(b.to_bits() as i32)
            .unsigned_abs()
    }

    fn assert_close(name: &str, got: f64, want: f64, max_ulps: u64) {
        assert!(
            ulps(got, want) <= max_ulps,
            "{name}: got {got:e}, want {want:e} ({} ulps)",
            ulps(got, want)
        );
    }

    fn assert_close_f32(name: &str, got: f32, want: f32, max_ulps: u32) {
        assert!(
            ulps_f32(got, want) <= max_ulps,
            "{name}: got {got:e}, want {want:e} ({} ulps)",
            ulps_f32(got, want)
        );
    }

    /// Values known independently of any math library.
    #[test]
    fn known_values() {
        let cases: &[(&str, f64, f64)] = &[
            ("cbrt", super::cbrt(27.0), 3.0),
            ("cbrt", super::cbrt(-8.0), -2.0),
            ("exp2", super::exp2(10.0), 1024.0),
            ("log2", super::log2(1024.0), 10.0),
            ("log10", super::log10(1000.0), 3.0),
            ("exp10", super::exp10(3.0), 1000.0),
            ("__exp10", super::__exp10(3.0), 1000.0),
            ("exp", super::exp(0.0), 1.0),
            ("log", super::log(1.0), 0.0),
            ("pow", super::pow(2.0, 0.5), SQRT_2),
            ("pow", super::pow(10.0, -2.0), 0.01),
            ("hypot", super::hypot(3.0, 4.0), 5.0),
            ("atan2", super::atan2(1.0, 0.0), FRAC_PI_2),
            ("atan2", super::atan2(0.0, -1.0), PI),
            ("acos", super::acos(-1.0), PI),
            ("asin", super::asin(1.0), FRAC_PI_2),
            ("sin", super::sin(FRAC_PI_6), 0.49999999999999994),
            (
                "__sincos_stret.sin",
                super::__sincos_stret(FRAC_PI_6).sin,
                0.49999999999999994,
            ),
            ("tanh", super::tanh(0.0), 0.0),
            ("atanh", super::atanh(0.0), 0.0),
            ("acosh", super::acosh(1.0), 0.0),
            ("expm1", super::expm1(0.0), 0.0),
            ("log1p", super::log1p(0.0), 0.0),
            ("powf", f64::from(super::powf(2.0, 3.0)), 8.0),
            ("exp10f", f64::from(super::exp10f(2.0)), 100.0),
            ("__exp10f", f64::from(super::__exp10f(2.0)), 100.0),
            ("hypotf", f64::from(super::hypotf(3.0, 4.0)), 5.0),
            (
                "atan2f",
                f64::from(super::atan2f(1.0, 0.0)),
                f64::from(core::f32::consts::FRAC_PI_2),
            ),
        ];
        for &(name, got, want) in cases {
            assert_close(name, got, want, 1);
        }
    }

    /// The Apple aliases are the same function as the standard names, bit for bit.
    #[test]
    fn aliases_are_identical() {
        for i in 0..=10_000 {
            let x = -300.0 + 600.0 * f64::from(i) / 10_000.0;
            assert_eq!(
                super::__exp10(x).to_bits(),
                super::exp10(x).to_bits(),
                "__exp10({x})"
            );
            let xf = x as f32 / 10.0;
            assert_eq!(
                super::__exp10f(xf).to_bits(),
                super::exp10f(xf).to_bits(),
                "__exp10f({xf})"
            );
        }
    }

    /// Every f64 export agrees with the platform library to within a few ulps over a
    /// sweep of inputs. Catches a wrong function, swapped arguments, or a typo.
    /// Run with SHIM_REPORT_ULPS=1 and --nocapture to print the worst gap per function.
    #[test]
    fn matches_platform_library() {
        type F1 = (
            &'static str,
            extern "C" fn(f64) -> f64,
            fn(f64) -> f64,
            f64,
            f64,
            u64,
        );
        let unary: &[F1] = &[
            ("sin", super::sin, f64::sin, -50.0, 50.0, 4),
            ("cos", super::cos, f64::cos, -50.0, 50.0, 4),
            ("tan", super::tan, f64::tan, -1.5, 1.5, 4),
            ("asin", super::asin, f64::asin, -1.0, 1.0, 4),
            ("acos", super::acos, f64::acos, -1.0, 1.0, 4),
            ("atan", super::atan, f64::atan, -50.0, 50.0, 4),
            ("sinh", super::sinh, f64::sinh, -20.0, 20.0, 4),
            ("cosh", super::cosh, f64::cosh, -20.0, 20.0, 4),
            ("tanh", super::tanh, f64::tanh, -20.0, 20.0, 4),
            ("asinh", super::asinh, f64::asinh, -50.0, 50.0, 4),
            ("acosh", super::acosh, f64::acosh, 1.0, 50.0, 4),
            // Rust's `f64::atanh` uses a ln_1p formula, not the platform's `atanh`, and is
            // itself ~20 ulps off near |x| = 1; the known values above pin `atanh` down.
            ("atanh", super::atanh, f64::atanh, -0.99, 0.99, 32),
            ("exp", super::exp, f64::exp, -50.0, 50.0, 4),
            ("exp2", super::exp2, f64::exp2, -50.0, 50.0, 4),
            ("expm1", super::expm1, f64::exp_m1, -20.0, 20.0, 4),
            ("log", super::log, f64::ln, 1e-6, 1e6, 4),
            ("log2", super::log2, f64::log2, 1e-6, 1e6, 4),
            ("log10", super::log10, f64::log10, 1e-6, 1e6, 4),
            ("log1p", super::log1p, f64::ln_1p, -0.99, 1e6, 4),
            ("cbrt", super::cbrt, f64::cbrt, -1e6, 1e6, 4),
            ("exp10", super::exp10, |x| 10f64.powf(x), -20.0, 20.0, 4),
        ];
        for &(name, ours, platform, lo, hi, max_ulps) in unary {
            for i in 0..=1000 {
                let x = lo + (hi - lo) * f64::from(i) / 1000.0;
                assert_close(name, ours(x), platform(x), max_ulps);
            }
            if std::env::var_os("SHIM_REPORT_ULPS").is_some() {
                let worst = (0..=100_000)
                    .map(|i| lo + (hi - lo) * f64::from(i) / 100_000.0)
                    .map(|x| ulps(ours(x), platform(x)))
                    .max()
                    .unwrap_or(0);
                std::println!("{name}: worst {worst} ulps vs the platform library");
            }
        }
        for i in 0..=200 {
            let a = -5.0 + 10.0 * f64::from(i) / 200.0;
            for j in 0..=20 {
                let b = -3.0 + 6.0 * f64::from(j) / 20.0;
                assert_close("atan2", super::atan2(a, b), a.atan2(b), 4);
                assert_close("hypot", super::hypot(a, b), a.hypot(b), 4);
                let base = a.abs() + 0.5;
                assert_close("pow", super::pow(base, b), base.powf(b), 4);
            }
        }
        let mut s = 0.0;
        let mut c = 0.0;
        // SAFETY: both pointers point to live locals.
        unsafe { super::sincos(1.25, &mut s, &mut c) };
        assert_close("sincos.sin", s, 1.25f64.sin(), 4);
        assert_close("sincos.cos", c, 1.25f64.cos(), 4);
        let sc = super::__sincos_stret(1.25);
        assert_close("__sincos_stret.sin", sc.sin, 1.25f64.sin(), 4);
        assert_close("__sincos_stret.cos", sc.cos, 1.25f64.cos(), 4);
    }

    /// Every f32 export, including the hand-written ones, against the platform library.
    #[test]
    fn f32_matches_platform_library() {
        type F1 = (
            &'static str,
            extern "C" fn(f32) -> f32,
            fn(f32) -> f32,
            f32,
            f32,
        );
        let unary: &[F1] = &[
            ("sinf", super::sinf, f32::sin, -50.0, 50.0),
            ("cosf", super::cosf, f32::cos, -50.0, 50.0),
            ("tanf", super::tanf, f32::tan, -1.5, 1.5),
            ("asinf", super::asinf, f32::asin, -1.0, 1.0),
            ("acosf", super::acosf, f32::acos, -1.0, 1.0),
            ("atanf", super::atanf, f32::atan, -50.0, 50.0),
            ("sinhf", super::sinhf, f32::sinh, -10.0, 10.0),
            ("coshf", super::coshf, f32::cosh, -10.0, 10.0),
            ("tanhf", super::tanhf, f32::tanh, -10.0, 10.0),
            ("asinhf", super::asinhf, f32::asinh, -50.0, 50.0),
            ("acoshf", super::acoshf, f32::acosh, 1.0, 50.0),
            ("expf", super::expf, f32::exp, -20.0, 20.0),
            ("exp2f", super::exp2f, f32::exp2, -20.0, 20.0),
            ("expm1f", super::expm1f, f32::exp_m1, -10.0, 10.0),
            ("logf", super::logf, f32::ln, 1e-3, 1e3),
            ("log2f", super::log2f, f32::log2, 1e-3, 1e3),
            ("log10f", super::log10f, f32::log10, 1e-3, 1e3),
            ("log1pf", super::log1pf, f32::ln_1p, -0.9, 1e3),
            ("cbrtf", super::cbrtf, f32::cbrt, -1e3, 1e3),
            ("exp10f", super::exp10f, |x| 10f32.powf(x), -10.0, 10.0),
        ];
        for &(name, ours, platform, lo, hi) in unary {
            for i in 0..=1000 {
                let x = lo + (hi - lo) * i as f32 / 1000.0;
                assert_close_f32(name, ours(x), platform(x), 4);
            }
        }
        // As for f64: Rust's atanh is a formula, so it gets a looser bound plus a known value.
        for i in 0..=1000 {
            let x = -0.9 + 1.8 * i as f32 / 1000.0;
            assert_close_f32("atanhf", super::atanhf(x), x.atanh(), 32);
        }
        assert_close_f32("atanhf", super::atanhf(0.0), 0.0, 0);
        for i in 0..=100 {
            let a = -5.0 + 10.0 * i as f32 / 100.0;
            for j in 0..=20 {
                let b = -3.0 + 6.0 * j as f32 / 20.0;
                assert_close_f32("atan2f", super::atan2f(a, b), a.atan2(b), 4);
                assert_close_f32("hypotf", super::hypotf(a, b), a.hypot(b), 4);
                let base = a.abs() + 0.5;
                assert_close_f32("powf", super::powf(base, b), base.powf(b), 4);
            }
        }
        let mut s = 0.0f32;
        let mut c = 0.0f32;
        // SAFETY: both pointers point to live locals.
        unsafe { super::sincosf(1.25, &mut s, &mut c) };
        assert_close_f32("sincosf.sin", s, 1.25f32.sin(), 4);
        assert_close_f32("sincosf.cos", c, 1.25f32.cos(), 4);
        let sc = super::__sincosf_stret(1.25);
        assert_close_f32("__sincosf_stret.sin", sc.sin, 1.25f32.sin(), 4);
        assert_close_f32("__sincosf_stret.cos", sc.cos, 1.25f32.cos(), 4);
    }

    /// Every export is exercised by the tests above. Adding an export without testing it
    /// fails here; hand-written exports are listed by hand.
    #[test]
    fn every_export_is_tested() {
        let tested: &[&str] = &[
            // matches_platform_library (f64 sweeps, two-argument grid, sincos)
            "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "asinh", "acosh",
            "atanh", "exp", "exp2", "expm1", "log", "log2", "log10", "log1p", "cbrt", "exp10",
            "atan2", "hypot", "pow", // aliases_are_identical
            "__exp10", "__exp10f", // f32_matches_platform_library
            "sinf", "cosf", "tanf", "asinf", "acosf", "atanf", "sinhf", "coshf", "tanhf", "asinhf",
            "acoshf", "atanhf", "expf", "exp2f", "expm1f", "logf", "log2f", "log10f", "log1pf",
            "cbrtf", "exp10f", "atan2f", "hypotf", "powf",
        ];
        for name in super::MACRO_EXPORTS {
            assert!(tested.contains(name), "export {name} has no test");
        }
        // The hand-written exports, tested in matches_platform_library and
        // f32_matches_platform_library.
        let _ = (
            super::sincos,
            super::sincosf,
            super::__sincos_stret,
            super::__sincosf_stret,
        );
    }

    /// IEEE special values come through unchanged in kind.
    #[test]
    fn special_values() {
        assert!(super::sin(f64::NAN).is_nan());
        assert!(super::log(-1.0).is_nan());
        assert_eq!(super::log(0.0), f64::NEG_INFINITY);
        assert_eq!(super::exp(f64::INFINITY), f64::INFINITY);
        assert_eq!(super::exp(f64::NEG_INFINITY), 0.0);
        assert_eq!(super::pow(0.0, 0.0), 1.0);
        assert_eq!(super::sin(-0.0).to_bits(), (-0.0f64).to_bits());
        // Huge arguments go through full argument reduction.
        assert_close("sin(1e22)", super::sin(1e22), (1e22f64).sin(), 4);
        assert_close("cos(1e300)", super::cos(1e300), (1e300f64).cos(), 4);
    }
}
