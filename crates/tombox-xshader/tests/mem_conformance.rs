//! Vector-memory conformance: image (MIMG) instructions as translated for real shaders, run on
//! the host GPU against a reference model of the instruction.
//!
//! Each case hand-assembles a short program: `s_load_dword*` fetches a T# (and S#) from the
//! probe's root descriptor tables, then one MIMG instruction reads or writes an image whose
//! contents encode the texel address. The program is translated with the same emitter as game
//! shaders (`hlsl::translate_mem_probe`: descriptor dataflow, resource declarations, the
//! instruction emitter), compiled with DXC and dispatched with real textures bound.
//!
//! Address and data layout follow the RDNA2 ISA reference guide (AMD, 2020-11-30), §8.2:
//! MIMG fields (Table 41, bit positions from AMD's machine-readable RDNA2 spec), address
//! VGPR order (Tables 42-43: [offset] [bias] [z-compare] [derivatives] coordinates [lod]
//! [clamp]), DMASK packing ("enabled components come from / go to consecutive VGPRs"),
//! integer addresses for image ops without a sampler and float addresses with one.
//!
//! The expected values are chosen to be exact on any conformant D3D12 implementation:
//! point sampling at texel centres, gathers at texel corners, integer LODs and slices. What is
//! tested is the translation (operand order, coordinate and component handling), not the
//! host's filtering precision.
//!
//! Gather4 texel order is not given in the ISA guide. The model uses the D3D order
//! ((0,1), (1,1), (1,0), (0,0) relative to the footprint's top-left texel). Evidence: Mesa's
//! ACO compiler (src/amd/compiler/instruction_selection/aco_select_nir.cpp, visit_tex; mesa
//! af38c52) returns the image_gather4 result as the Vulkan/GL `textureGather` vec4 without
//! reordering (except a GFX8-and-older integer-cube workaround), and Vulkan/GL define the
//! same order as D3D.
//!
//! Needs Windows, a Direct3D 12 GPU and DXC; skipped (with a message) when they are missing.
//! `TOMBOX_MEM_ONLY=<substring>` runs a subset; `--nocapture` prints the per-case report.
#![cfg(windows)]

use tombox_xshader::dxc;
use tombox_xshader::gpu::{Binding, BufferData, Gpu, Sampler, TexDim, Texture};
use tombox_xshader::hlsl::{translate_mem_probe, valu_probe_stride, MemProbeTables};

const NV: u32 = 12; // v0..v3 address, v4..v7 data, v8..v11 scratch (offsets etc.)
const DATA: usize = 4;
const S_ENDPGM: u32 = 0xbf81_0000;
const S_WAITCNT_0: u32 = 0xbf8c_0000;
/// Data registers start out holding this (+ register number): a register the instruction
/// must not write keeps it.
const SENTINEL: u32 = 0xdead_0000;
const RGBA32F: u32 = 2; // DXGI_FORMAT_R32G32B32A32_FLOAT
const R32F: u32 = 41; // DXGI_FORMAT_R32_FLOAT

// ---------------------------------------------------------------------------------------
// Encoders (RDNA2 ISA §13: SOP1, SOPP, SMEM, VOP1, MIMG)

/// SGPRs the prologue loads descriptors into: T# s[8:15], S# s[16:19], UAV T# s[20:27].
const T_SGPR: u32 = 8;
const S_SGPR: u32 = 16;
const U_SGPR: u32 = 20;

fn s_mov_b32(dst: u32, src: u32) -> u32 {
    0xbe80_0000 | (dst << 16) | (0x03 << 8) | src
}

/// s_load_dword{x2,x4,x8,x16} (op 0..4) `dst`, s[base:base+1], `offset` bytes.
fn s_load(op: u32, dst: u32, base: u32, offset: u32) -> [u32; 2] {
    assert!(base % 2 == 0);
    [(0b111101 << 26) | (op << 18) | (dst << 6) | (base / 2), (0x7d << 25) | offset]
}

/// v_mov_b32 `dst`, literal.
fn v_mov_lit(dst: u32, lit: u32) -> [u32; 2] {
    [(0b0111111 << 25) | (dst << 17) | (0x01 << 9) | 255, lit]
}

#[derive(Clone, Copy, Default)]
struct Flags {
    unorm: bool,
    d16: bool,
    a16: bool,
    tfe: bool,
    lwe: bool,
}

/// MIMG (non-NSA): `op` (8-bit, OPM in bit 7), `dim`, `dmask`, address from v[vaddr..],
/// data at v[vdata..], T# at s[srsrc..], S# at s[ssamp..].
fn mimg(op: u32, dim: u32, dmask: u32, vaddr: u32, vdata: u32, srsrc: u32, ssamp: u32, fl: Flags) -> [u32; 2] {
    let w0 = (0b111100 << 26)
        | ((op & 0x7f) << 18)
        | ((op >> 7) & 1)
        | (dmask << 8)
        | (dim << 3)
        | ((fl.unorm as u32) << 12)
        | ((fl.tfe as u32) << 16)
        | ((fl.lwe as u32) << 17);
    let w1 = vaddr | (vdata << 8) | ((srsrc / 4) << 16) | ((ssamp / 4) << 21) | ((fl.a16 as u32) << 30) | ((fl.d16 as u32) << 31);
    [w0, w1]
}

/// Descriptor fetches: T# of SRV slot `t` (or UAV slot `u`), S# of sampler slot `s`.
/// The sampler table pointer is in odd s5, so it is first copied to s[0:1] (as compiled
/// shaders do; SMEM bases are SGPR pairs).
fn prologue(t: Option<u32>, s: Option<u32>, u: Option<u32>) -> Vec<u32> {
    let mut w = Vec::new();
    if let Some(t) = t {
        w.extend(s_load(3, T_SGPR, 4, 32 * t));
    }
    if let Some(s) = s {
        w.push(s_mov_b32(0, 5));
        w.extend(s_load(2, S_SGPR, 0, 16 * s));
    }
    if let Some(u) = u {
        w.extend(s_load(3, U_SGPR, 6, 32 * u));
    }
    w.push(S_WAITCNT_0);
    w
}

// MIMG opcodes (RDNA2 ISA §12.16) and dims (Table 41).
const IMAGE_LOAD: u32 = 0x00;
const IMAGE_LOAD_MIP: u32 = 0x01;
const IMAGE_STORE: u32 = 0x08;
const SAMPLE_L: u32 = 0x24;
const SAMPLE_LZ: u32 = 0x27;
const SAMPLE_C_LZ: u32 = 0x2f;
const SAMPLE_LZ_O: u32 = 0x37;
const GATHER4_LZ: u32 = 0x47;
const GATHER4_LZ_O: u32 = 0x57;
const D1: u32 = 0;
const D2: u32 = 1;
const D3: u32 = 2;
const CUBE: u32 = 3;
const D1A: u32 = 4;
const D2A: u32 = 5;

