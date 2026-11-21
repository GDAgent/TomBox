//! Opcode-table regressions: entries checked against AMD's machine-readable ISA
//! specifications for RDNA1 (GFX10.1) and RDNA2 (GFX10.3) (GPUOpen, `amdgpu_isa_rdna{1,2}.xml`;
//! the two agree for every entry below). `tools/dev/isa_table_check.py` repeats the full
//! comparison when the XML files are available.

use tuxbox_xshader::rdna2::disassemble;

fn mnemonic(words: &[u32]) -> String {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    disassemble(&isa)[0].mnemonic.clone()
}

fn vopc(op: u32) -> String {
    mnemonic(&[(0b0111110 << 25) | (op << 17) | (1 << 9) | 256])
}

#[test]
fn vopc_u32_cmpx_block_is_not_half_float() {
    // 0xd0..0xd7 are V_CMPX_*_U32 (write EXEC); they were decoded as f16 compares.
    let want = ["f", "lt", "eq", "le", "gt", "ne", "ge", "t"];
    for (k, cc) in want.iter().enumerate() {
        assert_eq!(vopc(0xd0 + k as u32), format!("v_cmpx_{cc}_u32"));
    }
}

#[test]
fn vopc_half_float_compares_are_split_in_two_blocks() {
    let lo = ["f", "lt", "eq", "le", "gt", "lg", "ge", "o"];
    let hi = ["u", "nge", "nlg", "ngt", "nle", "neq", "nlt", "tru"];
    for k in 0..8u32 {
        assert_eq!(vopc(0xc8 + k), format!("v_cmp_{}_f16", lo[k as usize]));
        assert_eq!(vopc(0xe8 + k), format!("v_cmp_{}_f16", hi[k as usize]));
        assert_eq!(vopc(0xd8 + k), format!("v_cmpx_{}_f16", lo[k as usize]));
        assert_eq!(vopc(0xf8 + k), format!("v_cmpx_{}_f16", hi[k as usize]));
    }
    assert_eq!(vopc(0x10), "v_cmpx_f_f32");
    assert_eq!(vopc(0x1f), "v_cmpx_tru_f32");
    assert_eq!(vopc(0x95), "v_cmpx_ne_i32");
    assert_eq!(vopc(0x98), "v_cmpx_class_f32");
}

#[test]
fn vopc_16bit_integer_compare_blocks_match_gfx1030() {
    // Base opcodes assembled with llvm-mc -mcpu=gfx1030. Adjacent opcodes include
    // floating-point class tests and cannot be filled from the 32-bit predicate layout.
    for (base, prefix, ty) in [(0x89, "cmp", "i16"), (0x99, "cmpx", "i16"), (0xa9, "cmp", "u16"), (0xb9, "cmpx", "u16")] {
        for (i, cc) in ["lt", "eq", "le", "gt", "ne", "ge"].iter().enumerate() {
            assert_eq!(vopc(base + i as u32), format!("v_{prefix}_{cc}_{ty}"));
        }
    }
    assert_eq!(vopc(0x88), "v_cmp_class_f32");
    assert_eq!(vopc(0x98), "v_cmpx_class_f32");
}

#[test]
fn cmpk_immediate_uses_the_predicate_signedness() {
    for (base, expected) in [(3u32, -1i64), (9, 65535)] {
        for op in base..base + 6 {
            let word = 0xb010_ffff | (op << 23);
            let ins = &disassemble(&word.to_le_bytes())[0];
            assert_eq!(ins.aux.imm, expected, "{}", ins.mnemonic);
        }
    }
}

#[test]
fn half_float_fma_forms_have_gfx10_names() {
    let vop2 = |op: u32| mnemonic(&[(op << 25) | (3 << 17) | (1 << 9) | 256, 0x3c00]);
    assert_eq!(vop2(0x36), "v_fmac_f16");
    assert_eq!(vop2(0x37), "v_fmamk_f16");
    assert_eq!(vop2(0x38), "v_fmaak_f16");
}

#[test]
fn flat_dwordx4_precedes_dwordx3() {
    // FLAT: dword0 op[24:18], seg[15:14] = 2 (global); same x4-before-x3 order as MUBUF.
    let flat = |op: u32| mnemonic(&[(0b110111 << 26) | (op << 18) | (2 << 14), 0x007f_0000]);
    assert_eq!(flat(0x0e), "flat_load_dwordx4");
    assert_eq!(flat(0x0f), "flat_load_dwordx3");
}

#[test]
fn nonexistent_vop3_opcode_is_not_named() {
    // VOP3 0x16b is undefined in both references (it was labelled v_mul_lo_i32).
    let m = mnemonic(&[(0b110101 << 26) | (0x16b << 16) | 3, 0x0000_0300]);
    assert!(!m.starts_with("v_mul"), "{m}");
}

#[test]
fn dpp_lane_selectors_are_part_of_the_vector_instruction() {
    for prefix in [0x7e06_0200u32, 0x7c84_0200] {
        for selector in [233, 234, 250] {
            let isa: Vec<_> = [prefix | selector, 0xde54_c101, 0xbf81_0000]
                .into_iter().flat_map(u32::to_le_bytes).collect();
            let ins = disassemble(&isa);
            assert_eq!(ins.len(), 2);
            assert_eq!((ins[1].offset, ins[1].mnemonic.as_str()), (8, "s_endpgm"));
        }
    }
}
