//! Vector-ALU conformance: the HLSL the translator emits for each RDNA2 instruction, run on
//! the host GPU, against a reference model of the instruction.
//!
//! For every case an instruction is hand-assembled (`v3 = op(v0, v1, v2)`, VOP3 encoding so
//! source/output modifiers can be exercised), translated with the same emitter as real game
//! shaders (`hlsl::translate_valu_probe`), compiled with DXC and dispatched over a few
//! thousand input vectors: special values (zeros, infinities, NaN, half-float limits, integer
//! limits, rounding ties) plus pseudo-random ones. The reference models follow the RDNA2 ISA
//! reference guide (AMD, 2020-11-30) pseudocode quoted in the comments; where the guide is
//! incomplete the source used instead is named.
//!
//! Policy for floating point:
//! * any NaN matches any NaN (payload and sign are not modelled), +0 matches -0;
//! * input vectors containing f32 denormals are skipped (D3D leaves denormal handling of
//!   shader model 6.0 float32 math to the implementation), and a denormal reference result
//!   also accepts zero;
//! * transcendental instructions (exp, log, rcp, rsq, sqrt, sin, cos) are compared with the
//!   tolerance named in their case, not bit-exactly;
//! * multiply-add instructions accept both the fused and the unfused result: HLSL has no
//!   single-precision fused multiply-add, so `a * b + c` may compile to either.
//!
//! The standalone GPU test needs Windows, Direct3D 12 and DXC. Coin Run's Linux launcher
//! also runs the same vectors and reference through Xbox PSO creation under Proton.

#[cfg(windows)]
use tombox_xshader::gpu::Gpu;
#[cfg(windows)]
use tombox_xshader::hlsl::{translate_valu_probe, valu_probe_stride};
#[cfg(windows)]
use tombox_xshader::dxc;

#[cfg(windows)]
const NV: u32 = 4; // v0..v2 = sources, v3 = destination (also loaded: MAC-style ops read it)
const V: u32 = 256; // VGPR operand code base
const VCC_LO: u32 = 106;
const S_ENDPGM: u32 = 0xbf81_0000;
const ZERO: u32 = 0x80; // inline constant 0

// ---------------------------------------------------------------------------------------
// Encoders

#[derive(Clone, Copy, Default)]
struct Mods {
    abs: u32,
    neg: u32,
    clamp: bool,
    omod: u32,
}

/// VOP3A: v3 = op(v0, v1, v2) (unused sources = inline 0). `vdst` overrides the destination
/// code (compares write a scalar mask: 106 = VCC_LO).
fn vop3(op: u32, nsrc: usize, m: Mods, vdst: u32, src2: Option<u32>) -> Vec<u32> {
    let s1 = if nsrc >= 2 { V + 1 } else { ZERO };
    let s2 = src2.unwrap_or(if nsrc >= 3 { V + 2 } else { ZERO });
    vec![
        (0b110101 << 26) | (op << 16) | ((m.clamp as u32) << 15) | (m.abs << 8) | vdst,
        (m.neg << 29) | (m.omod << 27) | (s2 << 18) | (s1 << 9) | V,
        S_ENDPGM,
    ]
}

/// VOP3B (carry forms): v3 = op(v0, v1[, carry-in]), carry-out to VCC.
fn vop3b(op: u32, carry_in: bool) -> Vec<u32> {
    let s2 = if carry_in { VCC_LO } else { ZERO };
    vec![(0b110101 << 26) | (op << 16) | (VCC_LO << 8) | 3, (s2 << 18) | ((V + 1) << 9) | V, S_ENDPGM]
}

/// VOP3P: v3 = op(v0, v1, v2) on packed halves; op_sel = 0, op_sel_hi = all 1 (the default
/// "low halves for the low result, high halves for the high result").
fn vop3p(op: u32, nsrc: usize, op_sel: u32, op_sel_hi: u32, neg: u32, neg_hi: u32) -> Vec<u32> {
    let s1 = if nsrc >= 2 { V + 1 } else { ZERO };
    let s2 = if nsrc >= 3 { V + 2 } else { ZERO };
    vec![
        (0b110011 << 26) | (op << 16) | (((op_sel_hi >> 2) & 1) << 14) | (op_sel << 11) | (neg_hi << 8) | 3,
        (neg << 29) | ((op_sel_hi & 3) << 27) | (s2 << 18) | (s1 << 9) | V,
        S_ENDPGM,
    ]
}

/// VOPC (32-bit encoding): VCC = cmp(v0, v1).
fn vopc(op: u32) -> Vec<u32> {
    vec![(0b0111110 << 25) | (op << 17) | ((1) << 9) | V, S_ENDPGM]
}

// ---------------------------------------------------------------------------------------
// Number helpers

fn f(u: u32) -> f32 {
    f32::from_bits(u)
}
fn b(x: f32) -> u32 {
    x.to_bits()
}
fn is_denorm(u: u32) -> bool {
    (u & 0x7f80_0000) == 0 && (u & 0x007f_ffff) != 0
}

/// f32 -> f16 bits, round to nearest even (`rtz = false`) or toward zero, with f16 denormals.
fn f16_from(x: f32, rtz: bool) -> u32 {
    let sign = (x.to_bits() >> 16) & 0x8000;
    if x.is_nan() {
        return sign | 0x7e00;
    }
    if x.is_infinite() {
        return sign | 0x7c00;
    }
    let a = (x as f64).abs();
    if a == 0.0 {
        return sign;
    }
    let e = ((a.to_bits() >> 52) & 0x7ff) as i32 - 1023; // a is a normal f64
    let q = 2f64.powi(if e < -14 { -24 } else { e - 10 });
    let n = a / q; // exact: a has at most 24 significant bits
    let n = if rtz { n.trunc() } else { n.round_ties_even() };
    let v = n * q;
    if v > 65504.0 {
        return sign | if rtz { 0x7bff } else { 0x7c00 };
    }
    if v < 2f64.powi(-14) {
        return sign | (v / 2f64.powi(-24)) as u32;
    }
    let e = ((v.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    let m = (v / 2f64.powi(e - 10)) as u32 - 1024;
    sign | (((e + 15) as u32) << 10) | m
}

fn f16_to(h: u32) -> f32 {
    let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 0x1f, h & 0x3ff);
    let v = if e == 0 {
        m as f32 * 2f32.powi(-24)
    } else if e == 31 {
        if m == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15)
    };
    if s == 1 { -v } else { v }
}

fn ulp_diff(a: f32, b: f32) -> u64 {
    let key = |x: f32| -> i64 {
        let i = x.to_bits() as i32 as i64;
        if i < 0 { i64::from(i32::MIN) - i } else { i }
    };
    (key(a) - key(b)).unsigned_abs()
}

fn sext24(x: u32) -> i32 {
    ((x << 8) as i32) >> 8
}

// ---------------------------------------------------------------------------------------
// Expectations

#[derive(Clone, Debug)]
enum Want {
    /// f32 result: one of these values (NaN ~ NaN, +0 ~ -0, a denormal also accepts 0).
    F(Vec<f32>),
    /// f32 within `ulps` units in the last place, or `abs` absolute error.
    Approx { v: f32, ulps: u64, abs: f32 },
    /// Exact 32-bit result.
    U(u32),
    /// Low 16 bits hold this f16 (NaN ~ NaN, +0 ~ -0); the high bits are not checked.
    H(u32),
    /// Two packed f16 results.
    H2(u32, u32),
}

fn f_match(g: f32, w: f32) -> bool {
    if g.is_nan() || w.is_nan() {
        return g.is_nan() && w.is_nan();
    }
    if g == w {
        return true; // includes +0 == -0
    }
    is_denorm(b(w)) && g == 0.0
}

fn h_match(g: u32, w: u32) -> bool {
    let (gf, wf) = (f16_to(g & 0xffff), f16_to(w & 0xffff));
    if gf.is_nan() || wf.is_nan() {
        return gf.is_nan() && wf.is_nan();
    }
    gf == wf
}

