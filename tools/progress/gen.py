"""Render the README status images.  usage: py tools/progress/gen.py

docs/img/progress.svg is a map with one square per item (green = done, amber = partial,
grey = not done), grouped the way the data is grouped:
  * imports from the test title and the Scarlett DLL audit, keyed by (module, symbol).
    Refresh scarlett-dll-imports.tsv from the per-import audit report after auditing a new build.
    Refresh scarlett-import-checks.tsv from the passing --scarlett-import-test report.
  * Direct3D 12.x interface methods (slot ranges in progress.json)
  * RDNA2 instructions: named opcode table entries plus retail scan observations. Green
    requires every scanned retail shader using the instruction to translate. Each source
    snapshot contains the MNEM rows from `xscan <file> --mnemonics 1` without the prefix.
    A passing GPU reference case also earns amber. Unidentified opcode tags have a separate row.
    Set TOMBOX_VALU_MNEMONICS to the output path when running the full valu_conformance
    test to refresh valu-mnemonics.txt. Coin Run writes coin-rdna2-coverage.tsv after
    --rdna2-test passes; copy that report here to refresh the homebrew evidence.
Badges (docs/img/badge-*.svg) carry the headline numbers.
"""
import csv, json, os, re, math
from html import escape
here = os.path.dirname(os.path.abspath(__file__))
root = os.path.join(here, "..", "..")
cfg = json.load(open(os.path.join(here, "progress.json"), encoding="utf-8"))
GREEN, AMBER, GREY, BG = "#2ea043", "#d29922", "#6e7681", "#0d1117"
COL = {"done": GREEN, "partial": AMBER, "none": GREY}

def ranges(s):
    out = set()
    for part in filter(None, s.split(",")):
        a, _, b = part.partition("-")
        out.update(range(int(a), int(b or a) + 1))
    return out

# ---- data ---------------------------------------------------------------------------
def imports():
    exe = json.load(open(os.path.join(root, "analysis", "fortnite_exe.json"), encoding="utf-8"))
    ov = cfg["import_overrides"]
    module_states = {k.lower(): v for k, v in ov.items() if k.lower().endswith(".dll")}
    pair_states = {}
    for override, state in ov.items():
        if override != "comment" and state not in {"partial", "none"}:
            raise ValueError(f"Import overrides must describe a limitation: {override}")
        mod, sep, symbol = override.partition("!")
        if sep:
            pair_states[mod.lower(), symbol] = state
    sources, names = {}, {}
    mods = [(m["module"], m["functions"]) for m in exe["imports"]]
    mods.extend((mod, ["(delay-loaded)"]) for mod in exe.get("delay_imports", []))
    for mod, funcs in mods:
        low = mod.lower()
        names[low] = mod
        for symbol in funcs:
            sources.setdefault((low, symbol), set()).add("Fortnite")

    audited, dlls = {}, set()
    with open(os.path.join(here, "scarlett-dll-imports.tsv"), encoding="utf-8") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            mod = row["dependency"].lower()
            key = (mod, row["symbol"])
            source = row["importer"].lower()
            finding = (row["provider"], row["resolution"])
            if finding[1] not in {"resolved", "missing"} or audited.setdefault(key, finding) != finding:
                raise ValueError(f"Inconsistent audit resolution for {key}: {finding}")
            dlls.add(source)
            names.setdefault(mod, row["dependency"])
            sources.setdefault(key, set()).add(source)

    host_checks = {}
    with open(os.path.join(here, "scarlett-import-checks.tsv"), encoding="utf-8") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            key = (row["dependency"].lower(), row["symbol"])
            evidence = row["evidence"].strip()
            if key in host_checks or not evidence or audited.get(key) != ("Proton Experimental", "resolved"):
                raise ValueError(f"Duplicate, undocumented or unaudited host import check: {key}")
            host_checks[key] = evidence

    verified = dict(host_checks)
    for pair, description in cfg["verified_imports"].items():
        mod, symbol = pair.split("!", 1)
        key = (mod.lower(), symbol)
        if key not in sources or not description.strip() or key in pair_states:
            raise ValueError(f"Invalid or conflicting import evidence: {pair}")
        verified.setdefault(key, description)

    groups, labels = {}, {}
    for key, callers in sorted(sources.items()):
        mod, symbol = key
        if mod.startswith(("api-ms-win-crt", "vcruntime", "msvcp")):
            group = "C/C++ runtime"
        elif mod.startswith(("api-ms-", "ext-ms-")):
            group = "Windows API sets"
        else:
            group = names[mod].rsplit(".", 1)[0]
        default = "done" if key in verified else module_states.get(mod, "partial")
        state = pair_states.get(key, ov.get(symbol, default))
        evidence = []
        if key in audited:
            provider, resolution = audited[key]
            evidence.append(f"{provider} export {resolution}")
            if resolution == "missing":
                state = "none"
        if key in host_checks:
            evidence.append("homebrew host-import test passed: " + host_checks[key])
        elif key in verified:
            evidence.append(verified[key])
        evidence.append({"done": "covered", "partial": "partial or unverified", "none": "unavailable"}[state])
        note = cfg["import_notes"].get(f"{mod}!{symbol}")
        if note:
            evidence.append(note)
        evidence.append("imported by " + ", ".join(sorted(callers)))
        groups.setdefault(group, []).append(state)
        labels.setdefault(group, []).append(f"{names[mod]}!{symbol}: " + "; ".join(evidence))
    resolved = sum(result == "resolved" for _, result in audited.values())
    detail = f" · {len(dlls)} audited DLLs · {resolved}/{len(audited)} audited imports resolve"
    return groups, labels, detail

