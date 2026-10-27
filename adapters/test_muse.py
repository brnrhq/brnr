#!/usr/bin/env python3
"""Exercise Muse source builds offline, including replacement of a live binary."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import muse


class SourceBuild(unittest.TestCase):
    def test_builds_offline_and_replaces_a_running_binary_atomically(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            source = root / "source"
            (source / "src").mkdir(parents=True)
            msp = source / "crates/muse-msp-client"
            (msp / "src").mkdir(parents=True)
            version = muse.pinned_version()
            (source / "Cargo.toml").write_text(
                f'[package]\nname="muse-acp"\nversion="{version}"\n'
                'edition="2024"\nrust-version="1.88"\nlicense="Apache-2.0"\n'
                '[dependencies]\nmuse-msp-client={path="crates/muse-msp-client"}\n'
            )
            (msp / "Cargo.toml").write_text(
                '[package]\nname="muse-msp-client"\nversion="0.1.0"\n'
                'edition="2024"\nrust-version="1.88"\nlicense="Apache-2.0"\n[dependencies]\n'
            )
            (msp / "src/lib.rs").write_text('pub fn ready() -> &\'static str { "ready" }\n')
            for name in ("LICENSE", "NOTICE"):
                (source / name).write_text(name)
            code = """use std::io::{self, Write};
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("muse-acp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    println!("{}", muse_msp_client::ready());
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    println!("ALIVE");
}"""
            main = source / "src/main.rs"
            main.write_text(code.replace("ALIVE", "first"))
            out = root / "out"
            # An empty registry and an inherited target directory must not
            # make this build fetch test dependencies or install a stale file.
            with patch.dict(
                os.environ,
                {
                    "CARGO_HOME": str(root / "cargo"),
                    "CARGO_TARGET_DIR": str(root / "wrong-target"),
                },
            ):
                muse.build(out, source)
                binary = out / "brnr-muse-adapter"
                with subprocess.Popen(
                    [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
                ) as running:
                    try:
                        self.assertEqual(running.stdout.readline().strip(), "ready")
                        main.write_text(code.replace("ALIVE", "second"))
                        muse.build(out, source)
                        stdout, _ = running.communicate("\n", timeout=5)
                        self.assertEqual(stdout.strip(), "first")
                        self.assertEqual(running.returncode, 0)
                    finally:
                        if running.poll() is None:
                            running.kill()
                result = subprocess.check_output([binary], input="\n", text=True)
                self.assertEqual(result, "ready\nsecond\n")
                self.assertFalse((root / "wrong-target").exists())
                self.assertEqual((out / "licenses/muse-acp.LICENSE.txt").read_text(), "LICENSE")
                # A changed dependency graph must not silently build another
                # adapter under the same identity.
                with (source / "Cargo.toml").open("a") as manifest:
                    manifest.write('unexpected="1"\n')
                with self.assertRaisesRegex(ValueError, "dependencies changed"):
                    muse.build(out, source)


if __name__ == "__main__":
    unittest.main()
