//! Opcode census over shader containers: `opstat <files...>` prints (encoding, opcode,
//! mnemonic, count) for every instruction found, so opcode tables can be checked against
//! what real shaders contain. MIMG instructions that set a bit changing their operand or
//! result layout are listed separately, tagged `[d16 a16 unorm tfe lwe]`; MUBUF likewise
//! (`[lds tfe]`).
use std::collections::BTreeMap;
use tombox_xshader::{extract, rdna2::{disassemble, Enc}};
fn main() {
    let mut c: BTreeMap<(String, u32, String), usize> = BTreeMap::new();
    let mut n = 0;
    for p in std::env::args().skip(1) {
        let Ok(b) = std::fs::read(&p) else { continue };
        let isa = if p.ends_with(".bin") { b } else { match extract(&b) { Ok(e) => e.isa, Err(_) => continue } };
        n += 1;
        for i in disassemble(&isa) {
            let mut m = i.mnemonic.clone();
            if i.enc == Enc::Mubuf {
                let (w0, w1) = (i.raw.first().copied().unwrap_or(0), i.raw.get(1).copied().unwrap_or(0));
                let bits = [(w0 >> 16, "lds"), (w1 >> 23, "tfe")];
                let tags: Vec<&str> = bits.iter().filter(|(b, _)| b & 1 != 0).map(|(_, t)| *t).collect();
                if !tags.is_empty() {
                    m = format!("{m} [{}]", tags.join(" "));
                }
            }
            if i.enc == Enc::Mimg {
                let (w0, w1) = (i.raw.first().copied().unwrap_or(0), i.raw.get(1).copied().unwrap_or(0));
                let bits = [(w1 >> 31, "d16"), (w1 >> 30, "a16"), (w0 >> 12, "unorm"), (w0 >> 16, "tfe"), (w0 >> 17, "lwe")];
                let tags: Vec<&str> = bits.iter().filter(|(b, _)| b & 1 != 0).map(|(_, t)| *t).collect();
                if !tags.is_empty() {
                    m = format!("{m} [{}]", tags.join(" "));
                }
            }
            *c.entry((format!("{:?}", i.enc), i.op, m)).or_default() += 1;
        }
    }
    println!("{n} shaders");
    for ((e, op, m), k) in c {
        println!("{e:6} {op:#05x} {m:40} {k}");
    }
}
