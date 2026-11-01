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

# Exact production graphs accepted after ADR 11.1 review. Anything else fails.
# Legacy (≤0.16): path MSP client only. Reviewed (0.17+): MSP + acp-lite + serde_json.
DEPS_LEGACY = {"muse-msp-client": {"path": "crates/muse-msp-client"}}
DEPS_REVIEWED = {
    "muse-msp-client": {"path": "crates/muse-msp-client"},
    "acp-lite": {"path": "crates/acp-lite"},
    "serde_json": {
        "version": "1",
        "features": ["arbitrary_precision", "preserve_order", "raw_value"],
    },
}
ACP_LITE_DEPS = {
    "serde": {"version": "1", "features": ["derive"]},
    "serde_json": {"version": "1", "features": ["raw_value"]},
}


def pinned_version():
    version = json.loads((HERE / "package.json").read_text())["devDependencies"][PACKAGE]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError(f"{PACKAGE}: expected an exact release version, got {version!r}")
    return version


def _package_toml(package):
    fields = ("name", "version", "edition", "rust-version", "license")
    return "[package]\n" + "".join(f"{key} = {json.dumps(package[key])}\n" for key in fields)


def _write_dep(name, spec):
    if "path" in spec:
        return f"{name} = {{ path = {json.dumps(spec['path'])} }}\n"
    features = spec.get("features") or []
    return (
        f"{name} = {{ version = {json.dumps(spec['version'])}, "
        f"features = {json.dumps(features)} }}\n"
    )


def _stage_licenses(source, work, out, reviewed):
    licenses = out / "licenses"
    licenses.mkdir(exist_ok=True)
    for name in ("LICENSE", "NOTICE"):
        shutil.copy2(source / name, licenses / f"muse-acp.{name}.txt")
    if not reviewed:
        return
    # Registry crates' license files from the vendored tree (ADR 11.1).
    vendor = work / "vendor"
    if not vendor.is_dir():
        return
    for crate in sorted(vendor.iterdir()):
        if not crate.is_dir():
            continue
        for pattern in ("LICENSE*", "COPYING*", "NOTICE*"):
            for path in sorted(crate.glob(pattern)):
                if path.is_file():
                    shutil.copy2(path, licenses / f"{crate.name}.{path.name}")


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
        deps = manifest.get("dependencies")
        if deps == DEPS_LEGACY:
            reviewed = False
        elif deps == DEPS_REVIEWED:
            reviewed = True
        else:
            raise ValueError("Muse production dependencies changed; review the source build")
        msp = source / "crates/muse-msp-client"
        msp_manifest = tomllib.loads((msp / "Cargo.toml").read_text())
        if set(msp_manifest) - {"package", "dependencies", "test"}:
            raise ValueError("Muse MSP build configuration changed; review the source build")
        if msp_manifest.get("dependencies"):
            raise ValueError("Muse MSP client gained dependencies; review the source build")
        packages = [manifest["package"], msp_manifest["package"]]
        if reviewed:
            acp = source / "crates/acp-lite"
            acp_manifest = tomllib.loads((acp / "Cargo.toml").read_text())
            if set(acp_manifest) - {"package", "dependencies", "dev-dependencies"}:
                raise ValueError(
                    "Muse acp-lite build configuration changed; review the source build"
                )
            if acp_manifest.get("dependencies") != ACP_LITE_DEPS:
                raise ValueError("Muse acp-lite dependencies changed; review the source build")
            packages.append(acp_manifest["package"])
        for package in packages:
            if set(package) & {"build", "links", "autobins", "default-run"}:
                raise ValueError("Muse build entry points changed; review the source build")
        work = root / "build"
        shutil.copytree(source / "src", work / "src")
        shutil.copytree(msp / "src", work / "crates/muse-msp-client/src")
        (work / "crates/muse-msp-client/Cargo.toml").write_text(
            _package_toml(msp_manifest["package"])
        )
        members = ['"crates/muse-msp-client"']
        root_deps = DEPS_LEGACY
        if reviewed:
            shutil.copytree(acp / "src", work / "crates/acp-lite/src")
            (work / "crates/acp-lite/Cargo.toml").write_text(
                _package_toml(acp_manifest["package"])
                + "\n[dependencies]\n"
                + "".join(_write_dep(n, s) for n, s in ACP_LITE_DEPS.items())
            )
            members.append('"crates/acp-lite"')
            root_deps = DEPS_REVIEWED
        # Preserve the upstream packages' identities and compiler requirements.
        text = _package_toml(manifest["package"])
        text += f"\n[workspace]\nmembers = [{', '.join(members)}]\n"
        text += "\n[dependencies]\n" + "".join(_write_dep(n, s) for n, s in root_deps.items())
        (work / "Cargo.toml").write_text(text)
        if reviewed:
            # Pin the reviewed registry graph from upstream's lock, vendor it,
            # then build offline so an empty cache still cannot pull extras.
            lock = source / "Cargo.lock"
            if not lock.is_file():
                raise ValueError("Muse source is missing Cargo.lock; review the source build")
            shutil.copy2(lock, work / "Cargo.lock")
            subprocess.run(["cargo", "vendor", "--locked", "vendor"], cwd=work, check=True)
            (work / ".cargo").mkdir()
            (work / ".cargo/config.toml").write_text(
                '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
                '[source.vendored-sources]\ndirectory = "vendor"\n'
            )
        else:
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=work, check=True)
        build_cmd = [
            "cargo",
            "build",
            "--release",
            "--frozen",
            "--bin",
            "muse-acp",
            "--target-dir",
            str(work / "target"),
        ]
        if reviewed:
            # Vendored sources: refuse any further registry contact.
            build_cmd.insert(4, "--offline")
        subprocess.run(build_cmd, cwd=work, check=True)
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
        _stage_licenses(source, work, out, reviewed)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--source", type=Path)
    args = parser.parse_args()
    build(args.out, args.source)