fn matches(got: u32, want: &Want) -> bool {
    match want {
        Want::F(vs) => vs.iter().any(|w| f_match(f(got), *w)),
        Want::Approx { v, ulps, abs } => {
            let g = f(got);
            if g.is_nan() || v.is_nan() {
                return g.is_nan() && v.is_nan();
            }
            if g.is_infinite() || v.is_infinite() {
                return g == *v;
            }
            f_match(g, *v) || ulp_diff(g, *v) <= *ulps || (g - v).abs() <= *abs
        }
        Want::U(w) => got == *w,
        Want::H(w) => h_match(got, *w),
        Want::H2(lo, hi) => h_match(got, *lo) && h_match(got >> 16, *hi),
    }
}

// ---------------------------------------------------------------------------------------
// Inputs

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    /// A float with a moderate exponent (|x| in about 2^-24 .. 2^24), random sign.
    fn float(&mut self) -> u32 {
        let r = self.next();
        let e = 127 - 24 + (r % 49);
        (self.next() & 0x8000_0000) | (e << 23) | (self.next() & 0x7f_ffff)
    }
    /// Any non-denormal bit pattern.
    fn bits_float(&mut self) -> u32 {
        loop {
            let v = self.next();
            if !is_denorm(v) {
                return v;
            }
        }
    }
}

fn float_specials() -> Vec<u32> {
    let mut v: Vec<u32> = [
        0.0f32, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 3.5, -3.5, 0.25, 0.75, 0.1, -0.1,
        0.499_999_97, 0.500_000_06, 1.0 / 3.0, std::f32::consts::PI, 100.0, -100.0, 255.5, 1e-30, -1e-30,
        1e30, -1e30, 6.1e-5, 6.0e-8, 3.0e-8, 2.98e-8, 65504.0, -65504.0, 65519.996, 65520.0, -65520.0,
        70000.0, -70000.0, 1e5, 16_777_216.0, 16_777_218.0, 2_147_483_648.0, -2_147_483_648.0,
        -2_147_483_904.0, 4_294_967_296.0, 4_294_967_040.0, 3e9, -3e9, f32::MAX, -f32::MAX,
        f32::MIN_POSITIVE, -f32::MIN_POSITIVE, f32::INFINITY, f32::NEG_INFINITY,
    ]
    .iter()
    .map(|x| x.to_bits())
    .collect();
    v.push(0x7fc0_0000); // quiet NaN
    v.push(0xffc0_0000);
    v
}

fn int_specials() -> Vec<u32> {
    vec![
        0, 1, 2, 3, 7, 8, 15, 16, 31, 32, 33, 63, 64, 0x7f, 0x80, 0xff, 0x100, 0x7fff, 0x8000, 0xffff, 0x1_0000,
        0x7f_ffff, 0x80_0000, 0xff_ffff, 0x100_0000, 0x7fff_ffff, 0x8000_0000, 0x8000_0001, 0xffff_fffe, 0xffff_ffff,
        0x1234_5678, 0xdead_beef, 0x0f0f_0f0f, 0xf0f0_f0f0,
    ]
}

fn f16_specials() -> Vec<u32> {
    vec![
        0x0000, 0x8000, 0x3c00, 0xbc00, 0x3800, 0x3e00, 0x4100, 0x7bff, 0xfbff, 0x0001, 0x03ff, 0x0400, 0x8001,
        0x7c00, 0xfc00, 0x7e00, 0x3555, 0x5948, 0x2e66, 0xc8f5,
    ]
}

#[derive(Clone, Copy)]
enum Dom {
    /// f32: specials and random.
    F,
    /// f32 restricted to |x| <= 64 (trigonometric range).
    Trig,
    /// 32-bit integers.
    I,
    /// f16 in the low half, random high half.
    H,
    /// Two packed f16.
    H2,
    /// Small shift/width amounts (0..40) with random high bits sometimes.
    Small,
}

fn pick(d: Dom, r: &mut Rng, i: usize) -> u32 {
    let fs = float_specials();
    let is = int_specials();
    let hs = f16_specials();
    match d {
        Dom::F => if i % 3 == 0 { fs[r.next() as usize % fs.len()] } else if i % 3 == 1 { r.float() } else { r.bits_float() },
        Dom::Trig => b(((r.next() as f32 / u32::MAX as f32) - 0.5) * 128.0),
        Dom::I => if i % 2 == 0 { is[r.next() as usize % is.len()] } else { r.next() },
        Dom::H => (r.next() & 0xffff_0000) | if i % 2 == 0 { hs[r.next() as usize % hs.len()] } else { r.next() & 0xffff },
        Dom::H2 => {
            let mut h = || if r.next() % 2 == 0 { hs[r.next() as usize % hs.len()] } else { r.next() & 0xffff };
            h() | (h() << 16)
        }
        Dom::Small => (r.next() % 41) | if i % 4 == 0 { r.next() & 0xffff_ff00 } else { 0 },
    }
}