def d3d12x():
    groups, labels = {}, {}
    for name in ("Device", "CommandList"):
        d = cfg["d3d12x"][name]
        done, part = ranges(d["done"]), ranges(d["partial"])
        unmapped = ranges(d["observed_unmapped"])
        diagnostic = ranges(d.get("diagnostic_only", ""))
        conflicts = (done & part) | (done & unmapped) | (part & unmapped) | (diagnostic & (done | part))
        if conflicts:
            raise ValueError(f"{name} slots with conflicting statuses: {sorted(conflicts)}")
        highest = max(done | part | diagnostic | unmapped, default=-1)
        count = max(d["baseline_slots"], highest + 1)
        groups[name] = ["done" if i in done else "partial" if i in part or i in diagnostic else "none" for i in range(count)]
        labels[name] = []
        for slot, state in enumerate(groups[name]):
            if name == "Device" and 91 <= slot <= 159 and state == "partial":
                evidence = "Coin Run verified caller module, slot, return address and E_NOTIMPL; method unidentified"
            elif slot in diagnostic:
                evidence = "Coin Run verified the slot, first argument, caller and zero return; no desktop command"
                if slot in unmapped:
                    evidence += "; retail call observed, behavior unmapped"
            elif name == "CommandList" and slot == 90:
                evidence = "GXDK 250405 counter clear passed GPU readback in COPY_DEST; final-batch flag unresolved"
            elif slot in unmapped:
                evidence = "retail call observed, method unmapped"
            else:
                evidence = {"done": "implemented or forwarded", "partial": "partial implementation or diagnostic binding",
                            "none": "unidentified slot"}[state]
            labels[name].append(f"{name} slot {slot}: {evidence}")
    return groups, labels