// ---------------------------------------------------------------------------------------
// Textures. Every texel encodes its address: (x + 1000 * mip, y, layer_or_z, 7 + face).

fn texel(x: u32, y: u32, z: u32, mip: u32) -> [f32; 4] {
    [(x + 1000 * mip) as f32, y as f32, z as f32, 7.0]
}

fn make_tex(dim: TexDim, w: u32, h: u32, d: u32, mips: u32) -> Texture {
    let mut t = Texture { dim, format: RGBA32F, texel_bytes: 16, width: w, height: h, depth: d, mips, data: Vec::new() };
    let layers = if dim == TexDim::D3 { 1 } else { d };
    for l in 0..layers {
        for m in 0..mips {
            let (mw, mh, md) = t.mip_size(m);
            for z in 0..md {
                for y in 0..mh {
                    for x in 0..mw {
                        let mut v = texel(x, y, if dim == TexDim::D3 { z } else { l }, m);
                        if dim == TexDim::Cube {
                            v[3] = 7.0 + l as f32;
                        }
                        for c in v {
                            t.data.extend(c.to_le_bytes());
                        }
                    }
                }
            }
        }
    }
    t
}

fn tex_value(t: &Texture, x: u32, y: u32, z: u32, layer: u32, mip: u32) -> Option<[u32; 4]> {
    if mip >= t.mips || (t.dim != TexDim::D3 && layer >= t.depth) {
        return None;
    }
    let (w, h, d) = t.mip_size(mip);
    if x >= w || y >= h || z >= d {
        return None;
    }
    let o = t.sub_offset(if t.dim == TexDim::D3 { 0 } else { layer }, mip) + (((z * h + y) * w + x) * t.texel_bytes) as usize;
    let c = |k: usize| u32::from_le_bytes(t.data[o + 4 * k..o + 4 * k + 4].try_into().unwrap());
    Some([c(0), c(1), c(2), c(3)])
}

// ---------------------------------------------------------------------------------------
// Cases

/// Expected data registers v4..v7: (value, mask of the bits compared); None = not checked.
type Want = [Option<(u32, u32)>; 4];

struct Case {
    name: String,
    words: Vec<u32>,
    srv: Vec<Binding>,
    samplers: Vec<Sampler>,
    uav: Vec<Binding>,
    /// Per-thread address registers v0..v3 and scratch v8..v11 (offsets, or the integer
    /// texel address of a float-addressed case for the model).
    inputs: Vec<([u32; 4], [u32; 4])>,
    want: Box<dyn Fn(&[u32; 4], &[u32; 4]) -> Want>,
    /// Optional check of UAV slot 0's contents after the run.
    uav_check: Option<Box<dyn Fn(&Binding) -> Result<(), String>>>,
    /// Compile as cs_6_6: compute-shader quad derivatives, for implicit-LOD sampling.
    sm66: bool,
}

/// DMASK packing: the enabled components of `v`, in order, then untouched registers.
fn packed(v: [u32; 4], dmask: u32) -> Want {
    let mut out = [None; 4];
    let mut n = 0;
    for c in 0..4 {
        if dmask >> c & 1 != 0 {
            out[n] = Some((v[c], !0));
            n += 1;
        }
    }
    for (k, o) in out.iter_mut().enumerate().skip(n) {
        *o = Some((SENTINEL + DATA as u32 + k as u32, !0));
    }
    out
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

fn grid(w: u32, h: u32, d: u32) -> Vec<[u32; 4]> {
    let mut v = Vec::new();
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                v.push([x, y, z, 0]);
            }
        }
    }
    v
}

