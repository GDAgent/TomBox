"""Inventory an extracted Scarlett package and compare PE imports with TuxBox's audit."""
import argparse
import csv
import hashlib
import json
import subprocess
import zlib
from collections import Counter, defaultdict
from pathlib import Path, PureWindowsPath
import xml.etree.ElementTree as ET

from package_assets import audit_assets, probe_reads
from package_gpu import probe_package_gpu

REPO = Path(__file__).resolve().parents[2]


def package_key(name):
    path = PureWindowsPath(name)
    if path.drive or ".." in path.parts:
        raise ValueError(f"Expected a package-relative path, got {name!r}")
    return path.as_posix().lstrip("/").casefold()


def layout_files(layout, files):
    chunks = []
    for chunk in ET.parse(layout).findall("Chunk"):
        expected = []
        for group in chunk.findall("FileGroup"):
            name = group.attrib["Include"]
            if any(c in name for c in "*?") or "Exclude" in group.attrib:
                raise ValueError(f"Layout needs file-group expansion for {name!r}")
            dest = PureWindowsPath(group.attrib["DestinationPath"]) / name
            expected.append(package_key(dest))
        chunks.append({"id": chunk.attrib["Id"], "marker": chunk.get("Marker"),
                       "files": len(expected), "missing": [n for n in expected if n not in files]})
    return chunks


def inspect(tool, path):
    result = subprocess.run([tool, path, "--json"], capture_output=True, text=True)
    if result.returncode:
        raise ValueError(f"{path.name}: {result.stderr.strip()}")
    pe = json.loads(result.stdout)
    if "delay_import_details" not in pe:
        raise ValueError("Rebuild pe-inspect with cargo build --release -p tuxbox-cli")
    return pe


def imports(pe, field):
    return {(dll["module"].casefold(), symbol)
            for dll in pe[field] for symbol in dll["functions"]}


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def audit_package(root, tool, probe_asset_io=False, probe_asset_gpu=False):
    files = {}
    for entry in root.rglob("*"):
        if entry.is_file():
            key = package_key(entry.relative_to(root))
            if key in files:
                raise ValueError(f"Ambiguous package path: {key}")
            files[key] = entry
    config = ET.parse(files["microsoftgame.config"])
    appx = ET.parse(files["appxmanifest.xml"])
    apps = appx.findall("./{*}Applications/{*}Application")
    audit = defaultdict(set)
    with (REPO / "tools/progress/scarlett-dll-imports.tsv").open() as f:
        for row in csv.DictReader(f, delimiter="\t"):
            audit[row["importer"].casefold()].add((row["dependency"].casefold(), row["symbol"]))
    observed = set().union(*audit.values())
    fortnite = json.loads((REPO / "analysis/fortnite_exe.json").read_text())
    observed.update(imports(fortnite, "imports"))

    layouts = {path.name: layout_files(path, files) for key, path in files.items()
               if "/" not in key and key.startswith("layout_") and key.endswith(".xml")}
    library_checks = {}
    for key, path in sorted(files.items()):
        if not key.endswith(".dll"):
            continue
        pe = inspect(tool, path)
        ordinary = imports(pe, "imports")
        previous = audit.get(PureWindowsPath(key).name)
        library_checks[key] = {
            "sha256": file_hash(path),
            "import_count": sum(len(m["functions"]) for m in pe["imports"]),
            "prior_audit_available": previous is not None,
            "added_since_audit": sorted(ordinary - previous) if previous is not None else None,
            "removed_since_audit": sorted(previous - ordinary) if previous is not None else None,
            "delay_imports": pe["delay_import_details"], "parser_notes": pe["notes"],
        }

    titles = []
    for exe in config.findall("./ExecutableList/Executable"):
        key = package_key(exe.attrib["Name"])
        application = next((app for app in apps if package_key(app.attrib["Executable"]) == key
                            and app.get("Id") == exe.get("Id")), None)
        item = {"path": key, "target": exe.get("TargetDeviceFamily"), "id": exe.get("Id"),
                "appx_entry_point": application.get("EntryPoint") if application is not None else None,
                "present": key in files}
        if key in files:
            pe = inspect(tool, files[key])
            item["sha256"] = file_hash(files[key])
            item["pe"] = {field: pe[field] for field in ("machine_name", "is_dll", "subsystem_name",
                          "entry_point_rva", "tls", "notes", "imports", "delay_import_details")}
            for phase, field in (("ordinary", "imports"), ("delay", "delay_import_details")):
                item[f"new_{phase}_imports"] = sorted(imports(pe, field) - observed)
        titles.append(item)
    if not titles:
        raise ValueError("MicrosoftGame.config contains no Executable entries")

    identity = config.find("Identity")
    appx_identity = appx.find("./{*}Identity")
    if identity is None or appx_identity is None:
        raise ValueError("Package identity missing from game or Appx manifest")
    archives = sorted(p for p in files.values() if p.suffix.lower() in {".rpf", ".ttpf"})
    assets = audit_assets(root, archives)
    if probe_asset_io and archives:
        assets["proton_win32_reads"] = probe_reads(REPO, root, archives)
    if probe_asset_gpu:
        assets["gpu_readback"] = probe_package_gpu(REPO, root)
    return {
        "assets": assets, "identity": identity.attrib, "appx_identity": appx_identity.attrib,
        "title_id": config.findtext("TitleId"),
        "file_types": dict(sorted(Counter(p.suffix.lower() for p in files.values()).items())),
        "executables": titles,
        "storage_mb": config.findtext("PersistentLocalStorage/SizeMB"),
        "virtual_machine": {e.tag: e.text for e in config.findall("VirtualMachine/*")},
        "supported_users": appx.findtext("./{*}Properties/{*}SupportedUsers"),
        "os_dependencies": [e.attrib for e in appx.findall("./{*}Dependencies/*")],
        "capabilities": [e.get("Name") for e in appx.findall("./{*}Capabilities/*")],
        "layouts": layouts, "libraries": library_checks,
        "absent_audited_dll_names": sorted(audit.keys() - {PureWindowsPath(k).name for k in library_checks}),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--pe-inspect", type=Path, default=REPO / "target/release/pe-inspect")
    parser.add_argument("--probe-asset-io", action="store_true", help="compare Win32 reads under Proton with host archive bytes")
    parser.add_argument("--probe-asset-gpu", action="store_true", help="render the pinned Scarlett texture and PS09 shader references under Proton")
    args = parser.parse_args()
    try:
        report = audit_package(args.package.resolve(), args.pe_inspect.resolve(), args.probe_asset_io, args.probe_asset_gpu)
    except (OSError, KeyError, ValueError, ET.ParseError, zlib.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"Package inspection failed: {error}\n")
    print(json.dumps(report, indent=2))
