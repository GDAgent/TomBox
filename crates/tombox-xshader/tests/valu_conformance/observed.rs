//! Coin Run probes that need an Xbox shader container, embedded data, or group-shared memory.
use super::{case, f, f16_from, f16_to, mad_any, vop3p, Case, Dom, Want, S_ENDPGM};

const WAIT: u32 = 0xbf8c_0000;
fn vmov(dst: u32, src: u32) -> u32 {
    0x7e00_0200 | (dst << 17) | src
}

fn ds(op: u32, offsets: u32, addr: u32, data0: u32, data1: u32, dst: u32) -> [u32; 2] {
    [
        0xd800_0000 | (op << 18) | offsets,
        addr | (data0 << 8) | (data1 << 16) | (dst << 24),
    ]
}

fn lds_cases(cases: &mut Vec<Case>) {
    type Model = fn(u32, u32) -> u32;
    let atomics: &[(&str, u32, Model)] = &[
        ("ds_add_u32", 0x00, u32::wrapping_add),
        ("ds_min_u32", 7, u32::min),
        ("ds_max_u32", 8, u32::max),
        ("ds_or_b32", 10, |a, b| a | b),
        ("ds_add_rtn_u32", 0x20, u32::wrapping_add),
    ];
    // v8 addresses LDS byte 32. Distinct offsets and independent reads check scaling.
    let base = vmov(8, 0x80 + 32);
    for &(name, op, model) in atomics {
        let mut words = vec![base];
        words.extend(ds(0x0d, 12, 8, 0, 0, 0));
        words.extend([WAIT]);
        words.extend(ds(op, 12, 8, 1, 0, 4));
        words.push(WAIT);
        words.extend(ds(0x36, 12, 8, 0, 0, 3));
        words.push(WAIT);
        if op == 0x20 {
            // Mix the returned old word with the final value; dropping either is observable.
            words.push(0x3a06_0903); // v_xor_b32 v3,v3,v4
        }
        words.push(S_ENDPGM);
        cases.push(case(name, words, [Dom::I; 3], move |a, b, _, _| {
            Want::U(model(a, b) ^ if op == 0x20 { a } else { 0 })
        }));
    }
    for (name, op, offsets, bytes) in [
        ("ds_write_b32", 0x0d, 16, vec![16]),
        ("ds_write_b64", 0x4d, 16, vec![16, 20]),
        ("ds_write2_b32", 0x0e, 0x0502, vec![8, 20]),
        ("ds_write2st64_b32", 0x0f, 0x0502, vec![512, 1280]),
        ("ds_write2_b64", 0x4e, 0x0502, vec![16, 20, 40, 44]),
    ] {
        for (k, byte) in bytes.into_iter().enumerate() {
            let mut words = vec![base, vmov(4, 0x9d)];
            // First pair v[0:1], second pair v[3:4] = (c,29).
            words.extend(ds(op, offsets, 8, 0, 3, 0));
            words.push(WAIT);
            words.extend(ds(0x36, byte, 8, 0, 0, 3));
            words.extend([WAIT, S_ENDPGM]);
            cases.push(case(
                &format!("{name} word {k}"),
                words,
                [Dom::I; 3],
                move |a, b, c, _| {
                    let values = if op == 0x0e || op == 0x0f {
                        [a, c, 0, 0]
                    } else {
                        [a, b, c, 29]
                    };
                    Want::U(values[k])
                },
            ));
        }
    }
    for (name, op, offsets, bytes) in [
        ("ds_read_b32", 0x36, 20, vec![20]),
        ("ds_read2_b32", 0x37, 0x0502, vec![8, 20]),
        ("ds_read2st64_b32", 0x38, 0x0502, vec![512, 1280]),
    ] {
        for k in 0..bytes.len() {
            let mut words = vec![base];
            for (src, &byte) in bytes.iter().enumerate() {
                words.extend(ds(0x0d, byte, 8, src as u32, 0, 0));
            }
            words.push(WAIT);
            words.extend(ds(op, offsets, 8, 0, 0, 4));
            words.extend([WAIT, vmov(3, 260 + k as u32), S_ENDPGM]);
            cases.push(case(
                &format!("{name} word {k}"),
                words,
                [Dom::I; 3],
                move |a, b, _, _| Want::U([a, b][k]),
            ));
        }
    }
    for (name, read_half) in [("ds_read_u16", true), ("ds_write_b16", false)] {
        for offset in [0, 2] {
            let mut words = vec![base];
            words.extend(ds(0x0d, 0, 8, 0, 0, 0));
            words.push(WAIT);
            if read_half {
                words.extend(ds(0x3c, offset, 8, 0, 0, 3));
            } else {
                words.extend(ds(0x1f, offset, 8, 1, 0, 0));
                words.push(WAIT);
                words.extend(ds(0x36, 0, 8, 0, 0, 3));
            }
            words.extend([WAIT, S_ENDPGM]);
            cases.push(case(
                &format!("{name} byte {offset}"),
                words,
                [Dom::I; 3],
                move |a, b, _, _| {
                    let shift = 8 * offset;
                    Want::U(if read_half {
                        (a >> shift) & 65535
                    } else {
                        (a & !(65535 << shift)) | ((b & 65535) << shift)
                    })
                },
            ));
        }
    }
}