/// Input vectors: the cross product of the special values for the first two float/int
/// sources, then random vectors.
fn vectors(doms: &[Dom; 3], vcc: bool, seed: u64) -> Vec<[u32; 4]> {
    let mut r = Rng(seed | 1);
    let mut out = Vec::new();
    let spec = |d: Dom| -> Vec<u32> {
        match d {
            Dom::F => float_specials(),
            Dom::I => int_specials(),
            Dom::H => f16_specials(),
            _ => Vec::new(),
        }
    };
    let (a, bb) = (spec(doms[0]), spec(doms[1]));
    for &x in &a {
        if bb.is_empty() {
            out.push([x, pick(doms[1], &mut r, 1), pick(doms[2], &mut r, 1), if vcc { r.next() & 1 } else { 0 }]);
        }
        for &y in &bb {
            out.push([x, y, pick(doms[2], &mut r, out.len()), if vcc { r.next() & 1 } else { 0 }]);
        }
    }
    for i in 0..3000 {
        out.push([pick(doms[0], &mut r, i), pick(doms[1], &mut r, i), pick(doms[2], &mut r, i), if vcc { r.next() & 1 } else { 0 }]);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Cases

/// Where a case's result is read from.
#[derive(Clone, Copy, PartialEq)]
enum Out {
    V3,
    Vcc,
    Exec,
    /// v3 and the carry bit in VCC (`U` expectations only; carry is checked exactly).
    V3Carry,
}

struct Case {
    name: String,
    words: Vec<u32>,
    doms: [Dom; 3],
    vcc_in: bool,
    out: Out,
    /// Reference: (v0, v1, v2, vcc) -> expectation; for `V3Carry` also the carry bit.
    want: Box<dyn Fn(u32, u32, u32, u32) -> (Want, u32)>,
    /// Inputs where some source is an f32 denormal are skipped.
    float_in: bool,
}

fn case(name: &str, words: Vec<u32>, doms: [Dom; 3], want: impl Fn(u32, u32, u32, u32) -> Want + 'static) -> Case {
    let float_in = doms.iter().any(|d| matches!(d, Dom::F | Dom::Trig));
    Case { name: name.into(), words, doms, vcc_in: false, out: Out::V3, want: Box::new(move |a, b, c, v| (want(a, b, c, v), 0)), float_in }
}

fn fl(x: f32) -> Want {
    Want::F(vec![x])
}

/// RDNA2 V_MIN_F32 / V_MAX_F32 with IEEE mode off (graphics): a NaN operand yields the other
/// operand; otherwise the smaller/larger value.
fn amd_min(a: f32, b: f32) -> f32 {
    if a.is_nan() { b } else if b.is_nan() { a } else if a < b { a } else { b }
}
fn amd_max(a: f32, b: f32) -> f32 {
    if a.is_nan() { b } else if b.is_nan() { a } else if a > b { a } else { b }
}
fn amd_med3(a: f32, b: f32, c: f32) -> f32 {
    // ISA: if any NaN -> V_MIN3_F32; else the median.
    if a.is_nan() || b.is_nan() || c.is_nan() {
        return amd_min(amd_min(a, b), c);
    }
    let mx = amd_max(amd_max(a, b), c);
    if mx == a { amd_max(b, c) } else if mx == b { amd_max(a, c) } else { amd_max(a, b) }
}

/// Fused and unfused results of a*b+c; when the product is an f32 denormal, also the result
/// with the product flushed to zero (denormal flushing is the implementation's choice).
fn mad_any(a: f32, b: f32, c: f32) -> Want {
    let mut v = vec![libm::fmaf(a, b, c), a * b + c];
    if is_denorm(b_(a * b)) || is_denorm(b_((a as f64 * b as f64) as f32)) {
        v.push(c);
    }
    Want::F(v)
}

#[test]
fn mad_reference_accepts_both_roundings() {
    // This input from the conformance suite rounds one ULP higher when fused.
    let bits = std::hint::black_box([0x3fc00000, 0x3effffff, 0xb873abfd]);
    let [a, b, c] = bits.map(f32::from_bits);
    let want = mad_any(a, b, c);
    assert!(matches(0x3f3ffc30, &want)); // unfused
    assert!(matches(0x3f3ffc31, &want)); // fused
    assert!(!matches(0x3f3ffc32, &want));
}

fn b_(x: f32) -> u32 {
    x.to_bits()
}
fn legacy_mul(a: f32, b: f32) -> f32 {
    if a == 0.0 || b == 0.0 { 0.0 } else { a * b }
}

fn cases() -> Vec<Case> {
    use Dom::*;
    let m0 = Mods::default();
    let v3 = |op: u32, n: usize| vop3(op, n, m0, 3, None);
    let mut c: Vec<Case> = Vec::new();
    // --- f32 arithmetic (VOP2 opcodes in VOP3 form: 0x100 + op)
    c.push(case("v_add_f32", v3(0x103, 2), [F, F, I], |a, b2, _, _| fl(f(a) + f(b2))));
    c.push(case("v_sub_f32", v3(0x104, 2), [F, F, I], |a, b2, _, _| fl(f(a) - f(b2))));
    c.push(case("v_subrev_f32", v3(0x105, 2), [F, F, I], |a, b2, _, _| fl(f(b2) - f(a))));
    c.push(case("v_mul_legacy_f32", v3(0x107, 2), [F, F, I], |a, b2, _, _| fl(legacy_mul(f(a), f(b2)))));
    c.push(case("v_mul_f32", v3(0x108, 2), [F, F, I], |a, b2, _, _| fl(f(a) * f(b2))));
    c.push(case("v_min_f32", v3(0x10f, 2), [F, F, I], |a, b2, _, _| fl(amd_min(f(a), f(b2)))));
    c.push(case("v_max_f32", v3(0x110, 2), [F, F, I], |a, b2, _, _| fl(amd_max(f(a), f(b2)))));
    // MAC forms accumulate into the destination; the harness loads v3 = v2 beforehand.
    c.push(case("v_mac_f32", v3(0x11f, 2), [F, F, F], |a, b2, c2, _| mad_any(f(a), f(b2), f(c2))));
    c.push(case("v_fmac_f32", v3(0x12b, 2), [F, F, F], |a, b2, c2, _| mad_any(f(a), f(b2), f(c2))));
    // GFX10.1 V_MAD_LEGACY_F32 (0x140; GFX10.3 renames it V_FMA_LEGACY_F32): DX9 multiply.
    c.push(case("v_mad_legacy_f32", v3(0x140, 3), [F, F, F], |a, b2, c2, _| {
        let p = legacy_mul(f(a), f(b2));
        let fused = if f(a) == 0.0 || f(b2) == 0.0 { f(c2) } else { libm::fmaf(f(a), f(b2), f(c2)) };
        let mut v = vec![p + f(c2), fused];
        if is_denorm(b(p)) {
            v.push(f(c2)); // denormal product flushed
        }
        Want::F(v)
    }));
    c.push(case("v_mad_f32", v3(0x141, 3), [F, F, F], |a, b2, c2, _| mad_any(f(a), f(b2), f(c2))));
    c.push(case("v_fma_f32", v3(0x14b, 3), [F, F, F], |a, b2, c2, _| mad_any(f(a), f(b2), f(c2))));
    c.push(case("v_min3_f32", v3(0x151, 3), [F, F, F], |a, b2, c2, _| fl(amd_min(amd_min(f(a), f(b2)), f(c2)))));
    c.push(case("v_max3_f32", v3(0x154, 3), [F, F, F], |a, b2, c2, _| fl(amd_max(amd_max(f(a), f(b2)), f(c2)))));
    c.push(case("v_med3_f32", v3(0x157, 3), [F, F, F], |a, b2, c2, _| fl(amd_med3(f(a), f(b2), f(c2)))));
    // --- modifiers on v_add_f32: abs(src0), neg(src1), clamp, omod
    c.push(case("v_add_f32 |a|", vop3(0x103, 2, Mods { abs: 1, ..m0 }, 3, None), [F, F, I], |a, b2, _, _| fl(f(a).abs() + f(b2))));
    c.push(case("v_add_f32 -b", vop3(0x103, 2, Mods { neg: 2, ..m0 }, 3, None), [F, F, I], |a, b2, _, _| fl(f(a) - f(b2))));
    c.push(case("v_add_f32 clamp", vop3(0x103, 2, Mods { clamp: true, ..m0 }, 3, None), [F, F, I], |a, b2, _, _| {
        // Clamp to [0, 1]; a NaN result clamps to 0 (DX10 clamp semantics).
        let s = f(a) + f(b2);
        fl(if s.is_nan() { 0.0 } else { s.clamp(0.0, 1.0) })
    }));
    for (omod, k) in [(1u32, 2.0f32), (2, 4.0), (3, 0.5)] {
        c.push(case(&format!("v_add_f32 omod{omod}"), vop3(0x103, 2, Mods { omod, ..m0 }, 3, None), [F, F, I], move |a, b2, _, _| fl((f(a) + f(b2)) * k)));
    }
    // --- conversions (VOP1 opcodes in VOP3 form: 0x180 + op)
    c.push(case("v_cvt_f32_i32", v3(0x185, 1), [I, I, I], |a, _, _, _| fl(a as i32 as f32)));
    c.push(case("v_cvt_f32_u32", v3(0x186, 1), [I, I, I], |a, _, _, _| fl(a as f32)));
    // ISA: out-of-range values (including infinity) saturate, NaN converts to 0. Rust's
    // float-to-int casts have exactly these semantics.
    c.push(case("v_cvt_u32_f32", v3(0x187, 1), [F, I, I], |a, _, _, _| Want::U(f(a) as u32)));
    c.push(case("v_cvt_i32_f32", v3(0x188, 1), [F, I, I], |a, _, _, _| Want::U(f(a) as i32 as u32)));
    c.push(case("v_cvt_f16_f32", v3(0x18a, 1), [F, I, I], |a, _, _, _| Want::H(f16_from(f(a), false))));
    c.push(case("v_cvt_f32_f16", v3(0x18b, 1), [H, I, I], |a, _, _, _| fl(f16_to(a & 0xffff))));
    // D.i = (int)floor(S0.f + 0.5) / (int)floor(S0.f); saturation and NaN -> 0 as for
    // V_CVT_I32_F32 (inferred: the guide states it only for that instruction).
    c.push(case("v_cvt_rpi_i32_f32", v3(0x18c, 1), [F, I, I], |a, _, _, _| Want::U((f(a) + 0.5).floor() as i32 as u32)));
    c.push(case("v_cvt_flr_i32_f32", v3(0x18d, 1), [F, I, I], |a, _, _, _| Want::U(f(a).floor() as i32 as u32)));
    let offset = |a: u32| {
        let nibble = (a & 15) as i32;
        let signed = if nibble >= 8 { nibble - 16 } else { nibble };
        signed as f32 / 16.0
    };
    c.push(case("v_cvt_off_f32_i4 e32", vec![0x7e06_1d00, S_ENDPGM], [I, I, I], move |a, _, _, _| {
        Want::U(offset(a).to_bits())
    }));
    let scaled = vop3(0x18e, 1, Mods { omod: 2, clamp: true, ..m0 }, 3, None);
    c.push(case("v_cvt_off_f32_i4 e64 x4 clamp", scaled, [I, I, I], move |a, _, _, _| {
        Want::U((4.0 * offset(a)).clamp(0.0, 1.0).to_bits())
    }));
    for k in 0..4u32 {
        c.push(case(&format!("v_cvt_f32_ubyte{k}"), v3(0x191 + k, 1), [I, I, I], move |a, _, _, _| fl(((a >> (8 * k)) & 0xff) as f32)));
    }
    // Round toward zero (the guide: "ignore the current rounding mode").
    c.push(case("v_cvt_pkrtz_f16_f32", v3(0x12f, 2), [F, F, I], |a, b2, _, _| Want::H2(f16_from(f(a), true), f16_from(f(b2), true))));
    // --- rounding
    c.push(case("v_fract_f32", v3(0x1a0, 1), [F, I, I], |a, _, _, _| {
        // D.f = S0.f + -floor(S0.f); hardware and D3D both keep the result below 1.0.
        let x = f(a);
        let r = x - x.floor();
        Want::F(vec![r, r.min(f(0x3f7f_ffff))])
    }));
    c.push(case("v_trunc_f32", v3(0x1a1, 1), [F, I, I], |a, _, _, _| fl(f(a).trunc())));
    c.push(case("v_ceil_f32", v3(0x1a2, 1), [F, I, I], |a, _, _, _| fl(f(a).ceil())));
    c.push(case("v_rndne_f32", v3(0x1a3, 1), [F, I, I], |a, _, _, _| fl(f(a).round_ties_even())));
    c.push(case("v_floor_f32", v3(0x1a4, 1), [F, I, I], |a, _, _, _| fl(f(a).floor())));
    // --- transcendentals (tolerances: D3D11.3 functional spec minimum precision)
    c.push(case("v_exp_f32", v3(0x1a5, 1), [F, I, I], |a, _, _, _| Want::Approx { v: f(a).exp2(), ulps: 3, abs: 0.0 }));
    c.push(case("v_log_f32", v3(0x1a7, 1), [F, I, I], |a, _, _, _| Want::Approx { v: f(a).log2(), ulps: 3, abs: 2f32.powi(-21) }));
    c.push(case("v_rcp_f32", v3(0x1aa, 1), [F, I, I], |a, _, _, _| Want::Approx { v: 1.0 / f(a), ulps: 1, abs: 0.0 }));
    c.push(case("v_rsq_f32", v3(0x1ae, 1), [F, I, I], |a, _, _, _| Want::Approx { v: 1.0 / f(a).sqrt(), ulps: 2, abs: 0.0 }));
    c.push(case("v_sqrt_f32", v3(0x1b3, 1), [F, I, I], |a, _, _, _| Want::Approx { v: f(a).sqrt(), ulps: 1, abs: 0.0 }));
    c.push(case("v_sin_f32", v3(0x1b5, 1), [Trig, I, I], |a, _, _, _| {
        Want::Approx { v: ((f(a) as f64) * std::f64::consts::TAU).sin() as f32, ulps: 0, abs: 0.0008 }
    }));
    c.push(case("v_cos_f32", v3(0x1b6, 1), [Trig, I, I], |a, _, _, _| {
        Want::Approx { v: ((f(a) as f64) * std::f64::consts::TAU).cos() as f32, ulps: 0, abs: 0.0008 }
    }));
    // --- bit ops
    c.push(case("v_not_b32", v3(0x1b7, 1), [I, I, I], |a, _, _, _| Want::U(!a)));
    c.push(case("v_bfrev_b32", v3(0x1b8, 1), [I, I, I], |a, _, _, _| Want::U(a.reverse_bits())));
    c.push(case("v_ffbh_u32", v3(0x1b9, 1), [I, I, I], |a, _, _, _| Want::U(if a == 0 { u32::MAX } else { a.leading_zeros() })));
    c.push(case("v_ffbl_b32", v3(0x1ba, 1), [I, I, I], |a, _, _, _| Want::U(if a == 0 { u32::MAX } else { a.trailing_zeros() })));
    c.push(case("v_ffbh_i32", v3(0x1bb, 1), [I, I, I], |a, _, _, _| {
        // Count of bits after the sign bit equal to it, from the MSB; -1 when all are equal.
        let x = if (a as i32) < 0 { !a } else { a };
        Want::U(if x == 0 { u32::MAX } else { x.leading_zeros() })
    }));
    let int2: [(&str, u32, fn(u32, u32) -> u32); 23] = [
        ("v_min_i32", 0x111, |a, b| (a as i32).min(b as i32) as u32),
        ("v_max_i32", 0x112, |a, b| (a as i32).max(b as i32) as u32),
        ("v_min_u32", 0x113, |a, b| a.min(b)),
        ("v_max_u32", 0x114, |a, b| a.max(b)),
        ("v_lshrrev_b32", 0x116, |a, b| b >> (a & 31)),
        ("v_ashrrev_i32", 0x118, |a, b| ((b as i32) >> (a & 31)) as u32),
        ("v_lshlrev_b32", 0x11a, |a, b| b << (a & 31)),
        ("v_and_b32", 0x11b, |a, b| a & b),
        ("v_or_b32", 0x11c, |a, b| a | b),
        ("v_xor_b32", 0x11d, |a, b| a ^ b),
        ("v_xnor_b32", 0x11e, |a, b| !(a ^ b)),
        ("v_add_nc_u32", 0x125, |a, b| a.wrapping_add(b)),
        ("v_sub_nc_u32", 0x126, |a, b| a.wrapping_sub(b)),
        ("v_subrev_nc_u32", 0x127, |a, b| b.wrapping_sub(a)),
        ("v_mul_i32_i24", 0x109, |a, b| sext24(a).wrapping_mul(sext24(b)) as u32),
        ("v_mul_hi_i32_i24", 0x10a, |a, b| ((sext24(a) as i64 * sext24(b) as i64) >> 32) as u32),
        ("v_mul_u32_u24", 0x10b, |a, b| (a & 0xff_ffff).wrapping_mul(b & 0xff_ffff)),
        ("v_mul_hi_u32_u24", 0x10c, |a, b| (((a & 0xff_ffff) as u64 * (b & 0xff_ffff) as u64) >> 32) as u32),
        ("v_mul_lo_u32", 0x169, |a, b| a.wrapping_mul(b)),
        ("v_mul_hi_u32", 0x16a, |a, b| ((a as u64 * b as u64) >> 32) as u32),
        ("v_mul_hi_i32", 0x16c, |a, b| ((a as i32 as i64 * b as i32 as i64) >> 32) as u32),
        ("v_bfm_b32", 0x363, |a, b| ((1u32 << (a & 31)).wrapping_sub(1)) << (b & 31)),
        ("v_bcnt_u32_b32", 0x364, |a, b| a.count_ones().wrapping_add(b)),
    ];
    for (n, op, fun) in int2 {
        let doms = if n.contains("sh") || n == "v_bfm_b32" { [Small, I, I] } else { [I, I, I] };
        c.push(case(n, v3(op, 2), doms, move |a, b2, _, _| Want::U(fun(a, b2))));
    }
    let int3: [(&str, u32, fn(u32, u32, u32) -> u32, [Dom; 3]); 21] = [
        ("v_mad_i32_i24", 0x142, |a, b, c| sext24(a).wrapping_mul(sext24(b)).wrapping_add(c as i32) as u32, [I, I, I]),
        ("v_mad_u32_u24", 0x143, |a, b, c| (a & 0xff_ffff).wrapping_mul(b & 0xff_ffff).wrapping_add(c), [I, I, I]),
        ("v_bfe_u32", 0x148, |a, b, c| {
            // D.u = (S0.u >> S1.u[4:0]) & ((1 << S2.u[4:0]) - 1)
            (a >> (b & 31)) & ((1u32 << (c & 31)).wrapping_sub(1))
        }, [I, Small, Small]),
        ("v_bfe_i32", 0x149, |a, b, c| {
            // Signed bit-field extract. The guide's pseudocode omits the sign extension; this
            // follows LLVM AMDGPU's constant folding of V_BFE_I32: width 0 -> 0; if
            // offset + width < 32 sign-extend the field, else arithmetic-shift by offset.
            let (o, w) = (b & 31, c & 31);
            if w == 0 {
                0
            } else if o + w < 32 {
                (((a << (32 - o - w)) as i32) >> (32 - w)) as u32
            } else {
                ((a as i32) >> o) as u32
            }
        }, [I, Small, Small]),
        ("v_bfi_b32", 0x14a, |a, b, c| (a & b) | (!a & c), [I, I, I]),
        ("v_lerp_u8", 0x14d, |a, b, c| {
            let bytes = std::array::from_fn(|i| {
                let sum = u16::from(a.to_le_bytes()[i]) + u16::from(b.to_le_bytes()[i]);
                ((sum + u16::from(c.to_le_bytes()[i] & 1)) / 2) as u8
            });
            u32::from_le_bytes(bytes)
        }, [I, I, I]),
        ("v_alignbit_b32", 0x14e, |a, b, c| ((((a as u64) << 32) | b as u64) >> (c & 31)) as u32, [I, I, Small]),
        ("v_alignbyte_b32", 0x14f, |a, b, c| ((((a as u64) << 32) | b as u64) >> (8 * (c & 3))) as u32, [I, I, Small]),
        ("v_min3_i32", 0x152, |a, b, c| (a as i32).min(b as i32).min(c as i32) as u32, [I, I, I]),
        ("v_min3_u32", 0x153, |a, b, c| a.min(b).min(c), [I, I, I]),
        ("v_max3_i32", 0x155, |a, b, c| (a as i32).max(b as i32).max(c as i32) as u32, [I, I, I]),
        ("v_max3_u32", 0x156, |a, b, c| a.max(b).max(c), [I, I, I]),
        ("v_med3_i32", 0x158, |a, b, c| { let mut v = [a as i32, b as i32, c as i32]; v.sort(); v[1] as u32 }, [I, I, I]),
        ("v_med3_u32", 0x159, |a, b, c| { let mut v = [a, b, c]; v.sort(); v[1] }, [I, I, I]),
        ("v_xor3_b32", 0x178, |a, b, c| a ^ b ^ c, [I, I, I]),
        ("v_perm_b32", 0x344, |a, b, c| {
            // byte_permute({S0, S1}, sel): bytes 0..3 from S1, 4..7 from S0; 8..11 replicate
            // the sign bit of bytes 1, 3, 5, 7; 12 -> 0x00; 13.. -> 0xff.
            let bytes = (((a as u64) << 32) | b as u64).to_le_bytes();
            let pb = |sel: u32| -> u32 {
                match sel {
                    13.. => 0xff,
                    12 => 0,
                    8..=11 => ((bytes[(2 * (sel - 8) + 1) as usize] >> 7) as u32) * 0xff,
                    s => bytes[s as usize] as u32,
                }
            };
            (0..4).map(|k| pb((c >> (8 * k)) & 0xff) << (8 * k)).fold(0, |x, y| x | y)
        }, [I, I, I]),
        ("v_xad_u32", 0x345, |a, b, c| (a ^ b).wrapping_add(c), [I, I, I]),
        ("v_lshl_add_u32", 0x346, |a, b, c| (a << (b & 31)).wrapping_add(c), [I, Small, I]),
        ("v_add_lshl_u32", 0x347, |a, b, c| a.wrapping_add(b) << (c & 31), [I, I, Small]),
        ("v_add3_u32", 0x36d, |a, b, c| a.wrapping_add(b).wrapping_add(c), [I, I, I]),
        ("v_lshl_or_b32", 0x36f, |a, b, c| (a << (b & 31)) | c, [I, Small, I]),
    ];
    for (n, op, fun, doms) in int3 {
        c.push(case(n, v3(op, 3), doms, move |a, b2, c2, _| Want::U(fun(a, b2, c2))));
    }
    c.push(case("v_and_or_b32", v3(0x371, 3), [I, I, I], |a, b2, c2, _| Want::U((a & b2) | c2)));
    c.push(case("v_or3_b32", v3(0x372, 3), [I, I, I], |a, b2, c2, _| Want::U(a | b2 | c2)));
    c.push(case("v_ldexp_f32", v3(0x362, 2), [F, I, I], |a, b2, _, _| {
        // D.f = S0.f * 2^S1.i (exact scaling, rounding only at the end).
        let e = (b2 as i32).clamp(-400, 400);
        fl(((f(a) as f64) * 2f64.powi(e)) as f32)
    }));
    // --- cube map helpers (pseudocode from the guide, VOP3 0x144..0x147)
    let cube = |x: f32, y: f32, z: f32, which: u32| -> f32 {
        let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
        if az >= ax && az >= ay {
            [if z < 0.0 { 5.0 } else { 4.0 }, if z < 0.0 { -x } else { x }, -y, 2.0 * z][which as usize]
        } else if ay >= ax {
            [if y < 0.0 { 3.0 } else { 2.0 }, x, if y < 0.0 { -z } else { z }, 2.0 * y][which as usize]
        } else {
            [if x < 0.0 { 1.0 } else { 0.0 }, if x < 0.0 { z } else { -z }, -y, 2.0 * x][which as usize]
        }
    };
    for (k, n) in ["v_cubeid_f32", "v_cubesc_f32", "v_cubetc_f32", "v_cubema_f32"].iter().enumerate() {
        c.push(case(n, v3(0x144 + k as u32, 3), [F, F, F], move |a, b2, c2, _| fl(cube(f(a), f(b2), f(c2), k as u32))));
    }
    // --- moves and selects
    c.push(case("v_mov_b32", v3(0x181, 1), [I, I, I], |a, _, _, _| Want::U(a)));
    let mut cnd = case("v_cndmask_b32", vop3(0x101, 2, m0, 3, Some(VCC_LO)), [I, I, I], |a, b2, _, vcc| Want::U(if vcc & 1 != 0 { b2 } else { a }));
    cnd.vcc_in = true;
    c.push(cnd);
    // --- carry arithmetic (VOP3B, carry-out in VCC)
    for (n, op, cin) in [("v_add_co_u32", 0x30fu32, false), ("v_sub_co_u32", 0x310, false), ("v_subrev_co_u32", 0x319, false), ("v_add_co_ci_u32", 0x128, true), ("v_sub_co_ci_u32", 0x129, true), ("v_subrev_co_ci_u32", 0x12a, true)] {
        let words = vop3b(op, cin);
        let sub = n.starts_with("v_sub_");
        let rev = n.starts_with("v_subrev");
        c.push(Case {
            name: n.into(),
            words,
            doms: [I, I, I],
            vcc_in: cin,
            out: Out::V3Carry,
            float_in: false,
            want: Box::new(move |a, b2, _, vcc| {
                let ci = if cin { (vcc & 1) as u64 } else { 0 };
                let (x, y) = if rev { (b2 as u64, a as u64) } else { (a as u64, b2 as u64) };
                if sub || rev {
                    let r = x.wrapping_sub(y).wrapping_sub(ci);
                    (Want::U(r as u32), (y + ci > x) as u32)
                } else {
                    let r = x + y + ci;
                    (Want::U(r as u32), (r >> 32) as u32)
                }
            }),
        });
    }
    // --- compares: VOPC writes VCC; VOP3 form with SDST = VCC_LO; CMPX writes EXEC
    let fcmp: [(&str, fn(f32, f32) -> bool); 16] = [
        ("f", |_, _| false),
        ("lt", |a, b| a < b),
        ("eq", |a, b| a == b),
        ("le", |a, b| a <= b),
        ("gt", |a, b| a > b),
        ("lg", |a, b| a < b || a > b),
        ("ge", |a, b| a >= b),
        ("o", |a, b| !a.is_nan() && !b.is_nan()),
        ("u", |a, b| a.is_nan() || b.is_nan()),
        ("nge", |a, b| !(a >= b)),
        ("nlg", |a, b| !(a < b || a > b)),
        ("ngt", |a, b| !(a > b)),
        ("nle", |a, b| !(a <= b)),
        ("neq", |a, b| !(a == b)),
        ("nlt", |a, b| !(a < b)),
        ("tru", |_, _| true),
    ];
    for (k, (cc, fun)) in fcmp.iter().enumerate() {
        let fun = *fun;
        c.push(Case { out: Out::Vcc, ..case(&format!("v_cmp_{cc}_f32"), vopc(k as u32), [F, F, I], move |a, b2, _, _| Want::U(fun(f(a), f(b2)) as u32)) });
        c.push(Case { out: Out::Exec, vcc_in: true, ..case(&format!("v_cmpx_{cc}_f32"), vopc(0x10 + k as u32), [F, F, I], move |a, b2, _, _| Want::U(fun(f(a), f(b2)) as u32)) });
        let hfun = move |a: u32, b2: u32| fun(f16_to(a & 0xffff), f16_to(b2 & 0xffff));
        let hop = if k < 8 { 0xc8 + k as u32 } else { 0xe8 + k as u32 - 8 };
        c.push(Case { out: Out::Vcc, ..case(&format!("v_cmp_{cc}_f16"), vopc(hop), [H, H, I], move |a, b2, _, _| Want::U(hfun(a, b2) as u32)) });
        c.push(Case { out: Out::Exec, vcc_in: true, ..case(&format!("v_cmpx_{cc}_f16"), vopc(hop + 0x10), [H, H, I], move |a, b2, _, _| Want::U(hfun(a, b2) as u32)) });
    }
    let icmp: [(&str, fn(i64, i64) -> bool); 8] = [
        ("f", |_, _| false),
        ("lt", |a, b| a < b),
        ("eq", |a, b| a == b),
        ("le", |a, b| a <= b),
        ("gt", |a, b| a > b),
        ("ne", |a, b| a != b),
        ("ge", |a, b| a >= b),
        ("t", |_, _| true),
    ];
    for (k, (cc, fun)) in icmp.iter().enumerate() {
        let fun = *fun;
        c.push(Case { out: Out::Vcc, ..case(&format!("v_cmp_{cc}_i32"), vop3(0x80 + k as u32, 2, m0, VCC_LO, None), [I, I, I], move |a, b2, _, _| Want::U(fun(a as i32 as i64, b2 as i32 as i64) as u32)) });
        c.push(Case { out: Out::Vcc, ..case(&format!("v_cmp_{cc}_u32"), vopc(0xc0 + k as u32), [I, I, I], move |a, b2, _, _| Want::U(fun(a as i64, b2 as i64) as u32)) });
        c.push(Case { out: Out::Exec, vcc_in: true, ..case(&format!("v_cmpx_{cc}_u32"), vopc(0xd0 + k as u32), [I, I, I], move |a, b2, _, _| Want::U(fun(a as i64, b2 as i64) as u32)) });
        c.push(Case { out: Out::Exec, vcc_in: true, ..case(&format!("v_cmpx_{cc}_i32"), vopc(0x90 + k as u32), [I, I, I], move |a, b2, _, _| Want::U(fun(a as i32 as i64, b2 as i32 as i64) as u32)) });
        // The 16-bit integer blocks encode the six relational predicates.
        if (1..=6).contains(&k) {
            for (ty, base, signed) in [("i16", 0x88, true), ("u16", 0xa8, false)] {
                let widen = move |x: u32| if signed { i64::from(x as i16) } else { i64::from(x as u16) };
                for (prefix, op, out) in [("v_cmp", base, Out::Vcc), ("v_cmpx", base + 0x10, Out::Exec)] {
                    let name = format!("{prefix}_{cc}_{ty}");
                    c.push(Case { out, vcc_in: true, ..case(&name, vopc(op + k as u32), [I, I, I], move |a, b, _, _| {
                        Want::U(fun(widen(a), widen(b)) as u32)
                    }) });
                }
            }
        }
    }
    // LLVM gfx1030 encodings: e64 scalar mask destination and SDWA word selection.
    c.push(Case { out: Out::Vcc, ..case("v_cmp_ge_u16 e64", vop3(0xae, 2, m0, VCC_LO, None), [I, I, I], |a, b, _, _| {
        Want::U((a as u16 >= b as u16) as u32)
    }) });
    // Complementary CMPX instructions must leave all lanes inactive, including the
    // lanes where the second predicate is true but the first already cleared EXEC.
    let mut masked = vopc(0x99);
    masked.pop();
    masked.extend(vop3(0x9e, 2, m0, 126, None)); // v_cmpx_ge_i16_e64 (EXEC_LO)
    c.push(Case { out: Out::Exec, vcc_in: true, ..case("v_cmpx_lt_i16 then ge", masked, [I, I, I], |_, _, _, _| Want::U(0)) });
    for (name, word, out) in [("v_cmp_lt_i16", 0x7d12_02f9, Out::Vcc), ("v_cmpx_lt_i16", 0x7d32_02f9, Out::Exec)] {
        c.push(Case { out, vcc_in: true, ..case(&format!("{name} sdwa"), vec![word, 0x0405_0000, S_ENDPGM], [I, I, I], |a, b, _, _| {
            Want::U((((a >> 16) as i16) < b as i16) as u32)
        }) });
    }
    for (name, op, out) in [("v_cmp_class_f32", 0x88, Out::Vcc), ("v_cmpx_class_f32", 0x98, Out::Exec)] {
        // Classification inspects register bits, including subnormals.
        c.push(Case { out, vcc_in: true, float_in: false, ..case(name, vopc(op), [F, I, I], |a, b2, _, _| {
            // Class bits: 0 sNaN, 1 qNaN, 2 -inf, 3 -normal, 4 -denorm, 5 -0, 6 +0, 7 +denorm,
            // 8 +normal, 9 +inf.
            let x = f(a);
            let cls = if x.is_nan() {
                if a & 0x40_0000 != 0 { 1 } else { 0 }
            } else if x.is_infinite() {
                if x < 0.0 { 2 } else { 9 }
            } else if x == 0.0 {
                if a >> 31 != 0 { 5 } else { 6 }
            } else if is_denorm(a) {
                if a >> 31 != 0 { 4 } else { 7 }
            } else if x < 0.0 {
                3
            } else {
                8
            };
            Want::U((b2 >> cls) & 1)
        }) });
    }
    // --- half-float arithmetic (low halves; results rounded to nearest even)
    let h2: [(&str, u32, fn(f32, f32) -> f32); 6] = [
        ("v_add_f16", 0x132, |a, b| a + b),
        ("v_sub_f16", 0x133, |a, b| a - b),
        ("v_subrev_f16", 0x134, |a, b| b - a),
        ("v_mul_f16", 0x135, |a, b| a * b),
        ("v_max_f16", 0x139, amd_max),
        ("v_min_f16", 0x13a, amd_min),
    ];
    for (n, op, fun) in h2 {
        c.push(case(n, v3(op, 2), [H, H, I], move |a, b2, _, _| Want::H(f16_from(fun(f16_to(a & 0xffff), f16_to(b2 & 0xffff)), false))));
    }
    c.push(case("v_fma_f16", v3(0x34b, 3), [H, H, H], |a, b2, c2, _| {
        let (x, y, z) = (f16_to(a & 0xffff), f16_to(b2 & 0xffff), f16_to(c2 & 0xffff));
        Want::H(f16_from(((x as f64) * (y as f64) + z as f64) as f32, false))
    }));
    c.push(case("v_fmac_f16", v3(0x136, 2), [H, H, H], |a, b2, c2, _| {
        let (x, y, z) = (f16_to(a & 0xffff), f16_to(b2 & 0xffff), f16_to(c2 & 0xffff));
        Want::H(f16_from(((x as f64) * (y as f64) + z as f64) as f32, false))
    }));
    let h1: [(&str, u32, fn(f32) -> f32); 6] = [
        ("v_floor_f16", 0x1db, |x| x.floor()),
        ("v_ceil_f16", 0x1dc, |x| x.ceil()),
        ("v_trunc_f16", 0x1dd, |x| x.trunc()),
        ("v_rndne_f16", 0x1de, |x| x.round_ties_even()),
        ("v_fract_f16", 0x1df, |x| x - x.floor()),
        ("v_sqrt_f16", 0x1d5, |x| x.sqrt()),
    ];
    for (n, op, fun) in h1 {
        c.push(case(n, v3(op, 1), [H, I, I], move |a, _, _, _| {
            let r = fun(f16_to(a & 0xffff));
            let h = f16_from(r, false);
            // fract may round up to 1.0 in f16; hardware keeps it below 1 (0x3bff).
            if n == "v_fract_f16" && h == 0x3c00 { Want::H(0x3bff) } else { Want::H(h) }
        }));
    }
    c.push(case("v_cvt_f16_u16", v3(0x1d0, 1), [I, I, I], |a, _, _, _| Want::H(f16_from((a & 0xffff) as f32, false))));
    c.push(case("v_cvt_f16_i16", v3(0x1d1, 1), [I, I, I], |a, _, _, _| Want::H(f16_from((a as u16 as i16) as f32, false))));
    // Truncating conversion with saturation; NaN -> 0 (inferred from the f32 conversions).
    c.push(case("v_cvt_u16_f16", v3(0x1d2, 1), [H, I, I], |a, _, _, _| Want::U((f16_to(a & 0xffff) as u16) as u32)));
    c.push(case("v_cvt_i16_f16", v3(0x1d3, 1), [H, I, I], |a, _, _, _| Want::U((f16_to(a & 0xffff) as i16) as u16 as u32)));
    // --- packed half floats (VOP3P, default op_sel)
    let pk: [(&str, u32, usize, fn(f32, f32, f32) -> f32); 5] = [
        ("v_pk_fma_f16", 0x0e, 3, |a, b, c| ((a as f64) * (b as f64) + c as f64) as f32),
        ("v_pk_add_f16", 0x0f, 2, |a, b, _| a + b),
        ("v_pk_mul_f16", 0x10, 2, |a, b, _| a * b),
        ("v_pk_min_f16", 0x11, 2, |a, b, _| amd_min(a, b)),
        ("v_pk_max_f16", 0x12, 2, |a, b, _| amd_max(a, b)),
    ];
    for (n, op, ns, fun) in pk {
        c.push(case(n, vop3p(op, ns, 0, 0b111, 0, 0), [H2, H2, H2], move |a, b2, c2, _| {
            let lo = fun(f16_to(a & 0xffff), f16_to(b2 & 0xffff), f16_to(c2 & 0xffff));
            let hi = fun(f16_to(a >> 16), f16_to(b2 >> 16), f16_to(c2 >> 16));
            Want::H2(f16_from(lo, false), f16_from(hi, false))
        }));
    }
    // v_fma_mix_f32 with all-f32 sources, then with src1 = high half (f16) of v1.
    c.push(case("v_fma_mix_f32 f32", vop3p(0x20, 3, 0, 0, 0, 0), [F, F, F], |a, b2, c2, _| mad_any(f(a), f(b2), f(c2))));
    c.push(case("v_fma_mix_f32 hi16", vop3p(0x20, 3, 0b010, 0b010, 0, 0), [F, H2, F], |a, b2, c2, _| mad_any(f(a), f16_to(b2 >> 16), f(c2))));
    c
}

fn case_vectors(c: &Case, index: usize) -> Vec<[u32; 4]> {
    vectors(&c.doms, c.vcc_in, 0x9e37_79b9_7f4a_7c15 ^ index as u64)
        .into_iter()
        .filter(|v| !(c.float_in && v[..3].iter().any(|&x| is_denorm(x))))
        .collect()
}

fn check_result(c: &Case, v: &[u32; 4], result: u32, vcc: u32, exec: u32) -> (bool, Want, u32) {
    let (want, carry) = (c.want)(v[0], v[1], v[2], v[3]);
    let ok = match c.out {
        Out::V3 => matches(result, &want),
        Out::Vcc => matches(vcc & 1, &want) && exec == 1,
        Out::Exec => matches(exec & 1, &want) && vcc == v[3],
        Out::V3Carry => matches(result, &want) && (vcc & 1) == carry,
    };
    (ok, want, carry)
}

/// A one-thread Xbox workgroup has a single active lane. Move the per-group inputs to
/// SGPRs, execute the comparison, and expose SCC through v3 for independent readback.
fn coin_scalar_compares() -> Vec<Case> {
    let predicates: [(&str, fn(i64, i64) -> bool); 6] = [
        ("eq", |a, b| a == b), ("lg", |a, b| a != b), ("gt", |a, b| a > b),
        ("ge", |a, b| a >= b), ("lt", |a, b| a < b), ("le", |a, b| a <= b),
    ];
    let mut cases = Vec::new();
    for (ty, signed, base) in [("i32", true, 0), ("u32", false, 6)] {
        let widen = move |v: u32| if signed { i64::from(v as i32) } else { i64::from(v) };
        for (index, (cc, compare)) in predicates.iter().copied().enumerate() {
            let op = base + index as u32;
            // v_readfirstlane s16,v0; v_readfirstlane s17,v1; s_cmp s16,s17; v_mov v3,SCC.
            let regs = vec![0x7e20_0500, 0x7e22_0501, 0xbf00_1110 | (op << 16), 0x7e06_02fd, S_ENDPGM];
            cases.push(case(&format!("s_cmp_{cc}_{ty}"), regs, [Dom::I; 3], move |a, b, _, _| {
                Want::U(compare(widen(a), widen(b)) as u32)
            }));
            // SIMM16=0xffff represents -1 for a signed comparison and 65535 for unsigned.
            let immediate = vec![0x7e20_0500, 0xb010_ffff | ((3 + op) << 23), 0x7e06_02fd, S_ENDPGM];
            cases.push(case(&format!("s_cmpk_{cc}_{ty}"), immediate, [Dom::I; 3], move |a, _, _, _| {
                Want::U(compare(widen(a), if signed { -1 } else { 65535 }) as u32)
            }));
        }
    }
    cases
}

// The launcher supplies an isolated directory. The Python wrapper packages the ISA into
// Xbox shader containers and Coin Run returns four words per input through GPU readback.
#[test]
#[ignore = "run with tools/homebrew/play-coins.sh --rdna2-test"]
fn coin_run_valu_conformance() {
    use std::collections::BTreeSet;
    use std::io::{BufReader, BufWriter, Read, Write};
    use std::{env, fs::File, path::PathBuf, process::Command};

    let dir = PathBuf::from(env::var_os("TOMBOX_COIN_PROBE_DIR").expect("homebrew scratch directory"));
    let mut cases = cases();
    cases.extend(coin_scalar_compares());
    let mut exercised = BTreeSet::new();
    let mut inputs = BufWriter::new(File::create(dir.join("cases.bin")).unwrap());
    inputs.write_all(&(cases.len() as u32).to_le_bytes()).unwrap();
    let mut all_vectors = Vec::with_capacity(cases.len());
    for (index, c) in cases.iter().enumerate() {
        let isa: Vec<_> = c.words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let name = c.name.split_whitespace().next().unwrap();
        assert!(tombox_xshader::rdna2::disassemble(&isa).iter().any(|ins| ins.mnemonic == name), "{}", c.name);
        exercised.insert(name.to_owned());
        let vecs = case_vectors(c, index);
        assert!(!vecs.is_empty(), "{}", c.name);
        for word in [c.words.len() as u32, vecs.len() as u32].iter().chain(&c.words) {
            inputs.write_all(&word.to_le_bytes()).unwrap();
        }
        for v in &vecs {
            for word in v {
                inputs.write_all(&word.to_le_bytes()).unwrap();
            }
        }
        all_vectors.push(vecs);
    }
    inputs.flush().unwrap();
    drop(inputs);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/homebrew/rdna2_fixture.py");
    let status = Command::new("python3").arg(script).arg(&dir).status().expect("run Coin Run");
    assert!(status.success(), "Coin Run failed ({status})");

    let mut results = BufReader::new(File::open(dir.join("results.bin")).expect("Coin Run readback"));
    let mut failures = 0;
    let mut total = 0;
    for (c, vecs) in cases.iter().zip(all_vectors) {
        let mut bad = 0;
        for v in &vecs {
            let mut row = [0u8; 16];
            results.read_exact(&mut row).unwrap_or_else(|e| panic!("{}: missing GPU readback: {e}", c.name));
            let [value, vcc, exec, marker] = std::array::from_fn(|i| u32::from_le_bytes(row[i*4..i*4+4].try_into().unwrap()));
            let (ok, want, carry) = check_result(c, v, value, vcc, exec);
            if !ok || marker != 0x5244_4e41 {
                if bad == 0 {
                    eprintln!("{}: input {v:08x?}, got {value:08x}, VCC {vcc}, EXEC {exec}, marker {marker:08x}, expected {want:?} (carry {carry})", c.name);
                }
                bad += 1;
            }
        }
        total += vecs.len();
        if bad > 0 {
            eprintln!("{}: {bad}/{} mismatches", c.name, vecs.len());
            failures += 1;
        }
    }
    assert_eq!(results.read(&mut [0; 1]).unwrap(), 0, "unexpected extra GPU readback");
    assert_eq!(failures, 0, "Coin Run ALU cases failed");
    let tracked = include_str!("../../../tools/progress/valu-mnemonics.txt");
    for name in tracked.lines() {
        assert!(exercised.contains(name), "missing Coin Run coverage for tracked mnemonic {name}");
    }
    println!("Coin Run verified {} Xbox shader cases ({} ALU mnemonics) over {total} input vectors",
             cases.len(), exercised.len());
    let memory = std::fs::read_to_string(dir.join("memory-mnemonics.txt")).expect("companion memory suite results");
    let memory: BTreeSet<_> = memory.lines().collect();
    for row in include_str!("../../../tools/progress/coin-rdna2-coverage.tsv").lines().skip(1) {
        let fields: Vec<_> = row.split('\t').collect();
        let [name, _, game_checked, memory_checked] = fields[..] else { panic!("invalid coverage row: {row}") };
        if game_checked == "true" {
            assert!(exercised.contains(name), "lost Coin Run ALU coverage for {name}");
        }
        if memory_checked == "true" {
            assert!(memory.contains(name), "lost memory probe coverage for {name}");
        }
    }
    let retail: BTreeSet<_> = include_str!("../../../tools/progress/mnemonics.txt")
        .lines().map(|line| line.split_whitespace().next().unwrap()).collect();
    let names: BTreeSet<_> = exercised.iter().map(String::as_str).chain(memory.iter().copied()).chain(retail.iter().copied()).collect();
    let mut coverage = BufWriter::new(File::create(dir.join("coverage.tsv")).unwrap());
    writeln!(coverage, "mnemonic\tretail_observed\tcoin_run_alu\tmemory_probe").unwrap();
    for name in names {
        writeln!(coverage, "{name}\t{}\t{}\t{}", retail.contains(name), exercised.contains(name), memory.contains(name)).unwrap();
    }
    coverage.flush().unwrap();
    let directly_checked = retail.iter().filter(|name| exercised.contains(**name) || memory.contains(**name)).count();
    println!("Direct ALU and memory checks cover {directly_checked}/{} retail-observed instructions; {} still lack an instruction-level reference case.",
             retail.len(), retail.len() - directly_checked);
}

#[cfg(windows)]
#[test]
fn valu_conformance() {
    let export = std::env::var_os("TOMBOX_VALU_MNEMONICS");
    if !dxc::available() {
        assert!(export.is_none(), "GPU coverage export requires DXC");
        eprintln!("SKIPPED: DXC not available");
        return;
    }
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            assert!(export.is_none(), "GPU coverage export requires a D3D12 device: {e}");
            eprintln!("SKIPPED: no D3D12 device ({e})");
            return;
        }
    };
    let only = std::env::var("TOMBOX_VALU_ONLY").ok();
    let stride = valu_probe_stride(NV) as usize;
    let mut failed: Vec<String> = Vec::new();
    let mut report = String::new();
    let mut verified = std::collections::BTreeSet::new();
    for (ci, c) in cases().into_iter().enumerate() {
        if only.as_deref().is_some_and(|o| !c.name.contains(o)) {
            continue;
        }
        let hlsl = match translate_valu_probe(&c.words, NV) {
            Ok(h) => h,
            Err(e) => {
                report.push_str(&format!("{:28} UNSUPPORTED by translator: {e}\n", c.name));
                failed.push(format!("{}: translator refused ({e})", c.name));
                continue;
            }
        };
        let dxil = match dxc::compile(&hlsl, "cs_6_0") {
            Ok(d) => d,
            Err(e) => {
                failed.push(format!("{}: DXC failed: {e}\n{hlsl}", c.name));
                continue;
            }
        };
        let vecs = case_vectors(&c, ci);
        // The probe runs 64-thread groups and its buffer is not bounds-checked: size it for
        // every thread launched (the padding threads compute on zeros and are ignored).
        let groups = vecs.len().div_ceil(64) as u32;
        let mut data = vec![0u32; groups as usize * 64 * stride];
        for (i, v) in vecs.iter().enumerate() {
            let o = i * stride;
            data[o..o + 3].copy_from_slice(&v[..3]);
            data[o + 3] = v[2]; // MAC forms accumulate into v3
            data[o + NV as usize] = v[3]; // vcc_lo
        }
        gpu.run(&dxil, &mut data, groups).expect("GPU run");
        let mut bad = 0usize;
        let mut examples = Vec::new();
        for (i, v) in vecs.iter().enumerate() {
            let o = i * stride;
            let (vres, vcc, exec) = (data[o + 3], data[o + NV as usize], data[o + NV as usize + 1]);
            let (ok, want, carry) = check_result(&c, v, vres, vcc, exec);
            if !ok {
                bad += 1;
                if examples.len() < 4 {
                    let got = match c.out {
                        Out::Vcc => vcc,
                        Out::Exec => exec,
                        Out::V3Carry => { examples.push(format!("in {:08x} {:08x} {:08x} vcc {} -> got {vres:08x} carry {} want {want:?} carry {carry}", v[0], v[1], v[2], v[3], vcc & 1)); continue; }
                        Out::V3 => vres,
                    };
                    examples.push(format!(
                        "in {:08x} {:08x} {:08x} vcc {} ({:e}, {:e}, {:e}) -> got {got:08x} ({:e}) want {want:?}",
                        v[0], v[1], v[2], v[3], f(v[0]), f(v[1]), f(v[2]), f(got)
                    ));
                }
            }
        }
        report.push_str(&format!("{:28} {:5} vectors, {:5} mismatches\n", c.name, vecs.len(), bad));
        if bad == 0 && !vecs.is_empty() {
            let isa: Vec<u8> = c.words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let name = tombox_xshader::rdna2::disassemble(&isa)[0].mnemonic.clone();
            assert_eq!(Some(name.as_str()), c.name.split_whitespace().next(), "case {}", c.name);
            verified.insert(name);
        }
        if bad > 0 {
            for e in &examples {
                report.push_str(&format!("      {e}\n"));
            }
            failed.push(format!("{}: {bad}/{} mismatches", c.name, vecs.len()));
        }
    }
    eprintln!("{report}");
    if let Ok(p) = std::env::var("TOMBOX_VALU_REPORT") {
        let _ = std::fs::write(p, &report);
    }
    assert!(failed.is_empty(), "{} case(s) failed:\n{}", failed.len(), failed.join("\n"));
    if let Some(path) = export {
        assert!(only.is_none() && !verified.is_empty(), "coverage export requires the full GPU suite");
        let lines = verified.into_iter().collect::<Vec<_>>().join("\n");
        std::fs::write(path, format!("{lines}\n")).expect("write VALU coverage");
    }
}
