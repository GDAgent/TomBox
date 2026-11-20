"""Run the native library checks and retail shader probes using a local Scarlett DLL set."""
import csv
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HOMEBREW = Path(__file__).resolve().parent


def main(source):
    build = Path.cwd()
    libraries = {p.name.lower(): p for p in source.iterdir() if p.is_file() and p.suffix.lower() == ".dll"}
    with (HOMEBREW.parent / "progress/scarlett-dll-imports.tsv").open() as audit:
        expected = {row["importer"].lower() for row in csv.DictReader(audit, delimiter="\t")}
    if missing := expected - libraries.keys():
        raise SystemExit(f"Missing audited libraries: {', '.join(sorted(missing))}")

    failures = []
    with tempfile.TemporaryDirectory(prefix="scarlett-libs-", dir=build) as scratch:
        work = Path(scratch)
        for name in expected:
            shutil.copyfile(libraries[name], work / libraries[name].name)
        for name in ("coin_window.exe", "fidelityfx_probe.exe", "xmem.dll", "d3d12_xs.dll"):
            shutil.copyfile(build / name, work / name)
        # Proton also supplies several CRTs as builtins. Require the copies under test.
        env = os.environ.copy()
        native = ",".join(sorted(Path(name).stem for name in expected)) + "=n"
        env["WINEDLLOVERRIDES"] = ";".join(filter(None, [env.get("WINEDLLOVERRIDES"), native]))
        for command in ("coin_window.exe --scarlett-libraries-test", "fidelityfx_probe.exe --verify-provider"):
            log = work / "native.log"
            log.unlink(missing_ok=True)
            result = subprocess.run([env["PROTON"], "run", "cmd.exe", "/d", "/c",
                                     command + " > native.log 2>&1"], cwd=work, env=env)
            if log.exists():
                print(log.read_text(errors="replace"), end="", flush=True)
            if result.returncode:
                failures.append(command)
    for mode, name in [("--bvh-sort-test", "xboxbvhbuilder.dll"),
                       ("--fsr-postpass-test", "amd_fidelityfx_upscaler_xbox.dll"),
                       ("--fsr-sharpen-test", "amd_fidelityfx_upscaler_xbox.dll"),
                       ("--fsr-prepass-test", "amd_fidelityfx_upscaler_xbox.dll")]:
        if subprocess.run([HOMEBREW / "play-coins.sh", mode, libraries[name]]).returncode:
            failures.append(mode)
    if failures:
        raise SystemExit("Failed: " + ", ".join(failures))


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
