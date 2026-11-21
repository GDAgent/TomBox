//! CPU f64 comparisons against both machine encodings, including unordered
//! operands and register pairs differing only in their low word.
use super::{case, Case, Dom, Out, S_ENDPGM};

pub(super) fn cases() -> Vec<Case> {
    let names = [
        "f", "lt", "eq", "le", "gt", "lg", "ge", "o", "u", "nge", "nlg", "ngt", "nle", "neq",
        "nlt", "tru",
    ];
    let mut cases = Vec::new();
    for cmpx in [false, true] {
        let prefix = if cmpx { "v_cmpx" } else { "v_cmp" };
        for (pred, name) in names.into_iter().enumerate() {
            let op = 0x20 + u32::from(cmpx) * 16 + pred as u32;
            for extended in [false, true] {
                let mut words = vec![0x7e08_0302, 0x7e0a_0300];
                if extended {
                    // -abs(v[0:1]) vs v[4:5].
                    words.extend([
                        0xd400_0100 | op << 16 | if cmpx { 126 } else { 106 },
                        0x2002_0900,
                    ]);
                } else {
                    words.push(0x7c00_0900 | op << 17);
                }
                words.push(S_ENDPGM);
                cases.push(Case {
                    out: if cmpx { Out::Exec } else { Out::Vcc },
                    vcc_in: true,
                    ..case(
                        &format!("{prefix}_{name}_f64 e{}", if extended { 64 } else { 32 }),
                        words,
                        [Dom::I; 3],
                        move |a, b, c, _| {
                            let x = f64::from_bits(u64::from(a) | u64::from(b) << 32);
                            let x = if extended { -x.abs() } else { x };
                            let y = f64::from_bits(u64::from(c) | u64::from(a) << 32);
                            let result = match pred {
                                0 => false,
                                1 => x < y,
                                2 => x == y,
                                3 => x <= y,
                                4 => x > y,
                                5 => x < y || x > y,
                                6 => x >= y,
                                7 => !x.is_nan() && !y.is_nan(),
                                8 => x.is_nan() || y.is_nan(),
                                9 => !(x >= y),
                                10 => !(x < y || x > y),
                                11 => !(x > y),
                                12 => !(x <= y),
                                13 => x != y,
                                14 => !(x < y),
                                15 => true,
                                _ => unreachable!(),
                            };
                            super::Want::U(result.into())
                        },
                    )
                });
            }
        }
        let op = if cmpx { 0xb8 } else { 0xa8 };
        cases.push(Case {
            out: if cmpx { Out::Exec } else { Out::Vcc },
            vcc_in: true,
            ..case(
                &format!("{prefix}_class_f64"),
                vec![0x7c00_0500 | op << 17, S_ENDPGM],
                [Dom::I; 3],
                |a, b, mask, _| {
                    use std::num::FpCategory::*;
                    let x = f64::from_bits(u64::from(b) << 32 | u64::from(a));
                    let class = match (x.classify(), x.is_sign_negative()) {
                        (Nan, _) => u32::from(b & 0x80000 != 0),
                        (Infinite, true) => 2,
                        (Normal, true) => 3,
                        (Subnormal, true) => 4,
                        (Zero, true) => 5,
                        (Zero, false) => 6,
                        (Subnormal, false) => 7,
                        (Normal, false) => 8,
                        (Infinite, false) => 9,
                    };
                    super::Want::U((mask >> class) & 1)
                },
            )
        });
    }
    // Source literals occupy the high word; inline ROM constants supply all 64 bits.
    for (src, literal, bits) in [
        (255, Some(0x3ff0_0001), 0x3ff0_0001_0000_0000u64),
        (248, None, 0x3fc4_5f30_6dc9_c882),
    ] {
        let mut words = vec![0x7c00_0000 | 0x22 << 17 | src];
        words.extend(literal);
        words.push(S_ENDPGM);
        cases.push(Case {
            out: Out::Vcc,
            ..case(
                "v_cmp_eq_f64 constant",
                words,
                [Dom::I; 3],
                move |lo, hi, _, _| {
                    super::Want::U(u32::from(u64::from(lo) | (u64::from(hi) << 32) == bits))
                },
            )
        });
    }
    cases
}

pub(super) fn edge_cases() -> Vec<[u32; 4]> {
    let mut rows = Vec::new();
    for hi in [
        0,
        0x8000_0000,
        0x0010_0000,
        0x8010_0000,
        0x3ff0_0000,
        0xbff0_0000,
        0x7fef_ffff,
        0x7ff0_0000,
        0xfff0_0000,
        0x7ff8_0000,
        0xfff8_0000,
    ] {
        for lo in [0, 1, hi, u32::MAX] {
            for bit in 0..10 {
                rows.push([lo, hi, 1 << bit, 1]);
            }
            rows.push([lo, hi, lo, 0]);
        }
    }
    rows.extend([[0, 0x3ff0_0001, 0, 0], [0x6dc9_c882, 0x3fc4_5f30, 0, 0]]);
    rows
}
