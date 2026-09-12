#!/usr/bin/env python3
"""Extract checksum-pinned Debian development files for Linux cargo check.

Requires Python with tarfile's data extraction filter and the ar utility. This
creates a sysroot in the requested directory; it never executes package scripts
or installs software into the host OS. It does not provide a Linux runtime.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("output", type=Path)
parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parents[1] / "docs/upgrade-evidence/linux-sysroot-packages.json")
args = parser.parse_args()
manifest = json.loads(args.manifest.read_text())
args.output.mkdir(parents=True, exist_ok=True)
root = args.output / "root"
if root.is_symlink():
    raise SystemExit("Refusing a symlinked extraction root")
root.mkdir(exist_ok=True)
for package in manifest["packages"]:
    filename = package["filename"]
    if not filename.startswith("pool/") or ".." in Path(filename).parts:
        raise SystemExit("Invalid package path in manifest")
    archive_path = args.output / (package["package"] + ".deb")
    if not archive_path.exists():
        with urllib.request.urlopen("https://deb.debian.org/debian/" + filename, timeout=60) as response:
            data = response.read(64 * 1024 * 1024 + 1)
        if len(data) > 64 * 1024 * 1024:
            raise SystemExit("Package download budget exceeded")
        archive_path.write_bytes(data)
    if hashlib.sha256(archive_path.read_bytes()).hexdigest() != package["sha256"]:
        raise SystemExit("Package checksum mismatch: " + package["package"])
    members = subprocess.check_output(["ar", "t", str(archive_path)], text=True).splitlines()
    member = next(item for item in members if item.startswith("data.tar."))
    data = subprocess.check_output(["ar", "p", str(archive_path), member])
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
        safe = []
        for item in archive.getmembers():
            path = Path(item.name)
            if path.is_absolute() or ".." in path.parts:
                raise SystemExit("Unsafe package member")
            # Absolute target-library symlinks are unnecessary for metadata/type
            # checking and must never point outside this isolated extraction.
            if item.issym() and Path(item.linkname).is_absolute():
                continue
            safe.append(item)
        archive.extractall(root, members=safe, filter="data")
    print(package["package"], package["version"], flush=True)
print("Prepared real headers and pkg-config metadata at", root)
