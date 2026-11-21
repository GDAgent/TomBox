//! Native compute executables carried in the ACCC chunk of Scarlett DXBCs.
use super::{rdna2, trim_to_endpgm, u32_at, NativeProgram, Stage};

pub(super) fn extract(body: &[u8], threads: [u32; 3]) -> Result<NativeProgram, String> {
    let (config, code) = compute(body)?;
    let isa = trim_to_endpgm(code);
    let last = rdna2::disassemble(&isa).pop().ok_or("empty FSPX code")?;
    if last.mnemonic != "s_endpgm" || last.offset + 4 != isa.len() {
        return Err("FSPX code has no aligned endpgm".into());
    }
    Ok(NativeProgram {
        stage: Stage::Compute,
        sm: (0, 0),
        bitcode_stripped: false,
        fssm: Vec::new(),
        isa_off: 0,
        isa_preamble: [0, threads[0], threads[1], threads[2]],
        isa,
        isa_data: code.to_vec(),
        fspx_config: Some(config),
    })
}

fn compute(body: &[u8]) -> Result<([u32; 7], &[u8]), String> {
    if body.get(..4) != Some(b"FSPX")
        || u32_at(body, 4)? != 1
        || u32_at(body, 12)? as usize != body.len()
    {
        return Err("invalid FSPX v1 header".into());
    }
    let count = u32_at(body, 8)? as usize;
    if count > (body.len() - 16) / 16 {
        return Err("truncated FSPX directory".into());
    }
    let directory_end = 16 + 16 * count;
    let mut config = None;
    let mut code = None;
    for entry in body[16..directory_end].chunks_exact(16) {
        let tag = u32_at(entry, 0)?;
        let offset = u32_at(entry, 4)? as usize;
        let size = u32_at(entry, 8)? as usize;
        if offset < directory_end || offset > body.len() || size > body.len() - offset {
            return Err("FSPX section outside payload".into());
        }
        let part = &body[offset..offset + size];
        match tag {
            0 if size == 28 && config.is_none() => {
                let mut words = [0; 7];
                for (i, w) in words.iter_mut().enumerate() {
                    *w = u32_at(part, i * 4)?;
                }
                config = Some(words);
            }
            6 if offset % 4 == 0 && size % 4 == 0 && size > 0 && code.is_none() => {
                code = Some(part)
            }
            0 | 6 => return Err("invalid or duplicate FSPX compute section".into()),
            _ => {}
        }
    }
    Ok((
        config.ok_or("missing FSPX configuration")?,
        code.ok_or("missing FSPX ISA")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(code: &[u32]) -> Vec<u8> {
        let size = code.len() as u32 * 4;
        let mut words = vec![
            u32::from_le_bytes(*b"FSPX"),
            1,
            2,
            76 + size,
            0,
            48,
            28,
            0,
            6,
            76,
            size,
            0,
        ];
        words.extend(0..7);
        words.extend_from_slice(code);
        words.into_iter().flat_map(u32::to_le_bytes).collect()
    }

    #[test]
    fn program_ends_before_padding_and_embedded_data() {
        let blob = packet(&[
            0xbf80_0000,
            super::super::S_ENDPGM,
            super::super::S_CODE_END,
            42,
        ]);
        let native = extract(&blob, [32, 1, 1]).unwrap();
        assert_eq!(native.isa, blob[76..84]);
        assert_eq!(native.isa_data, blob[76..]);
        assert_eq!(native.fspx_config, Some([0, 1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn rejects_invalid_sections_and_literal_terminator() {
        let blob = packet(&[super::super::S_ENDPGM]);
        for (off, value) in [
            (4, 2u32),
            (8, u32::MAX),
            (12, 8),
            (32, 0),
            (36, 24),
            (40, u32::MAX),
        ] {
            let mut corrupt = blob.clone();
            corrupt[off..off + 4].copy_from_slice(&value.to_le_bytes());
            assert!(compute(&corrupt).is_err(), "directory word at {off}");
        }
        // s_mov_b32 s0, literal: the literal word is data, not a terminating instruction.
        assert!(extract(&packet(&[0xbe8003ff, super::super::S_ENDPGM]), [32, 1, 1]).is_err());
    }
}