fn cases() -> Vec<Case> {
    let mut c: Vec<Case> = Vec::new();
    let t2 = make_tex(TexDim::D2, 8, 4, 1, 3);

    // image_load 2D: every DMASK, including out-of-bounds texels (which read 0).
    for dmask in 1..16u32 {
        let t = t2.clone();
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD, D2, dmask, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let mut inputs: Vec<_> = grid(8, 4, 1).into_iter().map(|a| (a, [0; 4])).collect();
        inputs.extend([([8, 0, 0, 0], [0; 4]), ([0, 4, 0, 0], [0; 4]), ([100, 100, 0, 0], [0; 4])]);
        c.push(Case {
            name: format!("image_load 2d dmask {dmask:#x}"),
            words,
            srv: vec![Binding::Texture(t.clone())],
            samplers: vec![],
            uav: vec![],
            inputs,
            want: Box::new(move |a, _| packed(tex_value(&t, a[0], a[1], 0, 0, 0).unwrap_or([0; 4]), dmask)),
            uav_check: None,
            sm66: false,
        });
    }

    // image_load_mip 2D: (x, y, mip).
    {
        let t = t2.clone();
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD_MIP, D2, 0xf, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for m in 0..3 {
            let (w, h, _) = t.mip_size(m);
            for a in grid(w, h, 1) {
                inputs.push(([a[0], a[1], m, 0], [0; 4]));
            }
        }
        c.push(Case {
            name: "image_load_mip 2d".into(),
            words,
            srv: vec![Binding::Texture(t.clone())],
            samplers: vec![],
            uav: vec![],
            inputs,
            want: Box::new(move |a, _| packed(tex_value(&t, a[0], a[1], 0, 0, a[2]).unwrap_or([0; 4]), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }

    // image_load on 1D, 1D array, 2D array, 3D: (x[, y][, slice|z]).
    for (name, dim, td, w, h, d) in [
        ("1d", D1, TexDim::D1, 8, 1, 1),
        ("1d array", D1A, TexDim::D1Array, 8, 1, 3),
        ("2d array", D2A, TexDim::D2Array, 4, 4, 3),
        ("3d", D3, TexDim::D3, 4, 4, 4),
    ] {
        let t = make_tex(td, w, h, d, 1);
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD, dim, 0xf, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        // 1D array: the slice is the second address component.
        let inputs: Vec<_> = grid(w, if dim == D1A { d } else { h }, if dim == D1A { 1 } else { d })
            .into_iter()
            .map(|a| (a, [0; 4]))
            .collect();
        let tt = t.clone();
        c.push(Case {
            name: format!("image_load {name}"),
            words,
            srv: vec![Binding::Texture(t)],
            samplers: vec![],
            uav: vec![],
            inputs,
            want: Box::new(move |a, _| {
                let v = match dim {
                    D1 => tex_value(&tt, a[0], 0, 0, 0, 0),
                    D1A => tex_value(&tt, a[0], 0, 0, a[1], 0),
                    D2A => tex_value(&tt, a[0], a[1], 0, a[2], 0),
                    _ => tex_value(&tt, a[0], a[1], a[2], 0, 0),
                };
                packed(v.unwrap_or([0; 4]), 0xf)
            }),
            uav_check: None,
            sm66: false,
        });
    }

    // Sampling at texel centres with a point sampler: (u, v[, slice|w]) normalised.
    let point = Sampler::point_clamp();
    for (name, dim, td, w, h, d) in [
        ("1d", D1, TexDim::D1, 8, 1, 1),
        ("1d array", D1A, TexDim::D1Array, 8, 1, 3),
        ("2d", D2, TexDim::D2, 8, 4, 1),
        ("2d array", D2A, TexDim::D2Array, 4, 4, 3),
        ("3d", D3, TexDim::D3, 4, 4, 4),
    ] {
        let t = make_tex(td, w, h, d, 1);
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_LZ, dim, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let rows = if dim == D1A { d } else { h };
        let deep = if dim == D1A { 1 } else { d };
        let inputs: Vec<_> = grid(w, rows, deep)
            .into_iter()
            .map(|a| {
                let u = f((a[0] as f32 + 0.5) / w as f32);
                let v = if dim == D1A { f(a[1] as f32) } else { f((a[1] as f32 + 0.5) / h as f32) };
                let z = match dim {
                    D2A => f(a[2] as f32),
                    D3 => f((a[2] as f32 + 0.5) / d as f32),
                    _ => 0,
                };
                ([u, v, z, 0], a)
            })
            .collect();
        let tt = t.clone();
        c.push(Case {
            name: format!("image_sample_lz {name}"),
            words,
            srv: vec![Binding::Texture(t)],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| {
                let v = match dim {
                    D1 => tex_value(&tt, s[0], 0, 0, 0, 0),
                    D1A => tex_value(&tt, s[0], 0, 0, s[1], 0),
                    D2 => tex_value(&tt, s[0], s[1], 0, 0, 0),
                    D2A => tex_value(&tt, s[0], s[1], 0, s[2], 0),
                    _ => tex_value(&tt, s[0], s[1], s[2], 0, 0),
                };
                packed(v.expect("in range"), 0xf)
            }),
            uav_check: None,
            sm66: false,
        });
    }

    sample_cases(&mut c);
    gather_cases(&mut c);
    uav_cases(&mut c);
    modifier_cases(&mut c);
    lod_cases(&mut c);
    buffer_cases(&mut c);
    a16_lod_cases(&mut c);
    c
}

/// A16 with a bias and with derivatives, laid out as Mesa's ACO emits them
/// (src/amd/compiler/instruction_selection/aco_select_nir.cpp, visit_tex, and
/// aco_isel_helpers.cpp, emit_pack_v1; mesa af38c52): the bias is an f16 in its own dword,
/// derivatives stay 32-bit without the _g16 opcodes, coordinates are packed f16 pairs.
fn a16_lod_cases(c: &mut Vec<Case>) {
    const SAMPLE_D: u32 = 0x22;
    const SAMPLE_B: u32 = 0x25;
    let point = Sampler::point_clamp();
    let t = make_tex(TexDim::D2, 8, 8, 1, 4);
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg_nsa(SAMPLE_D, D2, 0xf, &[8, 9, 10, 11, 0], DATA as u32, T_SGPR, S_SGPR));
        // NSA with A16: set the A16 bit in dword 1.
        let n = words.len();
        words[n - 2] |= 1 << 30;
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for k in 0..4u32 {
            let size = 8 >> k;
            let g = f((1u32 << k) as f32 / 8.0);
            for a in grid(size, size, 1) {
                let (u, v) = ((a[0] as f32 + 0.5) / size as f32, (a[1] as f32 + 0.5) / size as f32);
                // v0 = packed (u, v); v1..v3 are not addresses here and carry the model's
                // inputs (mip, u, v); v8..v11 = 32-bit derivatives.
                inputs.push(([h(u) | (h(v) << 16), k, f(u), f(v)], [g, 0, 0, g]));
            }
        }
        c.push(Case {
            name: "image_sample_d a16 2d nsa".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |a, _| packed(texel_at(&tt, a[2], a[3], a[1]), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_B, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags { a16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for k in 0..3u32 {
            let size = 8 >> k;
            for qy in (0..size).step_by(2) {
                for qx in (0..size).step_by(2) {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let u = ((qx + dx) as f32 + 0.5) / size as f32;
                        let v = ((qy + dy) as f32 + 0.5) / size as f32;
                        // [bias: f16 in its own dword, high half garbage] [u|v packed]
                        inputs.push(([0xabcd_0000 | h(1.0), h(u) | (h(v) << 16), 0, 0], [f(u), f(v), k, 0]));
                    }
                }
            }
        }
        c.push(Case {
            name: "image_sample_b a16 2d quad +1".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| packed(texel_at(&tt, s[0], s[1], (s[2] + 1).min(3)), 0xf)),
            uav_check: None,
            sm66: true,
        });
    }
}

/// MUBUF: `op`, immediate `offset`, OFFEN / IDXEN, address v[vaddr..], data v[vdata..],
/// V# at s[srsrc..], SOFFSET = inline 0.
fn mubuf(op: u32, offset: u32, offen: bool, idxen: bool, vaddr: u32, vdata: u32, srsrc: u32) -> [u32; 2] {
    [
        (0b111000 << 26) | (op << 18) | ((idxen as u32) << 13) | ((offen as u32) << 12) | offset,
        vaddr | (vdata << 8) | ((srsrc / 4) << 16) | (0x80 << 24),
    ]
}

fn words_le(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Buffer instructions (MUBUF). Typed buffers are addressed by element index (IDXEN); the
/// view's format converts. Structured buffers: address = index * stride + offset; the
/// stride is the view's (V#), which the translation has to declare (see `sbuf_words`), so
/// these cases bind views with the stride it declares and test the addressing on top.
/// Out-of-range elements read 0 on both sides (RDNA2 ISA §8.1 "Range Checking", Table 35:
/// index >= NumRecords is out of range, reads return zero; D3D11.3 §22.4.12 ld_structured:
/// out-of-bounds components return 0).
fn buffer_cases(c: &mut Vec<Case>) {
    const LOAD_FORMAT_X: u32 = 0x00;
    const LOAD_FORMAT_XYZ: u32 = 0x02;
    const LOAD_FORMAT_XYZW: u32 = 0x03;
    const STORE_FORMAT_XYZW: u32 = 0x07;
    const LOAD_DWORD: u32 = 0x0c;
    const LOAD_DWORDX3: u32 = 0x0f;
    const STORE_DWORDX4: u32 = 0x1e;
    const N: u32 = 24;
    let val = |i: u32, k: u32| f((100 * i + k) as f32);

    // Typed loads from an RGBA32F view (and an R32F view for _x: the missing channels of
    // the view are not loaded at all, the instruction writes one VGPR).
    for (name, op, n, fmt, fbytes) in [
        ("buffer_load_format_x r32f", LOAD_FORMAT_X, 1u32, R32F, 4u32),
        ("buffer_load_format_xyz rgba32f", LOAD_FORMAT_XYZ, 3, RGBA32F, 16),
        ("buffer_load_format_xyzw rgba32f", LOAD_FORMAT_XYZW, 4, RGBA32F, 16),
    ] {
        let comps = fbytes / 4;
        let data: Vec<u32> = (0..N).flat_map(|i| (0..comps).map(move |k| val(i, k))).collect();
        let mut words = prologue(Some(0), None, None);
        words.extend(mubuf(op, 0, false, true, 0, DATA as u32, T_SGPR));
        words.push(S_ENDPGM);
        c.push(Case {
            name: name.into(),
            words,
            srv: vec![Binding::Buffer(BufferData { data: words_le(&data), format: fmt, format_bytes: fbytes, stride: 0 })],
            samplers: vec![],
            uav: vec![],
            inputs: (0..N + 4).map(|i| ([i, 0, 0, 0], [0; 4])).collect(),
            want: Box::new(move |a, _| {
                let mut v = [0u32; 4];
                if a[0] < N {
                    for (k, x) in v.iter_mut().enumerate().take(comps as usize) {
                        *x = val(a[0], k as u32);
                    }
                }
                packed(v, (1 << n) - 1)
            }),
            uav_check: None,
            sm66: false,
        });
    }

    // Typed store: element i <- data registers v8..v11.
    {
        let mut words = prologue(None, None, Some(0));
        words.extend(mubuf(STORE_FORMAT_XYZW, 0, false, true, 0, 8, U_SGPR));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "buffer_store_format_xyzw rgba32f".into(),
            words,
            srv: vec![],
            samplers: vec![],
            uav: vec![Binding::Buffer(BufferData { data: vec![0x55; (N * 16) as usize], format: RGBA32F, format_bytes: 16, stride: 0 })],
            inputs: (0..N).map(|i| ([i, 0, 0, 0], [val(i, 0), val(i, 1), val(i, 2), val(i, 3)])).collect(),
            want: Box::new(|_, _| packed([0; 4], 0)),
            uav_check: Some(Box::new(move |b: &Binding| {
                let Binding::Buffer(b) = b else { return Err("not a buffer".into()) };
                for i in 0..N {
                    for k in 0..4 {
                        let o = (16 * i + 4 * k) as usize;
                        let got = u32::from_le_bytes(b.data[o..o + 4].try_into().unwrap());
                        if got != val(i, k) {
                            return Err(format!("element {i}.{k} = {got:08x}, want {:08x}", val(i, k)));
                        }
                    }
                }
                Ok(())
            })),
            sm66: false,
        });
    }

    // Structured loads, 16-byte elements: dwordx3 at offset 0 and dword at offset 12 of
    // element v0 (the translation declares 16-byte elements for this extent).
    {
        let data: Vec<u32> = (0..N).flat_map(|i| (0..4).map(move |k| 1000 * i + k)).collect();
        let mut words = prologue(Some(0), None, None);
        words.extend(mubuf(LOAD_DWORDX3, 0, false, true, 0, DATA as u32, T_SGPR));
        words.extend(mubuf(LOAD_DWORD, 12, false, true, 0, DATA as u32 + 3, T_SGPR));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "buffer_load_dwordx3+dword structured".into(),
            words,
            srv: vec![Binding::Buffer(BufferData { data: words_le(&data), format: 0, format_bytes: 0, stride: 16 })],
            samplers: vec![],
            uav: vec![],
            inputs: (0..N + 4).map(|i| ([i, 0, 0, 0], [0; 4])).collect(),
            want: Box::new(move |a, _| {
                let v = if a[0] < N { [1000 * a[0], 1000 * a[0] + 1, 1000 * a[0] + 2, 1000 * a[0] + 3] } else { [0; 4] };
                packed(v, 0xf)
            }),
            uav_check: None,
            sm66: false,
        });
    }

    // Structured store, 16-byte elements.
    {
        let mut words = prologue(None, None, Some(0));
        words.extend(mubuf(STORE_DWORDX4, 0, false, true, 0, 8, U_SGPR));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "buffer_store_dwordx4 structured".into(),
            words,
            srv: vec![],
            samplers: vec![],
            uav: vec![Binding::Buffer(BufferData { data: vec![0x55; (N * 16) as usize], format: 0, format_bytes: 0, stride: 16 })],
            inputs: (0..N).map(|i| ([i, 0, 0, 0], [7 * i, 7 * i + 1, 7 * i + 2, 7 * i + 3])).collect(),
            want: Box::new(|_, _| packed([0; 4], 0)),
            uav_check: Some(Box::new(move |b: &Binding| {
                let Binding::Buffer(b) = b else { return Err("not a buffer".into()) };
                for i in 0..N {
                    for k in 0..4 {
                        let o = (16 * i + 4 * k) as usize;
                        let got = u32::from_le_bytes(b.data[o..o + 4].try_into().unwrap());
                        if got != 7 * i + k {
                            return Err(format!("element {i}.{k} = {got}, want {}", 7 * i + k));
                        }
                    }
                }
                Ok(())
            })),
            sm66: false,
        });
    }
}

