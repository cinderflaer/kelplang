//! `std/math`: the libm functions and the two float predicates.

unsafe extern "C" {
    fn pow(base: f64, exp: f64) -> f64;
    fn sqrt(x: f64) -> f64;
    fn floor(x: f64) -> f64;
    fn ceil(x: f64) -> f64;
    fn round(x: f64) -> f64;
    fn trunc(x: f64) -> f64;
    fn exp(x: f64) -> f64;
    fn log(x: f64) -> f64;
    fn log2(x: f64) -> f64;
    fn log10(x: f64) -> f64;
    fn sin(x: f64) -> f64;
    fn cos(x: f64) -> f64;
    fn tan(x: f64) -> f64;
    fn atan2(y: f64, x: f64) -> f64;
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__sqrt_raw(x: f64) -> f64 {
    unsafe { sqrt(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__pow_raw(base: f64, exp_: f64) -> f64 {
    unsafe { pow(base, exp_) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__floor_raw(x: f64) -> f64 {
    unsafe { floor(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__ceil_raw(x: f64) -> f64 {
    unsafe { ceil(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__round_raw(x: f64) -> f64 {
    unsafe { round(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__trunc_raw(x: f64) -> f64 {
    unsafe { trunc(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__exp_raw(x: f64) -> f64 {
    unsafe { exp(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__ln_raw(x: f64) -> f64 {
    unsafe { log(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__log2_raw(x: f64) -> f64 {
    unsafe { log2(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__log10_raw(x: f64) -> f64 {
    unsafe { log10(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__sin_raw(x: f64) -> f64 {
    unsafe { sin(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__cos_raw(x: f64) -> f64 {
    unsafe { cos(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__tan_raw(x: f64) -> f64 {
    unsafe { tan(x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__atan2_raw(y: f64, x: f64) -> f64 {
    unsafe { atan2(y, x) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__is_nan_raw(x: f64) -> bool {
    x.is_nan()
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__math__is_finite_raw(x: f64) -> bool {
    x.is_finite()
}