fn code_loads(cases: &mut Vec<Case>) {
    // The packager patches the literal to the table's actual displacement from GETPC.
    let value = |i: u32| 0x791b_8dc5u32.wrapping_mul(i + 1);
    for (name, op, flat) in [
        ("s_load_dword", 0, false),
        ("s_load_dwordx2", 1, false),
        ("s_load_dwordx4", 2, false),
        ("s_load_dwordx8", 3, false),
        ("flat_load_dword", 0, true),
        ("flat_load_dwordx2", 1, true),
        ("flat_load_dwordx4", 2, true),
        ("s_getpc_b64", 0, false),
    ] {
        for k in 0..1 << op {
            let mut words = vec![
                0xbe90_1f00,
                0x8010_ff10,
                0xc0de_0000, // s16 = PC + table displacement
                0x7e24_0500,
                0x8712_8712,
                0x8f12_8212,
                0x8010_1210,
            ]; // s16 += (a & 7) * 4
            if flat {
                words.extend([
                    vmov(8, 16),
                    vmov(9, 17),
                    0xdc30_0004 | op << 18,
                    0x187d_0008,
                    WAIT,
                    vmov(3, 256 + 24 + k),
                ]);
            } else {
                words.extend([0xf400_0808 | op << 18, 0xfa00_0004, WAIT, vmov(3, 32 + k)]);
            }
            words.push(S_ENDPGM);
            let mut probe = case(
                &format!("{name} word {k}"),
                words,
                [Dom::I; 3],
                move |a, _, _, _| Want::U(value((a & 7) + 1 + k)),
            );
            probe.table = (0..16).map(value).collect();
            cases.push(probe);
        }
    }
}

