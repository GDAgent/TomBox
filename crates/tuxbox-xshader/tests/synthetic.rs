//! Translator regression tests on hand-assembled RDNA2 programs.
//!
//! These cover behaviour that was found wrong while running the title, without needing a
//! game shader as a fixture: each program is a few instructions written out as raw dwords,
//! wrapped in a `ShaderExtract` with a made-up signature.

use tuxbox_xshader::hlsl::translate;
use tuxbox_xshader::meta::{RootParam, RootParamKind, RootRange, RootSig, SigElement};
use tuxbox_xshader::{ShaderExtract, Stage};

const S_ENDPGM: u32 = 0xbf81_0000;
const S_NOP: u32 = 0xbf80_0000;
/// `v_cvt_pkrtz_f16_f32 v0, v2, v2` / `v1, v2, v2`: the default colour-export packing.
const PACK_V0_V2_V2: u32 = 0x5e00_0502;
const PACK_V1_V2_V2: u32 = 0x5e02_0502;
/// `exp mrt0 v0, v1` compressed, done, valid-mask.
const EXP_MRT0_COMPR: [u32; 2] = [0xf800_1c0f, 0x0000_0100];

fn sig(name: &str, sysval: u32, comp_type: u32, register: u32, mask: u8) -> SigElement {
    SigElement { name: name.into(), index: 0, sysval, comp_type, register, mask, rw_mask: 0 }
}

fn table(range_type: u32, num: u32, visibility: u32) -> RootParam {
    RootParam {
        visibility,
        kind: RootParamKind::Table(vec![RootRange { range_type, num, base_register: 0, space: 0, offset_in_table: u32::MAX }]),
    }
}

fn shader(stage: Stage, words: &[u32], input_sig: Vec<SigElement>, output_sig: Vec<SigElement>, root: Option<RootSig>) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract {
        stage,
        shader_model: (6, 0),
        bitcode_stripped: true,
        fspx_config: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        isa_preamble: [0; 4],
        has_input_sig: true,
        has_output_sig: true,
        has_psv0: false,
        has_root_sig: root.is_some(),
        input_sig,
        output_sig,
        resources: Vec::new(),
        gs_info: None,
        root_sig: root,
    }
}

