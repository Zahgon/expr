//! Follows the behaviour of Go's standard library math package (The Go Authors, BSD-style licence).
//!
//! `**` is Go's `math.Pow`, which is not the C library's `pow`: integer exponents go through repeated
//! squaring on the mantissa, so results differ from `f64::powf` in the last bits. The polynomial constants are
//! Go's, digit for digit.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

const SHIFT: u32 = 64 - 11 - 1;
const MASK: u64 = 0x7FF;
const BIAS: i64 = 1023;

fn normalize(x: f64) -> (f64, i64) {
    const SMALLEST_NORMAL: f64 = 2.2250738585072014e-308;
    if x.abs() < SMALLEST_NORMAL {
        (x * (1u64 << 52) as f64, -52)
    } else {
        (x, 0)
    }
}

/// Go's `math.Frexp`.
pub fn frexp(f: f64) -> (f64, i64) {
    if f == 0.0 || f.is_infinite() || f.is_nan() {
        return (f, 0);
    }
    let (f, mut exp) = normalize(f);
    let mut x = f.to_bits();
    exp += ((x >> SHIFT) & MASK) as i64 - BIAS + 1;
    x &= !(MASK << SHIFT);
    x |= ((-1 + BIAS) as u64) << SHIFT;
    (f64::from_bits(x), exp)
}