/// MIMG-NSA: address VGPRs listed individually (vaddr = first, then 4 per extra dword).
fn mimg_nsa(op: u32, dim: u32, dmask: u32, addrs: &[u32], vdata: u32, srsrc: u32, ssamp: u32) -> Vec<u32> {
    let extra = (addrs.len() - 1).div_ceil(4) as u32;
    let [w0, w1] = mimg(op, dim, dmask, addrs[0], vdata, srsrc, ssamp, Flags::default());
    let mut w = vec![w0 | (extra << 1), w1];
    for chunk in addrs[1..].chunks(4) {
        w.push(chunk.iter().enumerate().fold(0, |acc, (k, &r)| acc | (r << (8 * k))));
    }
    w
}

/// Mip `m` texel under normalised (u, v) of an 8x8 texture.
fn texel_at(t: &Texture, u: u32, v: u32, m: u32) -> [u32; 4] {
    let (w, hh, _) = t.mip_size(m);
    let x = (f32::from_bits(u) as f64 * w as f64).floor() as u32;
    let y = (f32::from_bits(v) as f64 * hh as f64).floor() as u32;
    tex_value(t, x, y, 0, 0, m).unwrap()
}

/// Explicit and implicit LOD selection. Every LOD the cases produce is an exact integer
/// (power-of-two texel steps), so a point mip filter selects one mip on any implementation.
fn lod_cases(c: &mut Vec<Case>) {
    const SAMPLE: u32 = 0x20;
    const SAMPLE_CL: u32 = 0x21;
    const SAMPLE_D: u32 = 0x22;
    const SAMPLE_B: u32 = 0x25;
    let point = Sampler::point_clamp();
    let t = make_tex(TexDim::D2, 8, 8, 1, 4);

    // image_sample_d 2D, NSA: [ddx.u ddx.v ddy.u ddy.v] u v from v8 v9 v10 v11 v0 v1.
    // Gradient 2^k texels per pixel -> LOD k.
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg_nsa(SAMPLE_D, D2, 0xf, &[8, 9, 10, 11, 0, 1], DATA as u32, T_SGPR, S_SGPR));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for k in 0..4u32 {
            let size = 8 >> k;
            let g = f((1u32 << k) as f32 / 8.0);
            for a in grid(size, size, 1) {
                let u = f((a[0] as f32 + 0.5) / size as f32);
                let v = f((a[1] as f32 + 0.5) / size as f32);
                inputs.push(([u, v, k, 0], [g, 0, 0, g]));
            }
        }
        c.push(Case {
            name: "image_sample_d 2d nsa".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |a, _| packed(texel_at(&tt, a[0], a[1], a[2]), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }

    // Implicit derivatives (cs_6_6 quads: lanes 4q..4q+3 = top-left, top-right, bottom-left,
    // bottom-right, as on RDNA). Each quad covers 2x2 adjacent texel centres of mip k, so
    // the computed LOD is exactly k. The model gets (u, v, k) in a; `bias` and `clamp` move
    // the selected mip.
    for (name, op, bias, clamp) in [
        ("image_sample 2d quad", SAMPLE, None, None),
        ("image_sample_b 2d quad +1", SAMPLE_B, Some(1.0f32), None),
        ("image_sample_b 2d quad -1", SAMPLE_B, Some(-1.0), None),
        ("image_sample_cl 2d quad 2", SAMPLE_CL, None, Some(2.0f32)),
    ] {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(op, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for k in 0..3u32 {
            let size = 8 >> k;
            for qy in (0..size).step_by(2) {
                for qx in (0..size).step_by(2) {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let u = f(((qx + dx) as f32 + 0.5) / size as f32);
                        let v = f(((qy + dy) as f32 + 0.5) / size as f32);
                        // Address operands: [bias] u v [clamp].
                        let addr = match (bias, clamp) {
                            (Some(b), _) => [f(b), u, v, 0],
                            (_, Some(cl)) => [u, v, f(cl), 0],
                            _ => [u, v, 0, 0],
                        };
                        inputs.push((addr, [u, v, k, 0]));
                    }
                }
            }
        }
        c.push(Case {
            name: name.into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| {
                let mut m = s[2] as f32 + bias.unwrap_or(0.0);
                if let Some(cl) = clamp {
                    m = m.max(cl);
                }
                packed(texel_at(&tt, s[0], s[1], m.clamp(0.0, 3.0) as u32), 0xf)
            }),
            uav_check: None,
            sm66: true,
        });
    }
}