pub(super) fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    lds_cases(&mut cases);
    code_loads(&mut cases);
    control_cases(&mut cases);
    group_cases(&mut cases);
    cache_cases(&mut cases);
    for (name, instruction) in [("s_nop", 0xbf80_0003), ("v_nop", 0x7e00_0000)] {
        let mut probe = case(
            name,
            vec![
                0xbf07_806a,
                vmov(3, 256),
                instruction,
                0xbeea_03fd,
                S_ENDPGM,
            ],
            [Dom::I; 3],
            |a, _, _, _| Want::U(a),
        );
        probe.vcc_in = true;
        probe.out = super::Out::V3Carry;
        probe.want = Box::new(|a, _, _, flag| (Want::U(a), flag));
        cases.push(probe);
    }
    for (name, word, expected) in [
        ("v_movrels_b32", 0x7e06_8700, true),
        ("v_movreld_b32", 0x7e02_8500, false),
    ] {
        // M0 = c & 1. Relative writes target either v1 or v2; read v2 afterward.
        let mut words = vec![0x7e20_0502, 0x8710_8110, 0xbefc_0310, word];
        if !expected {
            words.push(vmov(3, 258));
        }
        words.push(S_ENDPGM);
        cases.push(case(name, words, [Dom::I; 3], move |a, b, c, _| {
            Want::U(if expected {
                [a, b][(c & 1) as usize]
            } else if c & 1 != 0 {
                a
            } else {
                c
            })
        }));
    }
    for (name, lane) in [("v_writelane_b32", 5), ("v_readlane_b32", 7)] {
        let words = vec![
            0x7e20_0500,
            0x7e22_0501,
            0xd761_0009,
            0x0201_0a10,
            0xd761_0009,
            0x0201_0e11,
            0xd760_0012,
            0x0201_0109 | lane << 9,
            vmov(3, 18),
            S_ENDPGM,
        ];
        cases.push(case(name, words, [Dom::I; 3], move |a, b, _, _| {
            Want::U(if lane == 5 { a } else { b })
        }));
    }
    cases.push(case(
        "v_madak_f32",
        vec![0x4206_0300, 0x3f40_0000, S_ENDPGM],
        [Dom::F; 3],
        |a, b, _, _| mad_any(f(a), f(b), 0.75),
    ));
    cases.push(case(
        "v_rcp_iflag_f32 numeric result",
        vec![0x7e06_5700, S_ENDPGM],
        [Dom::F; 3],
        |a, _, _, _| Want::Approx {
            v: 1.0 / f(a),
            ulps: 1,
            abs: 0.0,
        },
    ));
    for (name, op) in [("v_fma_mixlo_f16", 0x21), ("v_fma_mixhi_f16", 0x22)] {
        cases.push(case(
            name,
            vop3p(op, 3, 0, 7, 0, 0),
            [Dom::H2; 3],
            move |a, b, c, _| {
                let [x, y, z] = [a, b, c].map(|v| f16_to(v & 65535));
                let result = f16_from(x.mul_add(y, z), false);
                if op == 0x21 {
                    Want::H2(result, c >> 16)
                } else {
                    Want::H2(c & 65535, result)
                }
            },
        ));
    }
    cases
}

