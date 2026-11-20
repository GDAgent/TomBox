//! Opcode census over shader containers: `opstat <files...>` prints (encoding, opcode,
//! mnemonic, count) for every instruction found, so opcode tables can be checked against
//! what real shaders contain.
use std::collections::BTreeMap;
use tombox_xshader::{extract, rdna2::disassemble};
fn main() {
    let mut c: BTreeMap<(String, u32, String), usize> = BTreeMap::new();
    let mut n = 0;
    for p in std::env::args().skip(1) {
        let Ok(b) = std::fs::read(&p) else { continue };
        let isa = if p.ends_with(".bin") { b } else { match extract(&b) { Ok(e) => e.isa, Err(_) => continue } };
        n += 1;
        for i in disassemble(&isa) {
            *c.entry((format!("{:?}", i.enc), i.op, i.mnemonic.clone())).or_default() += 1;
        }
    }
    println!("{n} shaders");
    for ((e, op, m), k) in c {
        println!("{e:6} {op:#05x} {m:28} {k}");
    }
}