def instructions():
    src = open(os.path.join(root, "crates", "tuxbox-xshader", "src", "rdna2", "mnem.rs"), encoding="utf-8").read()
    used = {}
    for source, filename in [("Fortnite", "mnemonics.txt"), ("Xbox BVH", "bvh-mnemonics.txt"), ("FidelityFX", "fsr-mnemonics.txt")]:
        with open(os.path.join(here, filename), encoding="utf-8") as f:
            for line in f:
                name, seen, passed = line.split()
                seen, passed = int(seen), int(passed)
                counts = used.setdefault(name, {})
                if source in counts or not 0 <= passed <= seen or not seen:
                    raise ValueError(f"Invalid {source} scan row: {line}")
                counts[source] = (seen, passed)
    unknown = {n: used.pop(n) for n in list(used) if "<" in n}
    with open(os.path.join(here, "valu-mnemonics.txt"), encoding="utf-8") as f:
        vector = set(f.read().splitlines())
    with open(os.path.join(here, "coin-rdna2-coverage.tsv"), encoding="utf-8") as f:
        results = list(csv.DictReader(f, delimiter="\t"))
    game = {r["mnemonic"] for r in results if r["coin_run_alu"] == "true"}
    memory = {r["mnemonic"] for r in results if r["memory_probe"] == "true"}
    graphics = {r["mnemonic"] for r in results if r["graphics_probe"] == "true"}
    cache = {r["mnemonic"]: r["cache_probe"] for r in results if r["cache_probe"] != "-"}
    probed = vector | game | memory | graphics
    families, cur = {}, None
    for line in src.split("\n"):
        line = line.partition("//")[0]
        m = re.match(r"pub fn (\w+)\(", line)
        if m:
            cur = m.group(1).upper()
        enc = re.match(r"\s*Enc::(\w+) =>", line)
        if enc:
            cur = enc.group(1).upper()
        # Includes compare predicate arrays and Some("mnemonic") entries in lookup().
        for n in re.findall(r'"([a-z0-9_]+)"', line):
            families.setdefault(n, cur)
    for n in used:
        families.setdefault(n, "MIMG" if n.startswith("image_") else "OTHER")
    if probed - families.keys():
        raise ValueError(f"GPU results outside tracked instructions: {sorted(probed - families.keys())}")
    groups, labels = {}, {}
    for n, family in families.items():
        counts = used.get(n, {})
        complete = counts and all(ok == seen for seen, ok in counts.values())
        state = "done" if complete else "partial" if counts or n in probed else "none"
        groups.setdefault(family, []).append(state)
        evidence = [f"{ok}/{seen} {source} shaders translated" for source, (seen, ok) in counts.items()]
        if n in vector:
            evidence.append("vector ALU GPU probe passed")
        if n in game:
            evidence.append("Coin Run Xbox shader readback passed")
        if n in memory:
            evidence.append("vector memory GPU probe passed")
        if n in graphics:
            evidence.append("Coin Run rendered the padded shader and verified its executable boundary"
                            if n == "s_code_end" else "Coin Run graphics pixel reference passed")
        if n in cache:
            evidence.append({"uav_visibility": "cross-wave UAV visibility passed", "state_preserved": "instruction-prefetch state preservation passed"}[cache[n]])
        scope = {
            "image_get_resinfo": "2D width, height and mip count",
            "image_store_mip": "UAV view-relative mip zero",
            "image_sample_c_l": "statically zero compare LOD",
            "s_round_mode": "MODE field and FP32-to-FP16 conversion",
            "s_denorm_mode": "MODE field and FP32-to-FP16 conversion",
        }.get(n)
        if scope and n in probed:
            evidence.append(scope)
        labels.setdefault(family, []).append(f"{n}: {', '.join(evidence) or 'no retail or GPU probe evidence'}")
    detail = (f" · GPU checks {len(probed & used.keys())}/{len(used)} retail"
              f" · {len(cache)} cache probes · {len(probed - used.keys())} others checked")
    tag_groups, tag_labels = {}, {}
    for tag, counts in sorted(unknown.items()):
        family = tag.split("<", 1)[0].upper()
        evidence = ", ".join(f"{ok}/{seen} {source} shaders translated" for source, (seen, ok) in counts.items())
        tag_groups.setdefault(family, []).append("none")
        tag_labels.setdefault(family, []).append(f"{tag}: unidentified; {evidence}")
    return groups, labels, detail, tag_groups, tag_labels

# ---- drawing ------------------------------------------------------------------------
def squarify(items, x, y, w, h):
    """items: [(key, area)] sorted descending; returns [(key, x, y, w, h)]."""
    out, items = [], list(items)
    total = sum(a for _, a in items)
    scale = w * h / total
    items = [(k, a * scale) for k, a in items]
    def worst(row, side):
        s = sum(a for _, a in row)
        return max(max(side * side * a / (s * s), s * s / (side * side * a)) for _, a in row)
    while items:
        side = min(w, h)
        row = [items.pop(0)]
        while items and worst(row + [items[0]], side) <= worst(row, side):
            row.append(items.pop(0))
        s = sum(a for _, a in row)
        if w >= h:
            cw, cy = s / h, y
            for k, a in row:
                out.append((k, x, cy, cw, a / cw)); cy += a / cw
            x += cw; w -= cw
        else:
            ch, cx = s / w, x
            for k, a in row:
                out.append((k, cx, y, a / ch, ch)); cx += a / ch
            y += ch; h -= ch
    return out

def text(x, y, s, size=12):
    return (f'<text x="{x:.2f}" y="{y:.2f}" font-family="sans-serif" font-size="{size}" fill="#ffffff" stroke="{BG}" '
            f'stroke-width="3" paint-order="stroke" pointer-events="none">{escape(s)}</text>')