fn control_cases(cases: &mut Vec<Case>) {
    // Two result paths. A taken early ENDPGM must leave the first record in the UAV.
    let early = vec![
        vmov(3, 256),
        vmov(4, 0x80),
        vmov(5, 0x81),
        vmov(6, 255),
        0x5244_4e41,
        0xe078_2010,
        0x8003_0307,
        0xbf07_806a,
        0xbf84_0001,
        S_ENDPGM,
        vmov(3, 257),
        S_ENDPGM,
    ];
    let mut p = case(
        "s_endpgm conditional termination",
        early,
        [Dom::I; 3],
        |a, b, _, vcc| Want::U(if vcc != 0 { a } else { b }),
    );
    p.vcc_in = true;
    cases.push(p);
    for (name, wait) in [("s_waitcnt", WAIT), ("s_waitcnt_vscnt", 0xbbfd_0000)] {
        cases.push(case(
            name,
            vec![
                0xe070_2010,
                0x8003_0107,
                wait,
                0xe030_2010,
                0x8003_0307,
                WAIT,
                S_ENDPGM,
            ],
            [Dom::I; 3],
            |_, b, _, _| Want::U(b),
        ));
    }
    for (name, op, nand) in [
        ("s_and_saveexec_b64", 0x24, false),
        ("s_nand_saveexec_b64", 0x29, true),
    ] {
        // Seed old EXEC and the source independently. Pack saved EXEC, new EXEC
        // and SCC into one result before re-enabling vector writes.
        let words = vec![
            0x7e20_0500,
            0x8710_8110,
            0xbe91_0380,
            0xbefe_036a,
            0xbe92_0010 | op << 8,
            0xbe94_037e,
            0xbe95_03fd,
            0x8714_8114, // retain this lane only after saving SCC
            0x8f14_8114,
            0x8f15_8215,
            0x8814_1214,
            0x8814_1514,
            0xbefe_0381,
            vmov(3, 20),
            S_ENDPGM,
        ];
        let mut p = case(name, words, [Dom::I; 3], move |a, _, _, old| {
            let new = (old & a & 1) ^ u32::from(nand);
            Want::U(old | (new << 1) | (u32::from(nand || new != 0) << 2))
        });
        p.vcc_in = true;
        cases.push(p);
    }
    // Jump over the write of b. SWAPPC also supplies the address of the skipped
    // instruction; subtract the original GETPC value to verify its byte offset.
    for (name, jump) in [("s_setpc_b64", 0xbe80_2010), ("s_swappc_b64", 0xbe92_2110)] {
        let mut words = vec![
            vmov(3, 256),
            0xbe94_1f00,
            0xbe90_1f00,
            0x8010_8c10,
            jump,
            vmov(3, 257),
        ];
        if name == "s_swappc_b64" {
            words.extend([0x8092_1412, 0x3a06_0612]); // s18=link-s20; v3 ^= s18
        }
        words.push(S_ENDPGM);
        cases.push(case(name, words, [Dom::I; 3], move |a, _, _, _| {
            Want::U(if name == "s_swappc_b64" { a ^ 12 } else { a })
        }));
    }
    for (name, reg) in [("s_setreg_imm32_b32", 20), ("s_getreg_b32", 21)] {
        // Initialize both hardware words, replace only bits 15:8 in one,
        // then read the full word. A wrong selector or exhausted literal fails.
        cases.push(case(
            name,
            vec![
                0x7e20_0500,
                0x7e22_0501,
                0xb990_f814,
                0xbf80_0000,
                0xb991_f815,
                0xbf80_0000,
                0xba80_3a00 | reg,
                0x53,
                0xbf80_0000,
                0xb912_f800 | reg,
                vmov(3, 18),
                S_ENDPGM,
            ],
            [Dom::I; 3],
            move |a, b, _, _| Want::U(((if reg == 20 { a } else { b }) & !0xff00) | 0x5300),
        ));
    }
    cases.push(case(
        "v_fmaak_f16",
        vec![0x7006_0300, 0x3900, S_ENDPGM],
        [Dom::H; 3],
        |a, b, _, _| {
            Want::H(f16_from(
                f16_to(a & 65535).mul_add(f16_to(b & 65535), 0.625),
                false,
            ))
        },
    ));
}