/// A pixel shader that writes the integer 5 to a `uint4` render target through the default
/// pack-and-export tail. Converting the integer through float-to-half (what the tail does
/// literally) would give 0; the integer itself must be exported.
#[test]
fn integer_colour_export_keeps_the_integer() {
    let words = [
        0x7e04_0285, // v_mov_b32 v2, 5
        S_NOP,
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 1, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("uint4 o0 : SV_Target0"), "{}", t.hlsl);
    assert!(t.hlsl.contains("o.o0 = (uint4)(uint4(tb_pk0a, tb_pk0b, tb_pk1a, tb_pk1b).xyzw);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("tb_pk0a = v2;"), "{}", t.hlsl);
    // Only the shader body counts: the shared helper preamble mentions f16tof32.
    let body = &t.hlsl[t.hlsl.find(" main(").expect("entry point")..];
    assert!(!body.contains("f16tof32"), "integer export must not go through half floats:\n{}", t.hlsl);
}

/// The same tail feeding a float render target exports the unpacked floats (the target's own
/// format conversion applies, not a fixed half-float one).
#[test]
fn float_colour_export_is_not_squeezed_through_half() {
    let words = [
        0x7e04_02f2, // v_mov_b32 v2, 1.0
        S_NOP,
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(
        t.hlsl.contains("o.o0 = (float4)(float4(asfloat(tb_pk0a), asfloat(tb_pk0b), asfloat(tb_pk1a), asfloat(tb_pk1b)).xyzw);"),
        "{}",
        t.hlsl
    );
}

/// A packed export whose operands do not come from the pack instructions directly before it
/// keeps the literal behaviour (unpack the halves).
#[test]
fn packed_export_without_the_tail_is_left_alone() {
    let words = [
        0x7e00_0280, // v_mov_b32 v0, 0
        0x7e02_0280, // v_mov_b32 v1, 0
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("f16tof32"), "{}", t.hlsl);
    assert!(!t.hlsl.contains("tb_pk"), "{}", t.hlsl);
    // ...and an integer element cannot be produced from it: that has to fail loudly.
    let ex = shader(Stage::Pixel, &words, vec![], vec![sig("SV_Target", 64, 1, 0, 0xf)], None);
    assert!(translate(&ex).is_err());
}

/// A vertex shader exporting an integer parameter (the primitive id handed to the pixel
/// shader) must pass the register bits through, not convert them as if they were a float.
#[test]
fn integer_parameter_export_keeps_the_integer() {
    let words = [
        0x7e10_0287, // v_mov_b32 v8, 7
        0x7e02_02f2, // v_mov_b32 v1, 1.0
        0xf800_08cf, // exp pos0 v1, v1, v1, v1 (done)
        0x0101_0101,
        0xf800_020f, // exp param0 v8, v8, v8, v8
        0x0808_0808,
        S_ENDPGM,
    ];
    let out = vec![sig("PRIMITIVE_ID", 0, 1, 0, 0x1), sig("SV_Position", 1, 3, 1, 0xf)];
    let ex = shader(Stage::Vertex, &words, vec![], out, None);
    let t = translate(&ex).expect("translate");
    assert!(t.hlsl.contains("o.o0 = (uint)(uint4(v8, v8, v8, v8).x);"), "{}", t.hlsl);
}

/// Texel type of a texture read with texel fetches: integer when the loaded value is used as
/// an integer, float when it is used as a float. (The lighting-channel mask is an R16_UINT
/// texture; declared as float its bits never matched the light's channel mask.)
#[test]
fn texel_type_follows_the_use_of_the_loaded_value() {
    let root = || Some(RootSig { version: 2, flags: 1, params: vec![table(0, 64, 5), table(2, 16, 5)], num_static_samplers: 0, static_samplers: vec![] });
    let program = |use_op: u32| {
        vec![
            0xf40c_0004, // s_load_dwordx8 s0, s8 [off=0]: texture descriptor t0
            0xfa00_0000,
            0x7e00_0280, // v_mov_b32 v0, 0
            0x7e02_0280, // v_mov_b32 v1, 0
            0xf000_1108, // image_load v0 <- 2D texture, coords v0..
            0x0000_0000,
            use_op,
            S_NOP,
            S_NOP,
            PACK_V0_V2_V2,
            PACK_V1_V2_V2,
            EXP_MRT0_COMPR[0],
            EXP_MRT0_COMPR[1],
            S_ENDPGM,
        ]
    };
    // v_and_b32 v2, 1, v0
    let ex = shader(Stage::Pixel, &program(0x3604_0081), vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], root());
    let t = translate(&ex).expect("translate (integer use)");
    assert!(t.hlsl.contains("Texture2D<uint4> tex0 : register(t0);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("uint4 t0 = tex0.Load("), "{}", t.hlsl);
    // v_add_f32 v2, v0, v0
    let ex = shader(Stage::Pixel, &program(0x0604_0100), vec![], vec![sig("SV_Target", 64, 3, 0, 0xf)], root());
    let t = translate(&ex).expect("translate (float use)");
    assert!(t.hlsl.contains("Texture2D<float4> tex0 : register(t0);"), "{}", t.hlsl);
    assert!(t.hlsl.contains("float4 t0 = tex0.Load("), "{}", t.hlsl);
}

#[test]
fn thread_dispatch_metadata_rejects_access_to_the_wave_mask() {
    let make = |source| {
        let mut ex = shader(Stage::Compute, &[source, S_ENDPGM], vec![], vec![], None);
        ex.isa_preamble = [16, 8, 4, 1];
        translate(&ex).unwrap().portable_compute_group
    };
    assert_eq!(make(0x7e00_0281), Some([8, 4, 1])); // v_mov_b32 v0, 1
    assert_eq!(make(0x7e00_027e), None); // v_mov_b32 v0, exec_lo
}

/// A coherent GPU may return the expected value even if the translator drops the fence.
/// Check the device-scope lowering as well as the cross-wave readback in Coin Run.
#[test]
fn buffer_cache_controls_fence_compute_uavs() {
    for opcode in [0x71, 0x72] {
        let program = [
            0xf408_0302, 0xfa00_0000, // UAV descriptor in s[12:15]
            0xe070_2000, 0x8003_0107, // store v1
            0xe000_0000 | opcode << 18, 0, S_ENDPGM,
        ];
        let root = RootSig { version: 1, flags: 0, params: vec![table(1, 1, 0)], num_static_samplers: 0, static_samplers: vec![] };
        let ex = shader(Stage::Compute, &program, vec![], vec![], Some(root));
        let hlsl = translate(&ex).unwrap().hlsl;
        assert!(hlsl.contains("globallycoherent RWStructuredBuffer"));
        assert_eq!(hlsl.matches("DeviceMemoryBarrier();").count(), 1);
    }
}

#[test]
fn packed_root_addresses_and_vector_load_padding_cannot_escape_as_data() {
    let make = |operand: u32| {
        let root = RootSig {
            version: 1, flags: 0x0300_0000, num_static_samplers: 0, static_samplers: vec![],
            params: vec![
                RootParam { visibility: 0, kind: RootParamKind::Descriptor { kind: 4, register: 0, space: 0 } },
                RootParam { visibility: 0, kind: RootParamKind::Constants { register: 2, space: 0, num32: 1 } },
            ],
        };
        let mut ex = shader(Stage::Compute, &[
            0x8703_ff01, 255, 0xbe82_0300,   // s[2:3] = argument block
            0xf408_0201, 0xfa00_0000,       // s[8:11] = address, constant, padding
            0x7e00_0200 | operand, S_ENDPGM, // return a word as ordinary vector data
        ], vec![], vec![], Some(root));
        ex.psv_threads = Some([32, 1, 1]);
        translate(&ex)
    };
    assert!(make(10).expect("inline constant").hlsl.contains("cb2[0].x"));
    for operand in [8, 9, 11] {
        assert!(make(operand).err().unwrap().contains("root address or padding"));
    }
}

#[test]
fn psv_thread_dimensions_respect_stage_record_size_and_group_limits() {
    use tuxbox_xshader::meta::parse_compute_threads;
    let psv = |size: u32, stage, x, y, z| {
        [size, 0, 0, 0, 0, 0, 0, stage, 0, 0, x, y, z].into_iter()
            .flat_map(u32::to_le_bytes).collect::<Vec<_>>()
    };
    assert_eq!(parse_compute_threads(&psv(48, 5, 16, 4, 2)), Some([16, 4, 2]));
    for invalid in [psv(36, 5, 32, 1, 1), psv(56, 5, 32, 1, 1), psv(48, 2, 32, 1, 1),
                    psv(48, 5, 0, 1, 1), psv(48, 5, u32::MAX, 2, 1), psv(48, 5, 1, 1, 65)] {
        assert_eq!(parse_compute_threads(&invalid), None);
    }
}

#[test]
fn integer_wave_sum_requires_the_observed_shuffle_and_converged_full_waves() {
    let sum = [
        0xbe90_3dc1, 0xd501_0009, 0x0042_1680,
        0x4a12_12e9, 0xde54_c109, 0x4a12_12e9, 0x9360_1209, 0x4a12_12e9, 0x0009_2409,
        0x4a12_12fa, 0xff09_4009, 0xd778_000a, 0x0201_0109, 0x4a12_130a,
        0xd760_0013, 0x0001_0109, 0xbefe_0310, S_ENDPGM,
    ];
    let run = |words: &[u32], threads| {
        let mut ex = shader(Stage::Compute, words, vec![], vec![], None);
        ex.psv_threads = Some([threads, 1, 1]);
        translate(&ex)
    };
    assert_eq!(run(&sum, 64).unwrap().required_wave_size, Some(32));
    assert!(run(&sum, 48).err().unwrap().contains("complete 32-lane"));
    let mut altered = sum;
    altered[6] ^= 1 << 8;  // second stage selects a different source lane
    assert!(run(&altered, 64).err().unwrap().contains("shuffle"));
    let branch_inside = [&[0xbf88_0003][..], &sum].concat(); // EXECZ jumps to the first add
    assert!(run(&branch_inside, 64).err().unwrap().contains("divergent control flow"));
}

#[test]
fn a_wave_retiring_before_a_group_barrier_only_skips_later_work() {
    let run = |skip: u32| {
        let root = RootSig { version: 1, flags: 0x0300_0000, params: vec![], num_static_samplers: 0, static_samplers: vec![] };
        let words = [0xbe82_0300, 0x8703_ff01, 255, 0x7da8_0094, // packed root, then EXEC = local X < 20
                     0xbf88_0000 | skip, 0xbf8a_0000, 0x7e04_0285, S_ENDPGM];
        let mut ex = shader(Stage::Compute, &words, vec![], vec![], Some(root));
        ex.psv_threads = Some([64, 1, 1]);
        translate(&ex)
    };
    let hlsl = run(2).expect("branch to final ENDPGM").hlsl;
    let barrier = hlsl.find("GroupMemoryBarrierWithGroupSync();").unwrap();
    let guard = hlsl.find("if (!(j").unwrap();
    assert!(barrier < guard);
    assert!(hlsl[guard..].contains("v2 = 0x5u"));
    assert!(run(1).err().unwrap().contains("packed group barrier after a branch"));
}

#[test]
fn packed_ballots_require_paired_data_uses_and_converged_wave64() {
    let program = [
        0xbe82_0300, 0x8703_ff01, 255, // copy the argument pointer
        0x3602_00bf,                   // local X modulo 64
        0xbe88_246a,                   // save EXEC in s[8:9]
        0xd485_000a, 0x0002_0680,      // ballot v3 != 0 into s[10:11]
        0x3a0a_020a, 0x3a0c_020b,      // consume low and high words as integers
        0xbefe_0408, S_ENDPGM,
    ];
    let run = |words: &[u32], threads| {
        let root = RootSig { version: 1, flags: 0x0300_0000, params: vec![], num_static_samplers: 0, static_samplers: vec![] };
        let mut ex = shader(Stage::Compute, words, vec![], vec![], Some(root));
        ex.psv_threads = Some([threads, 1, 1]);
        translate(&ex)
    };
    let translated = run(&program, 128).unwrap();
    assert_eq!(translated.required_wave_size, Some(64));
    assert!(translated.hlsl.contains("WaveActiveBallot("));
    assert!(run(&program, 96).err().unwrap().contains("unresolved wave ballot"));
    let mut missing_high = program;
    missing_high[8] = S_NOP;
    assert!(run(&missing_high, 128).err().unwrap().contains("unresolved wave ballot"));
    let mut divergent = program.to_vec();
    divergent.insert(5, 0xbf88_0004); // EXECZ skips the comparison and both consumers
    assert!(run(&divergent, 128).err().unwrap().contains("unresolved wave ballot"));
    let mut invalid_restore = program;
    invalid_restore[9] = 0xbefe_040a; // physical ballot used as projected EXEC
    assert!(run(&invalid_restore, 128).err().unwrap().contains("unresolved saved EXEC"));
}

#[test]
fn packed_scan_rejects_divergent_swizzles_and_unresolved_masks() {
    let mut program = vec![
        0xbe82_0300, 0x8703_ff01, 255, // copy packed root pointer
        0x7d84_009f,                  // local X == last lane in wave32
    ];
    for (control, mask) in [(0x1e, 0xaaaaaaaau32), (0x3c, 0xcccccccc), (0x78, 0xf0f0f0f0),
                            (0xf0, 0xff00ff00), (0x1e0, 0xffff0000)] {
        program.extend([0xbe86_03ff, mask, 0xd8d4_0000 | control, 0x0400_0003,
                        0xd501_0004, 0x001a_0880, 0x4a06_0903]);
    }
    program.push(S_ENDPGM);
    let run = |words: &[u32]| {
        let root = RootSig { version: 1, flags: 0x0300_0000, params: vec![], num_static_samplers: 0, static_samplers: vec![] };
        let mut ex = shader(Stage::Compute, words, vec![], vec![], Some(root));
        ex.psv_threads = Some([32, 1, 1]);
        translate(&ex)
    };
    assert_eq!(run(&program).unwrap().required_wave_size, Some(32));
    let mut divergent = program.clone();
    divergent.insert(4, 0xbf88_0004); // EXECZ around the first lane read
    assert!(run(&divergent).err().unwrap().contains("non-converged"));
    let mut unknown_mask = program.clone();
    unknown_mask.splice(4..6, [0xbe86_0304]); // group ID instead of a known physical lane mask
    assert!(run(&unknown_mask).err().unwrap().contains("unresolved physical scan lane mask"));
    program[6] |= 1 << 17; // GDS addressing
    assert!(run(&program).is_err());
}

#[test]
fn fspx_prepass_checks_raw_descriptor_origin_and_type() {
    use tuxbox_xshader::meta::PsvResource;
    let root = RootSig {
        version: 2, flags: 0, num_static_samplers: 0, static_samplers: vec![],
        params: [table(1, 13, 0), table(0, 21, 0)].into_iter().chain((0..4).map(|register|
            RootParam { visibility: 0, kind: RootParamKind::Descriptor { kind: 2, register, space: 0 } }
        )).collect(),
    };
    let program = [
        0xbe8a0305, 0xbe8b0380,          // build the SRV table address from s5
        0xf4080405, 0xfa000240,          // load raw descriptor t18 into s[16:19]
        0x938aff11, 0xe0010, 0x848a810a, // extract stride; take max(stride, 1)
        0x8713ff13, 0xcfffffff, 0x93120a12,
        0x8813ff13, 0x30000000, 0x8711ff11, 0xc000ffff,
        0xe0381000, 0x80040c00, S_ENDPGM, // load four dwords from a byte offset
    ];
    let run = |words: &[u32], ty| {
        let mut ex = shader(Stage::Compute, words, vec![], vec![], Some(root.clone()));
        ex.psv_threads = Some([256, 1, 1]);
        ex.fspx_config = Some([0x4005cf00, 0x6c090102, 0, 0x10100, 0x100001, 0x402c0345, 0x1819c]);
        ex.resources.push(PsvResource { res_type: ty, space: 0, lower: 18, upper: 18 });
        translate(&ex)
    };
    assert!(run(&program,4).unwrap().hlsl.contains("root_t18.Load4("));
    assert!(run(&program,3).err().unwrap().contains("type disagrees with PSV"));
    let mut altered = program;
    altered[3] += 4;
    assert!(run(&altered,4).err().unwrap().contains("descriptor table address"));
    altered = program;
    altered[8] = u32::MAX; // changed OOB control field
    assert!(run(&altered,4).is_err());
    let skip_origin = [&[0xbf880001][..], &program].concat();
    assert!(run(&skip_origin,4).err().unwrap().contains("unresolved descriptor table"));
}

/// Input element with its read mask set (only read registers get an attribute slot).
fn read_sig(name: &str, sysval: u32, comp_type: u32, register: u32, mask: u8) -> SigElement {
    SigElement { rw_mask: mask, ..sig(name, sysval, comp_type, register, mask) }
}

/// A pixel shader that reads TEXCOORD0 per vertex (`v_interp_mov_f32 P10`, so it becomes
/// `nointerpolation` + `SV_Barycentrics`) and has an integer TEXCOORD1, fed by a vertex
/// shader exporting both. DXC packs elements into one register only when their
/// interpolation modes match: in the pixel shader both inputs are `nointerpolation` and
/// share r0, in a vertex shader without modifiers TEXCOORD0 is linear and TEXCOORD1 gets a
/// register of its own, and D3D12 rejects the pair (seen in Fortnite with a geometry shader
/// writing SV_RenderTargetArrayIndex). The linked translation gives the vertex shader's
/// outputs the pixel shader's modifiers.
fn linkage_pair() -> (ShaderExtract, ShaderExtract) {
    let ps_words = [
        0xc80a_0000, // v_interp_mov_f32 v2, p10, attr0.x (per-vertex read)
        0xc80e_0402, // v_interp_mov_f32 v3, p0, attr1.x (flat)
        S_NOP,
        PACK_V0_V2_V2,
        PACK_V1_V2_V2,
        EXP_MRT0_COMPR[0],
        EXP_MRT0_COMPR[1],
        S_ENDPGM,
    ];
    let ps_in = vec![
        read_sig("TEXCOORD", 0, 3, 0, 0x3),
        sig("SV_Position", 1, 3, 1, 0xf),
        SigElement { index: 1, ..read_sig("TEXCOORD", 0, 1, 2, 0x1) },
    ];
    let ps = shader(Stage::Pixel, &ps_words, ps_in, vec![sig("SV_Target", 64, 3, 0, 0xf)], None);
    let vs_words = [
        0x7e10_0287, // v_mov_b32 v8, 7
        0x7e02_02f2, // v_mov_b32 v1, 1.0
        0xf800_08cf, // exp pos0 v1, v1, v1, v1 (done)
        0x0101_0101,
        0xf800_020f, // exp param0 v1, v1, v1, v1
        0x0101_0101,
        0xf800_021f, // exp param1 v8, v8, v8, v8
        0x0808_0808,
        S_ENDPGM,
    ];
    let vs_out = vec![
        sig("TEXCOORD", 0, 3, 0, 0x3),
        sig("SV_Position", 1, 3, 1, 0xf),
        SigElement { index: 1, ..sig("TEXCOORD", 0, 1, 2, 0x1) },
    ];
    let vs = shader(Stage::Vertex, &vs_words, vec![], vs_out, None);
    (ps, vs)
}

#[test]
fn stage_feeding_a_pixel_shader_gets_its_interpolation_modifiers() {
    use tuxbox_xshader::hlsl::translate_linked;
    let (ps, vs) = linkage_pair();
    let ps_t = translate(&ps).expect("PS");
    assert!(ps_t.needs_barycentrics, "{}", ps_t.hlsl);
    let modifiers: Vec<(&str, &str)> = ps_t.ps_input_interp.0.iter().map(|(s, m)| (s.as_str(), m.as_str())).collect();
    assert_eq!(modifiers, [("TEXCOORD0", "TB_VERTEX_INPUT"), ("TEXCOORD1", "nointerpolation")]);
    let vs_t = translate_linked(&vs, Some(&ps_t.ps_input_interp)).expect("linked VS");
    assert!(vs_t.hlsl.contains("#define TB_VERTEX_INPUT nointerpolation"), "{}", vs_t.hlsl);
    assert!(vs_t.hlsl.contains("    TB_VERTEX_INPUT float2 o0 : TEXCOORD0;"), "{}", vs_t.hlsl);
    assert!(vs_t.hlsl.contains("    nointerpolation uint o2 : TEXCOORD1;"), "{}", vs_t.hlsl);
    // Position keeps its own declaration; unlinked translation is unchanged.
    assert!(vs_t.hlsl.contains("    precise float4 o1 : SV_Position0;"), "{}", vs_t.hlsl);
    assert_eq!(translate(&vs).unwrap().hlsl, translate_linked(&vs, None).unwrap().hlsl);
    assert!(!translate(&vs).unwrap().hlsl.contains("TB_VERTEX_INPUT"));
}

#[cfg(windows)]
fn compile_shader(shader: &tuxbox_xshader::hlsl::Translated) -> Vec<u8> {
    tuxbox_xshader::dxc::compile(&shader.hlsl, &shader.dxc_profile()).expect("DXC compilation")
}

#[cfg(windows)]
fn texcoord1_location(code: &[u8], chunk: &[u8; 4]) -> (u32, u8) {
    let container = tuxbox_xshader::Dxbc::parse(code).expect("DXIL container");
    let sig = container.chunk(chunk).expect("signature chunk");
    let elements = tuxbox_xshader::meta::parse_signature(&code[sig.offset..sig.offset + sig.size]).unwrap();
    let coord = elements.iter().find(|e| e.index == 1 && e.name.eq_ignore_ascii_case("TEXCOORD")).unwrap();
    (coord.register, coord.mask)
}

/// In this pair the pixel shader reads TEXCOORD1 from r0.z. The ordinary VS
/// puts it in r2.x. Relinking must also work with differently cased semantics.
#[cfg(windows)]
#[test]
fn linked_signatures_compile_to_matching_registers() {
    use tuxbox_xshader::{dxc, hlsl::translate_linked, meta::has_packing_mismatch};
    if !dxc::available() {
        eprintln!("SKIPPED: DXC not available");
        return;
    }
    let (ps, mut vs) = linkage_pair();
    let pixel = translate(&ps).expect("PS");
    let ps_code = compile_shader(&pixel);
    assert_eq!(texcoord1_location(&ps_code, b"ISG1"), (0, 0b0100));
    for spelling in ["TEXCOORD", "TexCoord"] {
        vs.output_sig[0].name = spelling.into();
        vs.output_sig[2].name = spelling.into();
        let ordinary = compile_shader(&translate(&vs).expect("VS"));
        let linked = compile_shader(&translate_linked(&vs, Some(&pixel.ps_input_interp)).expect("linked VS"));
        assert_eq!(texcoord1_location(&ordinary, b"OSG1"), (2, 1));
        assert!(has_packing_mismatch(&ordinary, &ps_code));
        assert_eq!(texcoord1_location(&linked, b"OSG1"), (0, 0b0100));
        assert!(!has_packing_mismatch(&linked, &ps_code));
    }
}

/// A producer-only float2 shares TEXCOORD0's register. Mirroring the pixel
/// shader's flat modifier moves TEXCOORD1 from r1.xy to r1.zw.
#[cfg(windows)]
#[test]
fn keep_matching_producer_when_pixel_shader_reads_a_subset() {
    use tuxbox_xshader::{dxc, hlsl::translate_linked, meta::has_packing_mismatch};
    if !dxc::available() {
        eprintln!("SKIPPED: DXC not available");
        return;
    }
    let (_, mut vs) = linkage_pair();
    let flat = read_sig("TEXCOORD", 0, 3, 0, 3);
    let smooth = SigElement { index: 1, ..read_sig("TEXCOORD", 0, 3, 1, 3) };
    let position = sig("SV_Position", 1, 3, 2, 15);
    let ps = shader(Stage::Pixel, &[
        0xc80a_0002, // v_interp_mov_f32 P0, attr0.x (flat)
        0xc800_0400, 0xc801_0401, // v_interp_p1/p2_f32 for attr1.x (smooth)
        S_NOP, PACK_V0_V2_V2, PACK_V1_V2_V2, EXP_MRT0_COMPR[0], EXP_MRT0_COMPR[1], S_ENDPGM,
    ], vec![flat.clone(), smooth.clone(), position.clone()], vec![sig("SV_Target", 64, 3, 0, 15)], None);
    vs.output_sig = vec![flat, sig("EXTRA", 0, 3, 0, 12), smooth, position];
    let pixel = translate(&ps).expect("PS");
    let ps_code = compile_shader(&pixel);
    let ordinary = compile_shader(&translate(&vs).expect("VS"));
    let linked = compile_shader(&translate_linked(&vs, Some(&pixel.ps_input_interp)).expect("linked VS"));
    assert_eq!(texcoord1_location(&ps_code, b"ISG1"), (1, 0b0011));
    assert_eq!(texcoord1_location(&ordinary, b"OSG1"), (1, 0b0011));
    assert!(!has_packing_mismatch(&ordinary, &ps_code));
    assert_eq!(texcoord1_location(&linked, b"OSG1"), (1, 0b1100));
    assert!(has_packing_mismatch(&linked, &ps_code));
}