def section(o, y, title, groups, height, detail="", labels=None, show_progress=True, subtitle=""):
    n = sum(len(v) for v in groups.values())
    done = sum(v.count("done") for v in groups.values())
    count = f"{100 * done / n:.2f}% ({done}/{n})" if show_progress else str(n)
    o.append(text(4, y + 21, f"{title}: {count}{detail}", 16))
    y += 30
    if subtitle:
        o.append(text(4, y + 12, subtitle, 13))
        y += 20
    order = sorted(groups.items(), key=lambda kv: -len(kv[1]))
    for key, cx, cy, cw, ch in squarify([(k, len(v)) for k, v in order], 0, y, 1000, height):
        st = groups[key]
        cnt = len(st)
        ix, iy, iw, ih = cx + 1, cy + 1, cw - 2, ch - 2
        cols = max(1, min(cnt, math.ceil(math.sqrt(cnt * iw / ih))))
        rows = math.ceil(cnt / cols)
        sw, sh = iw / cols, ih / rows
        summary = f'{st.count("done")}/{cnt} ({100 * st.count("done") // cnt}%)' if show_progress else str(cnt)
        o.append(f'<g><title>{escape(key)}: {summary}</title>')
        for i, j in enumerate(sorted(range(cnt), key=lambda j: ["done", "partial", "none"].index(st[j]))):
            end = f'><title>{escape(labels[key][j])}</title></rect>' if labels else '/>'
            o.append(f'<rect x="{ix + (i % cols) * sw:.2f}" y="{iy + (i // cols) * sh:.2f}" width="{sw:.2f}" height="{sh:.2f}" '
                     f'fill="{COL[st[j]]}" stroke="{BG}" stroke-width="0.5"{end}')
        o.append(f'<rect x="{cx:.2f}" y="{cy:.2f}" width="{cw:.2f}" height="{ch:.2f}" fill="none" stroke="{BG}" stroke-width="2"/>')
        label = key[: max(0, int((cw - 6) / 7))]
        if label and ch >= 18:
            o.append(text(cx + 5, cy + 16, label))
        o.append("</g>")
    return y + height + 10, done, n

def badge(name, label, value, colour):
    lw, vw = 7 * len(label) + 12, 7 * len(value) + 12
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{lw + vw}" height="20" role="img" aria-label="{label}: {value}">'
           f'<rect width="{lw}" height="20" fill="#555"/><rect x="{lw}" width="{vw}" height="20" fill="{colour}"/>'
           f'<g fill="#fff" font-family="Verdana,DejaVu Sans,sans-serif" font-size="11" text-anchor="middle">'
           f'<text x="{lw / 2}" y="14">{label}</text><text x="{lw + vw / 2}" y="14">{value}</text></g></svg>')
    open(os.path.join(root, "docs", "img", f"badge-{name}.svg"), "w", encoding="utf-8").write(svg)

def pct_colour(p):
    return "#4c1" if p >= 90 else "#97ca00" if p >= 70 else "#dfb317" if p >= 40 else "#fe7d37"

o, y = [], 0
import_groups, import_labels, import_detail = imports()
y, a_done, a_n = section(o, y, "Observed title and middleware imports", import_groups, 300, import_detail, import_labels)
d3d_groups, d3d_labels = d3d12x()
y, b_done, b_n = section(o, y, "Direct3D 12.x interface methods", d3d_groups, 170, labels=d3d_labels)
isa_groups, isa_labels, isa_detail, tag_groups, tag_labels = instructions()
kernel_results = []
for library, counts in cfg["audited_compute_shaders"].items():
    passed, total = counts["readback"], counts["observed"]
    if not 0 <= passed <= total or not total:
        raise ValueError(f"Invalid compute shader counts for {library}")
    kernel_results.append(f"{library} {passed}/{total}")
y, c_done, c_n = section(o, y, "RDNA2 in translated retail shaders", isa_groups, 260,
                         isa_detail, isa_labels, subtitle="Retail compute shaders passing GPU readback · " + " · ".join(kernel_results))
if tag_groups:
    y, _, _ = section(o, y, "Unidentified retail opcode tags", tag_groups, 55, labels=tag_labels, show_progress=False)
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 {y:.0f}" width="1000" height="{y:.0f}">\n'
       f'<rect x="0" y="0" width="1000" height="{y:.0f}" fill="{BG}"/>\n' + "\n".join(o) + "\n</svg>\n")
open(os.path.join(root, "docs", "img", "progress.svg"), "w", encoding="utf-8").write(svg)
m, s = cfg["milestones"], cfg["shaders"]
badge("imports", "import coverage", f"{100 * a_done / a_n:.2f}%", pct_colour(100 * a_done / a_n))
badge("d3d12x", "Direct3D 12.x methods", f"{100 * b_done / b_n:.2f}%", pct_colour(100 * b_done / b_n))
badge("instructions", "RDNA2 instructions", f"{100 * c_done / c_n:.2f}%", pct_colour(100 * c_done / c_n))
badge("shaders", "Fortnite shaders translated", f'{s["translated"]:,}/{s["total"]:,}', "#4c1")
badge("milestones", "milestones", f'{m["done"]}/{m["total"]}', pct_colour(100 * m["done"] / m["total"]))
print(f"imports {a_done}/{a_n}  d3d12x {b_done}/{b_n}  instructions {c_done}/{c_n}  height {y:.0f}")
