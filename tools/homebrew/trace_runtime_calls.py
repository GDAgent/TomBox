"""Instrument direct calls in the homebrew fixture to observed Scarlett runtime exports.

Compile the instrumented IR with the same clang that emitted it. The callback
compares the actual imported address with the supplied DLL before crediting it.
"""
import csv
import json
from pathlib import Path
import re
import sys

source, target = map(Path, sys.argv[1:])
root = Path(__file__).resolve().parents[2]
native = {"msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"}
pairs = set()
for mod in json.loads((root / "analysis/fortnite_exe.json").read_text())["imports"]:
    if mod["module"].lower() in native:
        pairs.update((mod["module"].lower(), name) for name in mod["functions"])
with (root / "tools/progress/scarlett-dll-imports.tsv").open() as inventory:
    for row in csv.DictReader(inventory, delimiter="\t"):
        if row["dependency"].lower() in native:
            pairs.add((row["dependency"].lower(), row["symbol"]))

# LLVM prints MSVC decorated identifiers in quotes and C identifiers unquoted.
identifier = r'@(?:"([^"]+)"|([\w.$]+))\('
call = re.compile(r'\b(?:call|invoke)\b[^@\n]*' + identifier)
lines = source.read_text().splitlines()
imports = set()
for line in lines:
    if line.startswith(("declare dllimport ", "declare dso_local ")):
        match = re.search(identifier, line)
        if match:
            imports.add(match[1] or match[2])
targets = {}
for dll, name in sorted(pairs):
    if name in imports:
        if name in targets:
            raise ValueError(f"Ambiguous runtime provider for {name}")
        targets[name] = dll

out = []
constants = {}
def literal(s):
    if s not in constants:
        if not s.isascii() or '"' in s or '\\' in s:
            raise ValueError(s)
        constants[s] = f"@runtime_literal_{len(constants)}"
    return constants[s]

for line in lines:
    if line.startswith("declare dso_local "):
        decl = re.search(identifier, line)
        if decl and (decl[1] or decl[2]) in targets:
            # Some MSVC headers omit dllimport and ordinarily call a linker thunk.
            # Import the same function directly so its provider can be checked.
            line = line.replace("declare dso_local ", "declare dllimport ", 1)
    found = call.search(line)
    if found:
        name = found[1] or found[2]
        if name in targets:
            bundle = re.search(r'( \[ "funclet"\(token %[^)]+\) \])', line)
            out.append(f'  call void @runtime_hit(ptr {literal(targets[name])}, ptr {literal(name)}, '
                       f'ptr @"{name}") nounwind' + (bundle[1] if bundle else ""))
    out.append(line)
if not constants:
    raise ValueError("No observed runtime calls found in fixture IR")
if not any(line.startswith("declare ") and "@runtime_hit(" in line for line in lines):
    out.append("declare void @runtime_hit(ptr, ptr, ptr) nounwind")
for s, symbol in constants.items():
    out.append(f'{symbol} = private unnamed_addr constant [{len(s)+1} x i8] c"{s}\\00"')
target.write_text("\n".join(out) + "\n")
