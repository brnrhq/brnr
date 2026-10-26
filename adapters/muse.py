#!/usr/bin/env python3
"""Build the pinned Muse adapter from source, never its npm binary (ADR 11.1).

The npm devDependency supplies only a version for Dependabot. --source lets
Homebrew supply its checksum-verified source resource instead of fetching the tag.
The production-only Cargo workspace excludes upstream's test/client SDK crates,
so Homebrew can build offline without fetching their unused registry dependencies.
Upstream's Rust sources are unchanged; a new production dependency fails here
until its build and license have been reviewed.
"""

import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
PACKAGE = "@brokkai/muse-acp"


def pinned_version():
    version = json.loads((HERE / "package.json").read_text())["devDependencies"][PACKAGE]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError(f"{PACKAGE}: expected an exact release version, got {version!r}")
    return version


def build(out, source=None):
    version = pinned_version()
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="brnr-muse-build.") as scratch:
        root = Path(scratch)
        if source is None:
            source = root / "upstream"
            subprocess.run(
                [
                    "git",
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    f"v{version}",
                    "https://github.com/BrokkAi/muse-acp.git",
                    str(source),
                ],
                check=True,
            )
        manifest = tomllib.loads((source / "Cargo.toml").read_text())
        if set(manifest) - {"workspace", "package", "dependencies", "dev-dependencies"}:
            raise ValueError("Muse build configuration changed; review the source build")
        if manifest["package"]["version"] != version:
            raise ValueError("Muse source version does not match adapters/package.json")
        if manifest.get("dependencies") != {"muse-msp-client": {"path": "crates/muse-msp-client"}}:
            raise ValueError("Muse production dependencies changed; review the source build")
        msp = source / "crates/muse-msp-client"
        msp_manifest = tomllib.loads((msp / "Cargo.toml").read_text())
        if set(msp_manifest) - {"package", "dependencies", "test"}:
            raise ValueError("Muse MSP build configuration changed; review the source build")
        if msp_manifest.get("dependencies"):
            raise ValueError("Muse MSP client gained dependencies; review the source build")
        for package in (manifest["package"], msp_manifest["package"]):
            if set(package) & {"build", "links", "autobins", "default-run"}:
                raise ValueError("Muse build entry points changed; review the source build")
        work = root / "build"
        shutil.copytree(source / "src", work / "src")
        shutil.copytree(msp / "src", work / "crates/muse-msp-client/src")
        # Preserve the upstream packages' identities and compiler requirements.
        for directory, package in (
            (work, manifest["package"]),
            (
                work / "crates/muse-msp-client",
                msp_manifest["package"],
            ),
        ):
            fields = ("name", "version", "edition", "rust-version", "license")
            text = "[package]\n" + "".join(
                f"{key} = {json.dumps(package[key])}\n" for key in fields
            )
            if directory == work:
                text += (
                    '\n[workspace]\nmembers = ["crates/muse-msp-client"]\n'
                    '\n[dependencies]\nmuse-msp-client = { path = "crates/muse-msp-client" }\n'
                )
            (directory / "Cargo.toml").write_text(text)
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=work, check=True)
        subprocess.run(
            [
                "cargo",
                "build",
                "--release",
                "--frozen",
                "--bin",
                "muse-acp",
                "--target-dir",
                str(work / "target"),
            ],
            cwd=work,
            check=True,
        )
        binary = work / "target/release/muse-acp"
        reported = subprocess.check_output([binary, "--version"], text=True).strip()
        if reported != f"muse-acp {version}":
            raise ValueError(f"unexpected Muse version: {reported!r}")
        # Replacing an executable in place invalidates macOS's cached code
        # signature and changes pages an already-running adapter still maps.
        with tempfile.NamedTemporaryFile(dir=out, prefix=".brnr-muse-", delete=False) as file:
            staged = Path(file.name)
        try:
            shutil.copy2(binary, staged)
            staged.replace(out / "brnr-muse-adapter")
        finally:
            staged.unlink(missing_ok=True)
        licenses = out / "licenses"
        licenses.mkdir(exist_ok=True)
        for name in ("LICENSE", "NOTICE"):
            shutil.copy2(source / name, licenses / f"muse-acp.{name}.txt")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--source", type=Path)
    args = parser.parse_args()
    build(args.out, args.source)