/// Go's `math.Ldexp`.
pub fn ldexp(frac: f64, exp: i64) -> f64 {
    if frac == 0.0 || frac.is_infinite() || frac.is_nan() {
        return frac;
    }
    let (frac, e) = normalize(frac);
    let mut exp = exp.saturating_add(e);
    let mut x = frac.to_bits();
    exp = exp.saturating_add(((x >> SHIFT) & MASK) as i64 - BIAS);
    if exp < -1075 {
        return 0.0f64.copysign(frac);
    }
    if exp > 1023 {
        return if frac < 0.0 { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let mut m = 1.0;
    if exp < -1022 {
        exp += 53;
        m = 1.0 / (1u64 << 53) as f64;
    }
    x &= !(MASK << SHIFT);
    x |= ((exp + BIAS) as u64) << SHIFT;
    m * f64::from_bits(x)
}

/// Go's `math.Modf`: the integer and fractional parts, both with the sign of `f`.
pub fn modf(f: f64) -> (f64, f64) {
    if f < 1.0 {
        if f < 0.0 {
            let (i, fr) = modf(-f);
            return (-i, -fr);
        }
        if f == 0.0 {
            return (f, f);
        }
        return (0.0, f);
    }
    let mut x = f.to_bits();
    let e = ((x >> SHIFT) & MASK) as i64 - BIAS;
    if e < 64 - 12 {
        x &= !((1u64 << (64 - 12 - e)) - 1);
    }
    let int = f64::from_bits(x);
    (int, f - int)
}

fn is_odd_int(x: f64) -> bool {
    if x.abs() >= (1u64 << 53) as f64 {
        return false;
    }
    let (xi, xf) = modf(x);
    xf == 0.0 && (xi as i64) & 1 == 1
}

/// Go's `math.Exp` (the portable implementation).
pub fn exp(x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01;
    const LN2_LO: f64 = 1.90821492927058770002e-10;
    const LOG2E: f64 = 1.44269504088896338700e+00;
    const OVERFLOW: f64 = 7.09782712893383973096e+02;
    const UNDERFLOW: f64 = -7.45133219101941108420e+02;
    const NEAR_ZERO: f64 = 1.0 / (1u64 << 28) as f64;
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == f64::NEG_INFINITY || x < UNDERFLOW {
        return 0.0;
    }
    if x > OVERFLOW {
        return f64::INFINITY;
    }
    if -NEAR_ZERO < x && x < NEAR_ZERO {
        return 1.0 + x;
    }
    let k: i64 = if x < 0.0 {
        (LOG2E * x - 0.5) as i64
    } else if x > 0.0 {
        (LOG2E * x + 0.5) as i64
    } else {
        0
    };
    let hi = x - k as f64 * LN2_HI;
    let lo = k as f64 * LN2_LO;
    expmulti(hi, lo, k)
}

fn expmulti(hi: f64, lo: f64, k: i64) -> f64 {
    const P1: f64 = 1.66666666666666657415e-01;
    const P2: f64 = -2.77777777770155933842e-03;
    const P3: f64 = 6.61375632143793436117e-05;
    const P4: f64 = -1.65339022054652515390e-06;
    const P5: f64 = 4.13813679705723846039e-08;
    let r = hi - lo;
    let t = r * r;
    let c = r - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let y = 1.0 - ((lo - (r * c) / (2.0 - c)) - hi);
    ldexp(y, k)
}

/// Go's `math.Log` (the portable implementation).
pub fn log(x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01;
    const LN2_LO: f64 = 1.90821492927058770002e-10;
    const L1: f64 = 6.666666666666735130e-01;
    const L2: f64 = 3.999999999940941908e-01;
    const L3: f64 = 2.857142874366239149e-01;
    const L4: f64 = 2.222219843214978396e-01;
    const L5: f64 = 1.818357216161805012e-01;
    const L6: f64 = 1.531383769920937332e-01;
    const L7: f64 = 1.479819860511658591e-01;
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    let (mut f1, mut ki) = frexp(x);
    if f1 < std::f64::consts::SQRT_2 / 2.0 {
        f1 *= 2.0;
        ki -= 1;
    }
    let f = f1 - 1.0;
    let k = ki as f64;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let t1 = s2 * (L1 + s4 * (L3 + s4 * (L5 + s4 * L7)));
    let t2 = s4 * (L2 + s4 * (L4 + s4 * L6));
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    k * LN2_HI - ((hfsq - (s * (hfsq + r) + k * LN2_LO)) - f)
}

/// Go's `math.Pow`.
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if y == 1.0 {
        return x;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 {
        if y < 0.0 {
            return if x.is_sign_negative() && is_odd_int(y) { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        if y > 0.0 {
            return if x.is_sign_negative() && is_odd_int(y) { x } else { 0.0 };
        }
    }
    if y.is_infinite() {
        return if x == -1.0 {
            1.0
        } else if (x.abs() < 1.0) == (y == f64::INFINITY) {
            0.0
        } else {
            f64::INFINITY
        };
    }
    if x.is_infinite() {
        if x == f64::NEG_INFINITY {
            return pow(1.0 / x, -y);
        }
        if y < 0.0 {
            return 0.0;
        }
        if y > 0.0 {
            return f64::INFINITY;
        }
    }
    if y == 0.5 {
        return x.sqrt();
    }
    if y == -0.5 {
        return 1.0 / x.sqrt();
    }

    let (mut yi, mut yf) = modf(y.abs());
    if yf != 0.0 && x < 0.0 {
        return f64::NAN;
    }
    if yi >= 9.223372036854775808e18 {
        return if x == -1.0 {
            1.0
        } else if (x.abs() < 1.0) == (y > 0.0) {
            0.0
        } else {
            f64::INFINITY
        };
    }

    let mut a1 = 1.0;
    let mut ae: i64 = 0;
    if yf != 0.0 {
        if yf > 0.5 {
            yf -= 1.0;
            yi += 1.0;
        }
        a1 = exp(yf * log(x));
    }

    let (mut x1, mut xe) = frexp(x);
    let mut i = yi as i64;
    while i != 0 {
        if !(-(1 << 12)..=(1 << 12)).contains(&xe) {
            ae += xe;
            break;
        }
        if i & 1 == 1 {
            a1 *= x1;
            ae += xe;
        }
        x1 *= x1;
        xe <<= 1;
        if x1 < 0.5 {
            x1 += x1;
            xe -= 1;
        }
        i >>= 1;
    }

    if y < 0.0 {
        a1 = 1.0 / a1;
        ae = -ae;
    }
    ldexp(a1, ae)
}

/// Go's `math.Round`: half away from zero.
pub fn round(x: f64) -> f64 {
    x.round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_powers_use_repeated_squaring() {
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(10.0, -2.0), 0.01);
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(1.1, 2.0), 1.2100000000000002);
        assert!(pow(-8.0, 1.0 / 3.0).is_nan());
        assert_eq!(pow(0.0, -1.0), f64::INFINITY);
        assert_eq!(pow(4.0, 0.5), 2.0);
    }

    #[test]
    fn frexp_and_ldexp_round_trip() {
        assert_eq!(frexp(8.0), (0.5, 4));
        assert_eq!(ldexp(0.5, 4), 8.0);
        assert_eq!(ldexp(1.0, -1074), 5e-324);
        assert_eq!(modf(-3.25), (-3.0, -0.25));
    }
}
