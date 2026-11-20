//! Parsers for the standard (PC-format) metadata chunks that survive in XSX shader
//! containers: ISG1/OSG1 signatures, PSV0 resource bindings and the RTS0 root signature.
//! These give the HLSL emitter the D3D-side view (semantics, registers, root parameters).

fn u32_at(b: &[u8], off: usize) -> Result<u32, String> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("meta: u32 out of bounds at {off:#x} (len {})", b.len()))
}

fn cstr_at(b: &[u8], off: usize) -> String {
    let tail = b.get(off..).unwrap_or(&[]);
    let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

/// One element of an ISG1/OSG1 signature.
#[derive(Debug, Clone)]
pub struct SigElement {
    pub name: String,
    pub index: u32,
    /// D3D_NAME system value (0 = none, 1 = SV_Position, 64 = SV_Target, ...).
    pub sysval: u32,
    /// D3D_REGISTER_COMPONENT_TYPE: 1 = uint, 2 = int, 3 = float.
    pub comp_type: u32,
    pub register: u32,
    pub mask: u8,
    pub rw_mask: u8,
}

/// Parse an ISG1/OSG1 chunk body (32-byte elements).
pub fn parse_signature(body: &[u8]) -> Result<Vec<SigElement>, String> {
    let count = u32_at(body, 0)?;
    if count > 64 {
        return Err(format!("implausible signature element count {count}"));
    }
    read_signature(body, 32, 4)
}

fn read_signature(body: &[u8], stride: usize, name_field: usize) -> Result<Vec<SigElement>, String> {
    let count = u32_at(body, 0)? as usize;
    let header = u32_at(body, 4)? as usize;
    if header < 8 || header > body.len() || count > (body.len() - header) / stride {
        return Err("signature elements exceed chunk bounds".into());
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = header + i * stride + name_field;
        let name_off = u32_at(body, e)? as usize;
        out.push(SigElement {
            name: cstr_at(body, name_off),
            index: u32_at(body, e + 4)?,
            sysval: u32_at(body, e + 8)?,
            comp_type: u32_at(body, e + 12)?,
            register: u32_at(body, e + 16)?,
            mask: body[e + 20],
            rw_mask: body[e + 21],
        });
    }
    Ok(out)
}

/// Only recompile the producer when a shared semantic has different physical packing.
/// Extra producer outputs and rasterizer-generated pixel inputs can legitimately be
/// absent from the opposite stage. D3D12 still validates the complete PSO.
pub fn has_packing_mismatch(producer: &[u8], pixel: &[u8]) -> bool {
    fn signature(code: &[u8], tags: &[&[u8; 4]]) -> Option<Vec<SigElement>> {
        let container = crate::Dxbc::parse(code).ok()?;
        let chunk = tags.iter().find_map(|tag| container.chunk(tag))?;
        // SM4/5 records have six words. OSG5 adds a leading stream index,
        // and the SM6 signature format also carries a trailing minimum precision.
        let (stride, name_field) = match &chunk.fourcc {
            b"ISGN" | b"OSGN" => (24, 0),
            b"OSG5" => (28, 4),
            _ => (32, 4),
        };
        read_signature(&code[chunk.offset..chunk.offset + chunk.size], stride, name_field).ok()
    }
    let Some(outputs) = signature(producer, &[b"OSG1", b"OSG5", b"OSGN"]) else { return false };
    let Some(inputs) = signature(pixel, &[b"ISG1", b"ISGN"]) else { return false };
    inputs.iter().any(|input| {
        outputs.iter().any(|out| {
            input.index == out.index && input.name.eq_ignore_ascii_case(&out.name)
                && (input.register != out.register || input.mask & !out.mask != 0)
        })
    })
}

/// A PSV0 resource binding.
#[derive(Debug, Clone)]
pub struct PsvResource {
    /// 1 Sampler, 2 CBV, 3 SRVTyped, 4 SRVRaw, 5 SRVStructured, 6 UAVTyped, 7 UAVRaw, 8 UAVStructured...
    pub res_type: u32,
    pub space: u32,
    pub lower: u32,
    pub upper: u32,
}

/// Parse the PSV0 chunk far enough to recover the resource-binding list.
/// Geometry-shader facts from the PSV0 runtime info.
#[derive(Debug, Clone, Copy)]
pub struct GsInfo {
    /// D3D input primitive: 1 point, 2 line, 3 triangle, 6 line-adj, 7 triangle-adj.
    pub input_primitive: u32,
    /// D3D output topology: 1 point list, 3 line strip, 5 triangle strip.
    pub output_topology: u32,
    pub max_vertex_count: u16,
}

/// Parse the GS part of PSV0's PSVRuntimeInfo: the stage union starts at +4
/// ({InputPrimitive u32, OutputTopology u32, OutputStreamMask u32, ...}); PSVRuntimeInfo1
/// adds ShaderStage u8 @+24, UsesViewID u8 @+25 and MaxVertexCount u16 @+26.
pub fn parse_psv0_gs(body: &[u8]) -> Option<GsInfo> {
    let info_size = u32_at(body, 0).ok()? as usize;
    if info_size < 28 {
        return None;
    }
    let info = body.get(4..4 + info_size)?;
    Some(GsInfo {
        input_primitive: u32_at(info, 0).ok()?,
        output_topology: u32_at(info, 4).ok()?,
        max_vertex_count: u16::from_le_bytes([*info.get(26)?, *info.get(27)?]),
    })
}

/// Compute group dimensions follow the 36-byte PSVRuntimeInfo1 prefix.
pub fn parse_compute_threads(body: &[u8]) -> Option<[u32; 3]> {
    let size = u32_at(body, 0).ok()? as usize;
    let info = body.get(4..4usize.checked_add(size)?)?;
    if info.get(24) != Some(&5) {
        return None;
    }
    let [x, y, z] = [u32_at(info, 36).ok()?, u32_at(info, 40).ok()?, u32_at(info, 44).ok()?];
    (x > 0 && y > 0 && (1..=64).contains(&z) && x.checked_mul(y)?.checked_mul(z)? <= 1024).then_some([x, y, z])
}

pub fn parse_psv0(body: &[u8]) -> Result<Vec<PsvResource>, String> {
    let info_size = u32_at(body, 0)? as usize;
    let mut p = 4 + info_size;
    let count = u32_at(body, p)? as usize;
    p += 4;
    if count == 0 {
        return Ok(vec![]);
    }
    if count > 256 {
        return Err(format!("implausible PSV0 resource count {count}"));
    }
    let bind_size = u32_at(body, p)? as usize;
    p += 4;
    if bind_size < 16 {
        return Err(format!("PSV0 bind size {bind_size} too small"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = p + i * bind_size;
        out.push(PsvResource {
            res_type: u32_at(body, e)?,
            space: u32_at(body, e + 4)?,
            lower: u32_at(body, e + 8)?,
            upper: u32_at(body, e + 12)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RootRange {
    /// 0 SRV, 1 UAV, 2 CBV, 3 Sampler.
    pub range_type: u32,
    pub num: u32,
    pub base_register: u32,
    pub space: u32,
    pub offset_in_table: u32,
}

#[derive(Debug, Clone)]
pub enum RootParamKind {
    Table(Vec<RootRange>),
    Constants { register: u32, space: u32, num32: u32 },
    /// 2 = CBV, 3 = SRV, 4 = UAV root descriptor.
    Descriptor { kind: u32, register: u32, space: u32 },
}

#[derive(Debug, Clone)]
pub struct RootParam {
    pub visibility: u32,
    pub kind: RootParamKind,
}

#[derive(Debug, Clone)]
pub struct RootSig {
    pub version: u32,
    pub flags: u32,
    pub params: Vec<RootParam>,
    pub num_static_samplers: u32,
    /// D3D12_STATIC_SAMPLER_DESC words in serialized v1.0/v1.1 signatures.
    pub static_samplers: Vec<[u32; 13]>,
}

/// Parse an RTS0 (serialized root signature) chunk body.
pub fn parse_rts0(body: &[u8]) -> Result<RootSig, String> {
    let version = u32_at(body, 0)?;
    let nparams = u32_at(body, 4)? as usize;
    let params_off = u32_at(body, 8)? as usize;
    let num_static_samplers = u32_at(body, 12)?;
    let flags = u32_at(body, 20)?;
    if nparams > 64 {
        return Err(format!("implausible root parameter count {nparams}"));
    }
    let v11 = version >= 2;
    let mut params = Vec::with_capacity(nparams);
    for i in 0..nparams {
        let e = params_off + i * 12;
        let ptype = u32_at(body, e)?;
        let visibility = u32_at(body, e + 4)?;
        let off = u32_at(body, e + 8)? as usize;
        let kind = match ptype {
            0 => {
                let nranges = u32_at(body, off)? as usize;
                let roff = u32_at(body, off + 4)? as usize;
                if nranges > 64 {
                    return Err(format!("implausible range count {nranges}"));
                }
                let stride = if v11 { 24 } else { 20 };
                let mut ranges = Vec::with_capacity(nranges);
                for r in 0..nranges {
                    let q = roff + r * stride;
                    ranges.push(RootRange {
                        range_type: u32_at(body, q)?,
                        num: u32_at(body, q + 4)?,
                        base_register: u32_at(body, q + 8)?,
                        space: u32_at(body, q + 12)?,
                        offset_in_table: u32_at(body, q + if v11 { 20 } else { 16 })?,
                    });
                }
                RootParamKind::Table(ranges)
            }
            1 => RootParamKind::Constants {
                register: u32_at(body, off)?,
                space: u32_at(body, off + 4)?,
                num32: u32_at(body, off + 8)?,
            },
            2..=4 => RootParamKind::Descriptor {
                kind: ptype,
                register: u32_at(body, off)?,
                space: u32_at(body, off + 4)?,
            },
            other => return Err(format!("unknown root parameter type {other}")),
        };
        params.push(RootParam { visibility, kind });
    }
    let mut static_samplers = Vec::new();
    if matches!(version, 1 | 2) && num_static_samplers != 0 {
        let offset = u32_at(body, 16)? as usize;
        let bytes = body.get(offset..).ok_or("invalid static sampler offset")?;
        if num_static_samplers as usize > bytes.len() / 52 {
            return Err("truncated static sampler array".into());
        }
        for desc in bytes.chunks_exact(52).take(num_static_samplers as usize) {
            let mut words = [0; 13];
            for (word, bytes) in words.iter_mut().zip(desc.chunks_exact(4)) {
                *word = u32::from_le_bytes(bytes.try_into().unwrap());
            }
            static_samplers.push(words);
        }
    }
    Ok(RootSig { version, flags, params, num_static_samplers, static_samplers })
}
