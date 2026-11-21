"""Run the native library checks and retail shader probes using a local Scarlett DLL set."""
import argparse
import csv
import os
from pathlib import Path
from collections import defaultdict
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET

HOMEBREW = Path(__file__).resolve().parent


def main(source, runtime, user_test):
    build = Path.cwd()
    libraries = {p.name.lower(): p for p in source.iterdir() if p.is_file() and p.suffix.lower() == ".dll"}
    with (HOMEBREW.parent / "progress/scarlett-dll-imports.tsv").open() as audit:
        imports = defaultdict(set)
        for row in csv.DictReader(audit, delimiter="\t"):
            imports[row["importer"].lower()].add(row["dependency"].lower())
    expected = set(imports)
    if runtime:
        expected = set()
        pending = set() if user_test else {"libhttpclient.gdk.dll"}
        while pending:
            library = pending.pop()
            expected.add(library)
            pending.update((imports[library] & imports.keys()) - expected)
    if missing := expected - libraries.keys():
        raise SystemExit(f"Missing audited libraries: {', '.join(sorted(missing))}")

    failures = []
    with tempfile.TemporaryDirectory(prefix="scarlett-libs-", dir=build) as scratch:
        work = Path(scratch)
        for name in expected:
            shutil.copyfile(libraries[name], work / libraries[name].name)
        artifacts = ["scarlett_startup_probe.exe"] if runtime else [
            "coin_window.exe", "fidelityfx_probe.exe", "xmem.dll", "d3d12_xs.dll"]
        for name in artifacts:
            shutil.copyfile(build / name, work / name)
        # Proton also supplies several CRTs as builtins. Require the copies under test.
        env = os.environ.copy()
        native = ",".join(sorted(Path(name).stem for name in expected)) + "=n" if expected else ""
        env["WINEDLLOVERRIDES"] = ";".join(filter(None, [env.get("WINEDLLOVERRIDES"), native]))
        if runtime:
            shutil.copyfile(runtime, work / "xgameruntime.dll")
            env["WINEDLLOVERRIDES"] = ";".join(filter(None, [env["WINEDLLOVERRIDES"], "xgameruntime=n"]))
            config = next((p for p in source.iterdir() if p.name.lower() == "microsoftgame.config"), None)
            if config is None:
                raise SystemExit("The package configuration MicrosoftGame.config is missing")
            try:
                title_id = int(ET.parse(config).findtext("TitleId") or "", 16)
                if not 0 <= title_id <= 0xffffffff:
                    raise ValueError("TitleId exceeds 32 bits")
            except (ET.ParseError, ValueError) as error:
                raise SystemExit(f"Invalid TitleId in {config}: {error}") from error
            if user_test:
                commands = [f"scarlett_startup_probe.exe --users {title_id:08x}"]
            else:
                commands = ["scarlett_startup_probe.exe", f"scarlett_startup_probe.exe --game-config {title_id:08x}"]
        else:
            commands = ["coin_window.exe --scarlett-libraries-test", "fidelityfx_probe.exe --verify-provider"]
        for command in commands:
            if runtime and command != "scarlett_startup_probe.exe":
                shutil.copyfile(config, work / "MicrosoftGame.config")
            print(command, flush=True)
            log = work / "native.log"
            log.unlink(missing_ok=True)
            result = subprocess.run([env["PROTON"], "run", "cmd.exe", "/d", "/c",
                                     command + " > native.log 2>&1"], cwd=work, env=env)
            if log.exists():
                print(log.read_text(errors="replace"), end="", flush=True)
            if result.returncode:
                failures.append(command)
    if runtime:
        return int(bool(failures))
    if subprocess.run([HOMEBREW / "play-coins.sh", "--scarlett-import-test"]).returncode:
        failures.append("--scarlett-import-test")
    for mode, name in [("--bvh-sort-test", "xboxbvhbuilder.dll"),
                       ("--fsr-postpass-test", "amd_fidelityfx_upscaler_xbox.dll"),
                       ("--fsr-sharpen-test", "amd_fidelityfx_upscaler_xbox.dll"),
                       ("--fsr-prepass-test", "amd_fidelityfx_upscaler_xbox.dll")]:
        if subprocess.run([HOMEBREW / "play-coins.sh", mode, libraries[name]]).returncode:
            failures.append(mode)
    if failures:
        raise SystemExit("Failed: " + ", ".join(failures))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--http-runtime", type=Path)
    modes.add_argument("--user-runtime", type=Path)
    args = parser.parse_args()
    raise SystemExit(main(args.source.resolve(), args.http_runtime or args.user_runtime, bool(args.user_runtime)))
