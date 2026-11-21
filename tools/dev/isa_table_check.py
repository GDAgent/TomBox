#!/usr/bin/env python3
"""Compare tuxbox-xshader's opcode -> mnemonic tables with AMD's machine-readable ISA specs.

Usage: python -I tools/dev/isa_table_check.py <dir with amdgpu_isa_rdna1.xml and amdgpu_isa_rdna2.xml>

The XML files come from https://gpuopen.com/machine-readable-isa/ (not redistributed here).
An entry passes when its mnemonic matches the RDNA1 (GFX10.1) or the RDNA2 (GFX10.3)
reference. Both are accepted because Xbox Series shaders contain GFX10.1-only opcodes (VOP2
v_mac_f32 / v_madak_f32, VOP3 v_mad_f32 / v_mad_legacy_f32) next to RDNA2 ones, so neither
reference alone describes the console's ISA. Exit status 1 when any entry differs.
"""
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ENCODINGS = {
    "vop2": ["ENC_VOP2", "VOP2_INST_LITERAL"],
    "vop1": ["ENC_VOP1"],
    "vopc": ["ENC_VOPC"],
    "vop3": ["ENC_VOP3", "VOP3_SDST_ENC", "VOP3_INST_LITERAL"],
    "sop2": ["ENC_SOP2"],
    "sop1": ["ENC_SOP1"],
    "sopk": ["ENC_SOPK", "SOPK_INST_LITERAL"],
    "sopc": ["ENC_SOPC"],
    "sopp": ["ENC_SOPP"],
    "smem": ["ENC_SMEM"],
    "mimg": ["ENC_MIMG"],
    "mubuf": ["ENC_MUBUF"],
    "Enc::Flat": ["ENC_FLAT", "ENC_FLAT_GLBL", "ENC_FLAT_SCRATCH"],
    "Enc::Ds": ["ENC_DS"],
}


def load_spec(xml_dir: Path) -> dict:
    spec: dict = {}
    for name in ("amdgpu_isa_rdna1.xml", "amdgpu_isa_rdna2.xml"):
        root = ET.parse(xml_dir / name).getroot()
        for ins in root.iter("Instruction"):
            mnem = ins.findtext("InstructionName").lower()
            for enc in ins.iter("InstructionEncoding"):
                op = int(enc.findtext("Opcode"))
                spec.setdefault(enc.findtext("EncodingName"), {}).setdefault(op, set()).add(mnem)
    return spec


def vopc_table(src: str) -> list:
    """Evaluate the computed vopc() table (const arrays + ranges) into (op, mnemonic) pairs."""
    body = src[src.index("pub fn vopc("):src.index("pub fn vop3(")]
    arrays = {n: re.findall(r'"([a-z0-9_]+)"', a) for n, a in re.findall(r"const (\w+): \[&str; \d+\] = \[(.*?)\];", body, re.S)}
    pairs = []
    for lo, hi, arr, base, extra in re.findall(r"(0x[0-9a-f]+)\.\.=(0x[0-9a-f]+) => (\w+)\[\(?(?:op - (0x[0-9a-f]+))?(?: \+ (\d+))?", body):
        for op in range(int(lo, 16), int(hi, 16) + 1):
            idx = op - (int(base, 16) if base else 0) + (int(extra) if extra else 0)
            pairs.append((op, arrays[arr][idx]))
    for op, m in re.findall(r'(0x[0-9a-f]+) => "([a-z0-9_]+)"', body):
        pairs.append((int(op, 16), m))
    return pairs


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    spec = load_spec(Path(sys.argv[1]))
    src = (Path(__file__).resolve().parents[2] / "crates/tuxbox-xshader/src/rdna2/mnem.rs").read_text(encoding="utf-8")
    bad = 0
    for section, encs in ENCODINGS.items():
        if section == "vopc":
            pairs = vopc_table(src)
        elif section.startswith("Enc::"):
            lk = src[src.index("pub fn lookup"):]
            body = lk[lk.index(section):]
            body = body[:body.index("_ =>")]
            pairs = [(int(o, 16), m) for o, m in re.findall(r'(0x[0-9a-fA-F]+)\s*=>\s*Some\("([a-z0-9_]+)"\)', body)]
        else:
            body = src[src.index(f"pub fn {section}("):]
            body = body[:body.index("\n}\n")]
            pairs = [(int(o, 16), m) for o, m in re.findall(r'(0x[0-9a-fA-F]+)\s*=>\s*"([a-z0-9_]+)"', body)]
        for op, m in pairs:
            if section == "vop3" and op >= 0x400:  # VOP3P, kept apart by +0x400
                names = spec.get("ENC_VOP3P", {}).get(op - 0x400, set())
            else:
                names = set().union(*(spec.get(e, {}).get(op, set()) for e in encs))
            # FLAT/GLOBAL/SCRATCH share opcodes; compare without the segment prefix.
            if m in names or any(m.split("_", 1)[-1] == n.split("_", 1)[-1] for n in names):
                continue
            print(f"{section} {op:#x}: table says {m}, references say {sorted(names) or 'nothing'}")
            bad += 1
        print(f"{section}: {len(pairs)} entries checked")
    print("all entries match" if bad == 0 else f"{bad} entries differ")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
