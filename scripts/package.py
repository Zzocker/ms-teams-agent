#!/usr/bin/env python3
"""Package an already-built executable using only the Python standard library."""
import argparse
import plistlib
import shutil
import struct
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def package(platform, arch, binary, output):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    name = f"ms-teams-agent-{version}-{platform}-{arch}"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        folder = Path(temporary) / name
        folder.mkdir()
        shutil.copy2(ROOT / "README.md", folder / "README.md")
        shutil.copytree(ROOT / "docs", folder / "docs")
        if platform == "macos":
            contents = folder / "Teams.app" / "Contents"
            (contents / "MacOS").mkdir(parents=True)
            (contents / "Resources").mkdir()
            executable = contents / "MacOS" / "ms-teams-agent"
            shutil.copy2(binary, executable)
            executable.chmod(0o755)
            # ICNS supports a PNG payload; ic09 is the existing 512px icon.
            png = (ROOT / "assets/icons/icon.png").read_bytes()
            (contents / "Resources" / "icon.icns").write_bytes(
                b"icns" + struct.pack(">I", len(png) + 16)
                + b"ic09" + struct.pack(">I", len(png) + 8) + png)
            with (contents / "Info.plist").open("wb") as stream:
                plistlib.dump({
                    "CFBundleName": "Teams", "CFBundleDisplayName": "Teams",
                    "CFBundleIdentifier": "com.github.zzocker.ms-teams-agent",
                    "CFBundleExecutable": "ms-teams-agent", "CFBundlePackageType": "APPL",
                    "CFBundleShortVersionString": version, "CFBundleVersion": version,
                    "CFBundleIconFile": "icon.icns", "NSHighResolutionCapable": True,
                    "NSMicrophoneUsageDescription": "Teams Agent uses your microphone during audio calls.",
                }, stream)
        else:
            executable = folder / ("ms-teams-agent.exe" if platform == "windows" else "ms-teams-agent")
            shutil.copy2(binary, executable)
            executable.chmod(0o755)
            if platform in ("ubuntu", "arch"):
                shutil.copytree(ROOT / "assets", folder / "assets")
                shutil.copy2(ROOT / "scripts/install-linux.sh", folder / "install.sh")
                (folder / "install.sh").chmod(0o755)
        if platform == "windows":
            archive = output / f"{name}.zip"
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as stream:
                for path in sorted(folder.rglob("*")):
                    stream.write(path, path.relative_to(folder.parent))
        else:
            archive = output / f"{name}.tar.gz"
            with tarfile.open(archive, "w:gz") as stream:
                def permissions(info):
                    if info.isdir() or info.name.endswith(('/ms-teams-agent', '/install.sh')):
                        info.mode = 0o755
                    else:
                        info.mode = 0o644
                    return info
                stream.add(folder, arcname=name, filter=permissions)
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["ubuntu", "arch", "windows", "macos"])
    parser.add_argument("arch", choices=["x86_64", "aarch64"])
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    print(package(args.platform, args.arch, args.binary, args.output))
