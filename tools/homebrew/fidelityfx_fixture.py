"""Run the pinned public Scarlett provider in an isolated application directory."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from urllib.request import urlopen
from zipfile import ZipFile

SAMPLES = ("https://raw.githubusercontent.com/microsoft/Xbox-GDK-Samples/"
           "b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Kits/FidelityFX/signedbin/")
PIX = ("https://api.nuget.org/v3-flatcontainer/winpixeventruntime/"
       "1.0.240308001/winpixeventruntime.1.0.240308001.nupkg")
LIBRARIES = {
    "amd_fidelityfx_loader_xbox.dll": "2326637cebc23ce2432b4fed8eac9e03d6b9bad99a413e7caf9df21e187dab42",
    "amd_fidelityfx_upscaler_xbox.dll": "3ffe4c8646ea3ac5f15ef6df49a019311bb5ba0ae9d491e6cc957d2b62584880",
}


def fetch(url, expected):
    with urlopen(url, timeout=45) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError(f"Unexpected SHA-256 for {url}")
    return data


def main(build, context):
    with tempfile.TemporaryDirectory(prefix="fidelityfx-", dir=build) as scratch:
        work = Path(scratch)
        for name in ("fidelityfx_probe.exe", "d3d12_xs.dll", "xmem.dll"):
            shutil.copyfile(build / name, work / name)
        for name, digest in LIBRARIES.items():
            (work / name).write_bytes(fetch(SAMPLES + name, digest))
        package = fetch(PIX, "726acc93d6968e2146261a1e415521747d50ad69894c2b42b5d0d4c29fd66ec4")
        with ZipFile(io.BytesIO(package)) as archive:
            (work / "PIXEvt.dll").write_bytes(archive.read("bin/x64/WinPixEventRuntime.dll"))
        args = " --context" if context else ""
        result = subprocess.run([os.environ["PROTON"], "run", "cmd.exe", "/d", "/c",
                                 f"fidelityfx_probe.exe{args} > result.txt 2>&1"], cwd=work)
        output = work / "result.txt"
        if output.exists():
            print(output.read_text(errors="replace"), end="")
        if context:
            log = work / "fidelityfx-context.log"
            if log.exists():
                shutil.copyfile(log, build / log.name)
                print(f"TuxBox trace: {build / log.name}")
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(Path.cwd(), sys.argv[1] == "--fidelityfx-context"))