fn group_cases(cases: &mut Vec<Case>) {
    type Model = fn(u32, u32, u32) -> u32;
    let probes: &[(&str, u32, &[u32], Model)] = &[
        // v9 = input xor local ID. All 32 lanes see a different value.
        (
            "v_readfirstlane_b32",
            32,
            &[0x3a12_2900, 0x7e20_0509, 0x7e06_0210],
            |a, _, _| a,
        ),
        (
            "v_mbcnt_lo_u32_b32",
            32,
            &[0xd765_0003, 0x0202_0300],
            |a, b, _| {
                let lane = a & 31;
                b.wrapping_add((a & ((1u32 << lane) - 1)).count_ones())
            },
        ),
        // Lane 0..15 uses the opposite 16-lane row. Two independent selectors.
        (
            "v_permlanex16_b32",
            32,
            &[
                0x3a12_2900,
                0x7e20_0501,
                0x7e22_0502,
                0xd778_0003,
                0x0044_2109,
            ],
            |a, b, c| {
                let lane = a & 31;
                let selector = if lane & 8 == 0 { b } else { c };
                a ^ ((16 ^ (lane & 16)) + ((selector >> (4 * (lane & 7))) & 15))
            },
        ),
    ];
    for &(name, threads, instructions, model) in probes {
        let mut words = instructions.to_vec();
        words.push(S_ENDPGM);
        let mut p = case(name, words, [Dom::I; 3], move |a, b, c, _| {
            Want::U(model(a, b, c))
        });
        p.group_size = threads;
        cases.push(p);
    }
    // A wave32 has no lanes above bit 31. On a wave64 adapter, the optional
    // full-width run checks the nonzero upper prefix against the CPU as well.
    let wave64 = std::env::var_os("TOMBOX_PROBE_WAVE64").is_some();
    let mut hi = case(
        "v_mbcnt_hi_u32_b32",
        vec![0xd766_0003, 0x0202_0300, S_ENDPGM],
        [Dom::I; 3],
        move |a, b, _, _| {
            let bits = if wave64 {
                (a & 63).saturating_sub(32)
            } else {
                0
            };
            Want::U(b.wrapping_add((a & ((1u32 << bits) - 1)).count_ones()))
        },
    );
    hi.group_size = if wave64 { 64 } else { 32 };
    cases.push(hi);
    // Exchange values between lanes 64 positions apart. Every group spans
    // multiple host waves on wave32 and wave64 GPUs.
    let mut words = vec![0x3410_2882, 0x3a12_2900]; // v8=local ID*4, v9=a xor local ID
    words.extend(ds(0x0d, 0, 8, 9, 0, 0));
    words.extend([WAIT, 0xbf8a_0000, 0x3a10_10ff, 256]); // flip byte-address bit 8
    words.extend(ds(0x36, 0, 8, 0, 0, 3));
    words.extend([WAIT, S_ENDPGM]);
    let mut p = case(
        "s_barrier cross-wave LDS exchange",
        words,
        [Dom::I; 3],
        |a, _, _, _| Want::U(a ^ ((a & 127) ^ 64)),
    );
    p.group_size = 128;
    cases.push(p);
}

// The three cache operations have no portable readback of their hardware state.
// Record only the observable contract each fixture checks.
pub(super) fn cache_check(name: &str) -> Option<&'static str> {
    match name {
        "buffer_gl0_inv" | "buffer_gl1_inv" => Some("uav_visibility"),
        "s_inst_prefetch" => Some("state_preserved"),
        _ => None,
    }
}

fn cache_cases(cases: &mut Vec<Case>) {
    for mode in 1..=3 {
        let mut p = case(
            &format!("s_inst_prefetch {mode} lines"),
            vec![
                0xbf07_806a,
                vmov(3, 256),
                0xbfa0_0000 | mode,
                0xbeea_03fd,
                S_ENDPGM,
            ],
            [Dom::I; 3],
            |a, _, _, _| Want::U(a),
        );
        p.vcc_in = true;
        p.out = super::Out::V3Carry;
        p.want = Box::new(|a, _, _, vcc| (Want::U(a), vcc));
        cases.push(p);
    }
    for (name, op) in [("buffer_gl0_inv", 0x71), ("buffer_gl1_inv", 0x72)] {
        // All lanes have read a. The lane 64 places away from the reader
        // publishes b to the same UAV address before the reader fetches it again.
        let inv = 0xe000_0000 | op << 18;
        let mut p = case(
            name,
            vec![
                0x3a2c_2aff,
                64, // v22 = selected reader ID xor 64
                inv,
                0,
                WAIT,
                0xbf8a_0000, // finish initial reads, rendezvous
                0x7da4_2d14, // v_cmpx_eq_u32 v20,v22
                0xe070_2000,
                0x8003_0107, // selected writer stores b
                0xbefe_043c, // restore the workgroup fixture's original EXEC mask
                0xbbfd_0000,
                inv,
                0,
                WAIT,
                0xbf8a_0000,
                inv,
                0,
                WAIT, // acquire, then reload from the same address
                0xe030_2000,
                0x8003_0307,
                WAIT,
                0x3a06_0103, // combine the old and new values
                S_ENDPGM,
            ],
            [Dom::I; 3],
            |a, b, _, _| Want::U(a ^ b),
        );
        p.group_size = 128;
        cases.push(p);
    }
}