/// Round-to-nearest-even f32 -> f16 for values the cases use (exactly representable).
fn h(x: f32) -> u32 {
    let b = x.to_bits();
    if x == 0.0 {
        return (b >> 16) & 0x8000;
    }
    let sign = (b >> 16) & 0x8000;
    let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
    assert!((1..31).contains(&e) && b & 0x1fff == 0, "{x} is not an exact normal f16");
    sign | ((e as u32) << 10) | ((b >> 13) & 0x3ff)
}

fn clampi(v: i32, n: u32) -> u32 {
    v.clamp(0, n as i32 - 1) as u32
}

/// Packed 6-bit texel offsets (x [5:0], y [13:8]), RDNA2 ISA §8.2.4 "Offset".
fn offs(x: i32, y: i32) -> u32 {
    (x as u32 & 0x3f) | ((y as u32 & 0x3f) << 8)
}

fn sample_cases(c: &mut Vec<Case>) {
    let point = Sampler::point_clamp();
    let t2 = make_tex(TexDim::D2, 8, 4, 1, 3);

    // image_sample_l 2D: (u, v, lod) with integer LODs selects that mip (point mip filter).
    {
        let t = t2.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_L, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for m in 0..3 {
            let (w, hh, _) = t.mip_size(m);
            for a in grid(w, hh, 1) {
                let u = f((a[0] as f32 + 0.5) / w as f32);
                let v = f((a[1] as f32 + 0.5) / hh as f32);
                inputs.push(([u, v, f(m as f32), 0], [a[0], a[1], m, 0]));
            }
        }
        c.push(Case {
            name: "image_sample_l 2d".into(),
            words,
            srv: vec![Binding::Texture(t.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| packed(tex_value(&t, s[0], s[1], 0, 0, s[2]).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }

    // image_sample_lz_o 2D with constant offsets: [offset] u v. The offset register is set
    // by v_mov_b32 in the program (HLSL needs a literal for Sample offsets).
    for (ox, oy) in [(1, 0), (-1, 2), (3, -2), (-8, 7)] {
        let t = t2.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(v_mov_lit(0, offs(ox, oy)));
        words.extend(mimg(SAMPLE_LZ_O, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let inputs: Vec<_> = grid(8, 4, 1)
            .into_iter()
            .map(|a| ([0, f((a[0] as f32 + 0.5) / 8.0), f((a[1] as f32 + 0.5) / 4.0), 0], a))
            .collect();
        c.push(Case {
            name: format!("image_sample_lz_o 2d ({ox},{oy})"),
            words,
            srv: vec![Binding::Texture(t.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| {
                packed(tex_value(&t, clampi(s[0] as i32 + ox, 8), clampi(s[1] as i32 + oy, 4), 0, 0, 0).unwrap(), 0xf)
            }),
            uav_check: None,
            sm66: false,
        });
    }

    // Cube: hardware (s, t, face) addressing, s and t in [1, 2] (what v_cubesc/v_cubetc/
    // v_cubema-based code produces), face as float. Face texel = ((s-1)*w, (t-1)*h).
    {
        let t = make_tex(TexDim::Cube, 2, 2, 6, 1);
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_LZ, CUBE, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let inputs: Vec<_> = grid(2, 2, 6)
            .into_iter()
            .map(|a| ([f(1.0 + (a[0] as f32 + 0.5) / 2.0), f(1.0 + (a[1] as f32 + 0.5) / 2.0), f(a[2] as f32), 0], a))
            .collect();
        c.push(Case {
            name: "image_sample_lz cube".into(),
            words,
            srv: vec![Binding::Texture(t.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| packed(tex_value(&t, s[0], s[1], 0, s[2], 0).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }

    // image_sample_c_lz 2D: [zref] u v against an R32_FLOAT texture holding x + 10 y, with a
    // LESS comparison sampler: result = (zref < texel) (D3D11.3 §22.4.19: "srcReferenceValue
    // {ComparisonFunction} texel.R"), replicated to the enabled channels.
    for dmask in [0x1u32, 0x3] {
        let mut t = Texture { dim: TexDim::D2, format: R32F, texel_bytes: 4, width: 8, height: 4, depth: 1, mips: 1, data: Vec::new() };
        for y in 0..4 {
            for x in 0..8 {
                t.data.extend(((x + 10 * y) as f32).to_le_bytes());
            }
        }
        let mut cmp = Sampler::point_clamp();
        cmp.filter = 0x80; // COMPARISON_MIN_MAG_MIP_POINT
        cmp.comparison = 2; // LESS
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_C_LZ, D2, dmask, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let mut inputs = Vec::new();
        for a in grid(8, 4, 1) {
            let texel = (a[0] + 10 * a[1]) as f32;
            for dz in [-0.5f32, 0.5] {
                inputs.push(([f(texel + dz), f((a[0] as f32 + 0.5) / 8.0), f((a[1] as f32 + 0.5) / 4.0), 0], [(dz < 0.0) as u32, 0, 0, 0]));
            }
        }
        c.push(Case {
            name: format!("image_sample_c_lz 2d dmask {dmask:#x}"),
            words,
            srv: vec![Binding::Texture(t)],
            samplers: vec![cmp],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| {
                let r = f(if s[0] == 1 { 1.0 } else { 0.0 });
                packed([r; 4], dmask)
            }),
            uav_check: None,
            sm66: false,
        });
    }
}

fn gather_cases(c: &mut Vec<Case>) {
    let point = Sampler::point_clamp();
    let t = make_tex(TexDim::D2, 8, 4, 1, 1);
    // Gathers at the shared corner of texels (x, y)..(x+1, y+1): the footprint is exact.
    let corner = |a: &[u32; 4]| [f((a[0] + 1) as f32 / 8.0), f((a[1] + 1) as f32 / 4.0)];
    let order = |t: &Texture, x: i32, y: i32, ch: usize| -> [u32; 4] {
        let at = |dx: i32, dy: i32| tex_value(t, clampi(x + dx, 8), clampi(y + dy, 4), 0, 0, 0).unwrap()[ch];
        [at(0, 1), at(1, 1), at(1, 0), at(0, 0)]
    };
    for (ch, dmask) in [1u32, 2, 4, 8].into_iter().enumerate() {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(GATHER4_LZ, D2, dmask, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
        words.push(S_ENDPGM);
        let inputs: Vec<_> = grid(7, 3, 1)
            .into_iter()
            .map(|a| {
                let [u, v] = corner(&a);
                ([u, v, 0, 0], a)
            })
            .collect();
        c.push(Case {
            name: format!("image_gather4_lz 2d dmask {dmask:#x}"),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs,
            want: Box::new(move |_, s| order(&tt, s[0] as i32, s[1] as i32, ch).map(|v| Some((v, !0)))),
            uav_check: None,
            sm66: false,
        });
    }
    // gather4_lz_o: constant offset (literal in the program) and run-time offset (per thread).
    for runtime in [false, true] {
        for (ox, oy) in [(1, 1), (-2, 0), (3, -1)] {
            let tt = t.clone();
            let mut words = prologue(Some(0), Some(0), None);
            if !runtime {
                words.extend(v_mov_lit(0, offs(ox, oy)));
            }
            words.extend(mimg(GATHER4_LZ_O, D2, 0x2, 0, DATA as u32, T_SGPR, S_SGPR, Flags::default()));
            words.push(S_ENDPGM);
            let inputs: Vec<_> = grid(7, 3, 1)
                .into_iter()
                .map(|a| {
                    let [u, v] = corner(&a);
                    ([offs(ox, oy), u, v, 0], a)
                })
                .collect();
            c.push(Case {
                name: format!("image_gather4_lz_o 2d ({ox},{oy}){}", if runtime { " run-time" } else { "" }),
                words,
                srv: vec![Binding::Texture(tt.clone())],
                samplers: vec![point],
                uav: vec![],
                inputs,
                want: Box::new(move |_, s| order(&tt, s[0] as i32 + ox, s[1] as i32 + oy, 1).map(|v| Some((v, !0)))),
                uav_check: None,
                sm66: false,
            });
        }
    }
}

fn uav_cases(c: &mut Vec<Case>) {
    // image_store 2D: data from v8.. (DMASK-packed); disabled components are written as 0
    // (RDNA2 ISA Table 41, DMASK: "missing components get 0").
    for dmask in [0xfu32, 0x1, 0x6, 0x9] {
        let mut t = make_tex(TexDim::D2, 8, 4, 1, 1);
        t.data.iter_mut().for_each(|b| *b = 0x55); // distinct from every stored value
        let mut words = prologue(None, None, Some(0));
        words.extend(mimg(IMAGE_STORE, D2, dmask, 0, 8, U_SGPR, 0, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let val = |x: u32, y: u32, k: u32| f((100 * k + 10 * y + x) as f32);
        let inputs: Vec<_> = grid(8, 4, 1).into_iter().map(|a| (a, [val(a[0], a[1], 0), val(a[0], a[1], 1), val(a[0], a[1], 2), val(a[0], a[1], 3)])).collect();
        c.push(Case {
            name: format!("image_store 2d dmask {dmask:#x}"),
            words,
            srv: vec![],
            samplers: vec![],
            uav: vec![Binding::Texture(t)],
            inputs,
            // The data registers are not written by a store.
            want: Box::new(|_, _| packed([0; 4], 0)),
            uav_check: Some(Box::new(move |b: &Binding| {
                let Binding::Texture(t) = b else { return Err("not a texture".into()) };
                for y in 0..4 {
                    for x in 0..8 {
                        let got = tex_value(t, x, y, 0, 0, 0).unwrap();
                        let mut want = [0u32; 4];
                        let mut k = 0;
                        for (ch, w) in want.iter_mut().enumerate() {
                            if dmask >> ch & 1 != 0 {
                                *w = val(x, y, k);
                                k += 1;
                            }
                        }
                        if got != want {
                            return Err(format!("texel ({x},{y}) = {got:08x?}, want {want:08x?}"));
                        }
                    }
                }
                Ok(())
            })),
            sm66: false,
        });
    }
    // D16 image_store: enabled components come from the halves of v8, v9 (low half first).
    for dmask in [0xfu32, 0x7] {
        let mut t = make_tex(TexDim::D2, 8, 4, 1, 1);
        t.data.iter_mut().for_each(|b| *b = 0x55);
        let mut words = prologue(None, None, Some(0));
        words.extend(mimg(IMAGE_STORE, D2, dmask, 0, 8, U_SGPR, 0, Flags { unorm: true, d16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let val = |x: u32, y: u32, k: u32| (100 * k + 10 * y + x) as f32;
        let inputs: Vec<_> = grid(8, 4, 1)
            .into_iter()
            .map(|a| {
                let v = |k| h(val(a[0], a[1], k));
                (a, [v(0) | (v(1) << 16), v(2) | (v(3) << 16), 0, 0])
            })
            .collect();
        c.push(Case {
            name: format!("image_store d16 2d dmask {dmask:#x}"),
            words,
            srv: vec![],
            samplers: vec![],
            uav: vec![Binding::Texture(t)],
            inputs,
            want: Box::new(|_, _| packed([0; 4], 0)),
            uav_check: Some(Box::new(move |b: &Binding| {
                let Binding::Texture(t) = b else { return Err("not a texture".into()) };
                for y in 0..4 {
                    for x in 0..8 {
                        let got = tex_value(t, x, y, 0, 0, 0).unwrap();
                        let mut want = [0u32; 4];
                        for (ch, w) in want.iter_mut().enumerate() {
                            if dmask >> ch & 1 != 0 {
                                *w = f(val(x, y, ch as u32));
                            }
                        }
                        if got != want {
                            return Err(format!("texel ({x},{y}) = {got:08x?}, want {want:08x?}"));
                        }
                    }
                }
                Ok(())
            })),
            sm66: false,
        });
    }
    // image_load from a UAV (RWTexture2D<float4>).
    {
        let t = make_tex(TexDim::D2, 8, 4, 1, 1);
        let mut words = prologue(None, None, Some(0));
        words.extend(mimg(IMAGE_LOAD, D2, 0xb, 0, DATA as u32, U_SGPR, 0, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        let tt = t.clone();
        c.push(Case {
            name: "image_load 2d uav dmask 0xb".into(),
            words,
            srv: vec![],
            samplers: vec![],
            uav: vec![Binding::Texture(t)],
            inputs: grid(8, 4, 1).into_iter().map(|a| (a, [0; 4])).collect(),
            want: Box::new(move |a, _| packed(tex_value(&tt, a[0], a[1], 0, 0, 0).unwrap(), 0xb)),
            uav_check: None,
            sm66: false,
        });
    }
}

/// Instruction bits the translator has to honour or refuse: D16 (16-bit data), A16 (16-bit
/// addresses), UNORM (texel-space coordinates with a sampler), TFE (residency status).
fn modifier_cases(c: &mut Vec<Case>) {
    let point = Sampler::point_clamp();
    let t = make_tex(TexDim::D2, 8, 4, 1, 1);
    // D16 loads: "convert data in memory to 16-bit format before storing it in VGPRs (2 16bit
    // values per VGPR)"; enabled components pack low half first.
    let pack16 = |v: [u32; 4], dmask: u32| -> Want {
        let comps: Vec<u32> = (0..4).filter(|ch| dmask >> ch & 1 != 0).map(|ch| h(f32::from_bits(v[ch]))).collect();
        let mut out = [None; 4];
        // With an odd count the high half of the last VGPR is not specified: not compared.
        for (k, pair) in comps.chunks(2).enumerate() {
            out[k] = Some(match pair.get(1) {
                Some(hi) => (pair[0] | hi << 16, !0),
                None => (pair[0], 0xffff),
            });
        }
        for (k, o) in out.iter_mut().enumerate().skip(comps.len().div_ceil(2)) {
            *o = Some((SENTINEL + DATA as u32 + k as u32, !0));
        }
        out
    };
    for dmask in [0xfu32, 0x7, 0x1] {
        let tt = t.clone();
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD, D2, dmask, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, d16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        c.push(Case {
            name: format!("image_load d16 2d dmask {dmask:#x}"),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![],
            uav: vec![],
            inputs: grid(8, 4, 1).into_iter().map(|a| (a, [0; 4])).collect(),
            want: Box::new(move |a, _| pack16(tex_value(&tt, a[0], a[1], 0, 0, 0).unwrap(), dmask)),
            uav_check: None,
            sm66: false,
        });
    }
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD, D2, 0xf, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, d16: true, ..Default::default() }));
        // RGBA16 fills v4:v5. v6 remains an independent integer register.
        words.push((0x13 << 25) | (6 << 17) | (6 << 9) | 129); // v_min_u32 v6, 1, v6
        words.push(S_ENDPGM);
        c.push(Case {
            name: "image_load d16 + neighbor uint".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![],
            uav: vec![],
            inputs: grid(8, 4, 1).into_iter().map(|a| (a, [0; 4])).collect(),
            want: Box::new(move |a, _| {
                let mut out = pack16(tex_value(&tt, a[0], a[1], 0, 0, 0).unwrap(), 0xf);
                out[2] = Some((1, !0));
                out
            }),
            uav_check: None,
            sm66: false,
        });
    }
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_LZ, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags { d16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "image_sample_lz d16 2d".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs: grid(8, 4, 1).into_iter().map(|a| ([f((a[0] as f32 + 0.5) / 8.0), f((a[1] as f32 + 0.5) / 4.0), 0, 0], a)).collect(),
            want: Box::new(move |_, s| pack16(tex_value(&tt, s[0], s[1], 0, 0, 0).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }
    // A16: "all address components are 16-bit UINT for image ops without sampler; 16-bit
    // float for image ops with sampler ... packed two per VGPR".
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), None, None);
        words.extend(mimg(IMAGE_LOAD, D2, 0xf, 0, DATA as u32, T_SGPR, 0, Flags { unorm: true, a16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "image_load a16 2d".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![],
            uav: vec![],
            inputs: grid(8, 4, 1).into_iter().map(|a| ([a[0] | (a[1] << 16), 0xffff_ffff, 0, 0], a)).collect(),
            want: Box::new(move |_, s| packed(tex_value(&tt, s[0], s[1], 0, 0, 0).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_LZ, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags { a16: true, ..Default::default() }));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "image_sample_lz a16 2d".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs: grid(8, 4, 1)
                .into_iter()
                .map(|a| ([h((a[0] as f32 + 0.5) / 8.0) | (h((a[1] as f32 + 0.5) / 4.0) << 16), f(100.0), 0, 0], a))
                .collect(),
            want: Box::new(move |_, s| packed(tex_value(&tt, s[0], s[1], 0, 0, 0).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }
    // UNORM with a sampler: coordinates in texels.
    {
        let tt = t.clone();
        let mut words = prologue(Some(0), Some(0), None);
        words.extend(mimg(SAMPLE_LZ, D2, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, Flags { unorm: true, ..Default::default() }));
        words.push(S_ENDPGM);
        c.push(Case {
            name: "image_sample_lz unorm 2d".into(),
            words,
            srv: vec![Binding::Texture(tt.clone())],
            samplers: vec![point],
            uav: vec![],
            inputs: grid(8, 4, 1).into_iter().map(|a| ([f(a[0] as f32 + 0.5), f(a[1] as f32 + 0.5), 0, 0], a)).collect(),
            want: Box::new(move |_, s| packed(tex_value(&tt, s[0], s[1], 0, 0, 0).unwrap(), 0xf)),
            uav_check: None,
            sm66: false,
        });
    }
}

// ---------------------------------------------------------------------------------------
// Runner

#[test]
fn mem_conformance() {
    if !dxc::available() {
        eprintln!("SKIPPED: DXC not available");
        return;
    }
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("SKIPPED: no D3D12 device ({e})");
            return;
        }
    };
    let only = std::env::var("TOMBOX_MEM_ONLY").ok();
    let sm66 = gpu.shader_model() >= 0x66;
    let stride = valu_probe_stride(NV) as usize;
    let mut failed: Vec<String> = Vec::new();
    let mut report = String::new();
    for mut c in cases() {
        if only.as_deref().is_some_and(|o| !c.name.contains(o)) {
            continue;
        }
        if c.sm66 && !sm66 {
            report.push_str(&format!("{:32} SKIPPED: needs shader model 6.6 (compute derivatives)
", c.name));
            continue;
        }
        let (hlsl, tables): (String, MemProbeTables) = match translate_mem_probe(&c.words, NV) {
            Ok(h) => h,
            Err(e) => {
                report.push_str(&format!("{:32} UNSUPPORTED by translator: {e}\n", c.name));
                failed.push(format!("{}: translator refused ({e})", c.name));
                continue;
            }
        };
        if tables.srv < c.srv.len() as u32 || tables.uav < c.uav.len() as u32 {
            failed.push(format!("{}: probe declares fewer descriptors than the case binds ({tables:?})", c.name));
            continue;
        }
        let dxil = match dxc::compile(&hlsl, if c.sm66 { "cs_6_6" } else { "cs_6_0" }) {
            Ok(d) => d,
            Err(e) => {
                failed.push(format!("{}: DXC failed: {e}\n{hlsl}", c.name));
                continue;
            }
        };
        let groups = c.inputs.len().div_ceil(64) as u32;
        let mut data = vec![0u32; groups as usize * 64 * stride];
        for t in 0..groups as usize * 64 {
            let o = t * stride;
            let (a, s) = c.inputs.get(t).copied().unwrap_or(c.inputs[0]);
            data[o..o + 4].copy_from_slice(&a);
            for k in 0..4 {
                data[o + DATA + k] = SENTINEL + (DATA + k) as u32;
            }
            data[o + 8..o + 12].copy_from_slice(&s);
        }
        if let Err(e) = gpu.run_bound(&dxil, &mut data, groups, &c.srv, &c.samplers, &mut c.uav) {
            failed.push(format!("{}: GPU run failed: {e}", c.name));
            continue;
        }
        let mut bad = 0usize;
        let mut examples = Vec::new();
        for (t, (a, s)) in c.inputs.iter().enumerate() {
            let o = t * stride;
            let got: [u32; 4] = data[o + DATA..o + DATA + 4].try_into().unwrap();
            let want = (c.want)(a, s);
            if want.iter().zip(got.iter()).any(|(w, g)| w.is_some_and(|(v, m)| v != *g & m)) {
                bad += 1;
                if examples.len() < 4 {
                    examples.push(format!("addr {:08x?} -> got {got:08x?} want {want:08x?}", a));
                }
            }
        }
        let mut uav_err = None;
        if let Some(check) = &c.uav_check {
            if let Some(b) = c.uav.first() {
                uav_err = check(b).err();
            }
        }
        report.push_str(&format!("{:32} {:4} threads, {:4} mismatches{}\n", c.name, c.inputs.len(), bad, uav_err.as_deref().map(|e| format!(", UAV: {e}")).unwrap_or_default()));
        for e in &examples {
            report.push_str(&format!("      {e}\n"));
        }
        if bad > 0 || uav_err.is_some() {
            failed.push(format!("{}: {bad}/{} mismatches{}", c.name, c.inputs.len(), uav_err.map(|e| format!(", UAV: {e}")).unwrap_or_default()));
        }
    }
    eprintln!("{report}");
    assert!(failed.is_empty(), "{} case(s) failed:\n{}", failed.len(), failed.join("\n"));
}

/// Instruction forms whose semantics the translator does not model must be refused, not
/// translated approximately.
#[test]
fn mem_probe_refusals() {
    let refuse = |op: u32, dim: u32, fl: Flags, sampler: bool, why: &str| {
        let mut words = prologue(Some(0), sampler.then_some(0), None);
        words.extend(mimg(op, dim, 0xf, 0, DATA as u32, T_SGPR, S_SGPR, fl));
        words.push(S_ENDPGM);
        match translate_mem_probe(&words, NV) {
            Ok(_) => panic!("{why}: translated"),
            Err(e) => eprintln!("{why}: refused ({e})"),
        }
    };
    refuse(IMAGE_LOAD, D2, Flags { unorm: true, tfe: true, ..Default::default() }, false, "image_load tfe");
    refuse(IMAGE_LOAD, D2, Flags { unorm: true, lwe: true, ..Default::default() }, false, "image_load lwe");
    refuse(SAMPLE_LZ, D2, Flags { tfe: true, ..Default::default() }, true, "image_sample_lz tfe");
    refuse(SAMPLE_LZ, CUBE, Flags { unorm: true, ..Default::default() }, true, "image_sample_lz unorm cube");
}
