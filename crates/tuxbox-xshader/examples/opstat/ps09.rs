//! Inventory compare instructions in PS09 records with a recognized program boundary.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, ErrorKind, Read};
use tuxbox_xshader::rdna2::{self, Enc};

const MAX_CACHE: u64 = 512 * 1024 * 1024;
const IDENT_END: [u8; 12] = [0, 0, 0x80, 0xd5, 0, 1, 0xfe, 3, 0xc0, 0xa, 0, 0];

fn invalid() -> io::Error {
    io::Error::new(ErrorKind::InvalidData, "invalid PS09 cache range")
}

fn word(data: &[u8], pos: usize) -> io::Result<usize> {
    let bytes = data
        .get(pos..)
        .and_then(|b| b.first_chunk())
        .ok_or_else(invalid)?;
    Ok(u32::from_le_bytes(*bytes) as usize)
}

// The identity starts with two three-dword V_NOPs, each carrying a literal.
// Some cache records have a third fixed V_NOP before the executable.
fn program_start(record: &[u8]) -> Option<usize> {
    record.get(..24)?;
    for pos in [0, 12] {
        if word(record, pos).ok()? >> 16 != 0xd580
            || (word(record, pos + 4).ok()? >> 18) & 0x1ff != 255
        {
            return None;
        }
    }
    Some(if record.get(24..36) == Some(&IDENT_END) {
        36
    } else {
        24
    })
}

/// Return at an aligned ENDPGM after all forward SOPP targets, before padding
/// or constants. Refuse records with unknown instruction lengths.
fn compares(code: &[u8]) -> Option<BTreeMap<&'static str, usize>> {
    let mut found = BTreeMap::new();
    let mut forward = 0;
    for ins in rdna2::instructions(code) {
        if ins.enc == Enc::Unknown || ins.mnemonic == "s_code_end" {
            break;
        }
        if ins.enc == Enc::Sopp {
            if matches!(ins.op, 2 | 4..=9) {
                let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
                if target < 0 || target as usize >= code.len() {
                    break;
                }
                forward = forward.max(target as usize);
            } else if ins.raw == [0xbf81_0000] && ins.offset >= forward {
                return Some(found);
            }
        }
        if matches!(ins.enc, Enc::Vopc | Enc::Vop3) && ins.op < 256 {
            if let Some(name) = rdna2::mnem::vopc(ins.op) {
                found.entry(name).or_insert(ins.offset);
            }
        }
    }
    None
}

// A vertex fetch subroutine has no shader identity prologue or ENDPGM. Accept
// only straight-line descriptor setup and typed fetches ending in a return to
// s[4:5] at the *exact* record boundary. None of these instructions is VOPC.
fn is_fetch_subroutine(code: &[u8]) -> bool {
    let mut saw_fetch = false;
    for ins in rdna2::instructions(code) {
        if ins.raw == [0xbe84_2004] {
            return saw_fetch && ins.offset + 4 == code.len();
        }
        if ins.enc == Enc::Mtbuf {
            saw_fetch = true;
        } else if !matches!(
            ins.mnemonic.as_str(),
            "s_mov_b32" | "s_movk_i32" | "v_add_nc_u32" | "s_load_dwordx4" | "s_waitcnt"
        ) {
            return false;
        }
    }
    false
}

pub(super) fn scan(selected: &[usize]) -> io::Result<()> {
    let mut data = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_CACHE + 1)
        .read_to_end(&mut data)?;
    if data.get(..4) != Some(b"PS\0\x09") || data.len() as u64 > MAX_CACHE {
        return Err(invalid());
    }
    let shaders = word(&data, 0x5c)?;
    // File counts are u32. Calculate table extents in u64 before narrowing.
    let table = 128
        + 112 * word(&data, 0x50)? as u64
        + 24 * word(&data, 0x54)? as u64
        + 8 * word(&data, 0x58)? as u64;
    let base = table + 8 * shaders as u64 + word(&data, 0x60)? as u64;
    if base + word(&data, 0x64)? as u64 != data.len() as u64 {
        return Err(invalid());
    }

    let selection: BTreeSet<_> = selected.iter().copied().collect();
    if selection.iter().any(|&index| index >= shaders) {
        return Err(invalid());
    }
    if !selection.is_empty() {
        println!("shader_index\tmnemonic\tbyte_offset_after_prologue");
    }
    let mut counts = BTreeMap::new();
    let mut helpers = 0;
    let (mut skipped_header, mut skipped_body) = (0, 0);
    for index in 0..shaders {
        if !selection.is_empty() && !selection.contains(&index) {
            continue;
        }
        let record = table as usize + 8 * index;
        let start = (base as usize)
            .checked_add(word(&data, record)?)
            .ok_or_else(invalid)?;
        let size = word(&data, record + 4)?;
        let code = data
            .get(start..)
            .and_then(|s| s.get(..size))
            .ok_or_else(invalid)?;
        if size % 4 != 0 {
            return Err(invalid());
        }
        let found = if let Some(entry) = program_start(code) {
            compares(&code[entry..])
        } else if is_fetch_subroutine(code) {
            helpers += 1;
            Some(BTreeMap::new())
        } else {
            skipped_header += 1;
            None
        };
        let Some(found) = found else {
            if !selection.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("no validated program boundary for shader {index}"),
                ));
            }
            if program_start(code).is_some() {
                skipped_body += 1;
            }
            continue;
        };
        if !selection.is_empty() && found.is_empty() {
            println!("{index}\t-\t-");
        }
        for (name, pc) in found {
            if !selection.is_empty() {
                println!("{index}\t{name}\t{pc}");
            }
            let row = counts.entry(name).or_insert((0, index, pc));
            row.0 += 1;
        }
    }
    if !selection.is_empty() {
        return Ok(());
    }
    eprintln!("{} / {shaders} records scanned (including {helpers} headerless fetch subroutines); {skipped_header} unrecognized headers, {skipped_body} unresolved instruction streams",
        shaders - skipped_header - skipped_body);
    if skipped_header != 0 || skipped_body != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "incomplete PS09 inventory cannot establish a per-instruction denominator",
        ));
    }
    println!("mnemonic\tshaders\tfirst_shader\tbyte_offset_after_prologue");
    for (name, (count, index, pc)) in counts {
        println!("{name}\t{count}\t{index}\t{pc}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &[u32]) -> Vec<u8> {
        input.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn truncated_identity_has_no_entry_point() {
        let short = words(&[0xd5800000, 0x03fe0100, 0, 0xd5800000, 0x03fe0100]);
        assert_eq!(program_start(&short), None);
    }

    #[test]
    fn fetch_subroutine_requires_a_terminal_return() {
        let code = words(&[0xea522000, 0x8003040c, 0xbe842004]);
        assert!(is_fetch_subroutine(&code));
        assert!(!is_fetch_subroutine(&code[..8]));
        assert!(!is_fetch_subroutine(&[code, words(&[0x7c020280])].concat()));
    }

    #[test]
    fn excludes_literals_and_post_program_data() {
        let code = words(&[0x7e00_02ff, 0x7c02_0280, 0xbf81_0000, 0x7c02_0280]);
        assert!(compares(&code).unwrap().is_empty());
    }

    #[test]
    fn scans_forward_branch_past_early_exit() {
        let code = words(&[0xbf84_0001, 0xbf81_0000, 0x7c02_0280, 0xbf81_0000]);
        assert_eq!(compares(&code).unwrap().get("v_cmp_lt_f32"), Some(&8));
        assert!(compares(&code[..12]).is_none());
    }
}
