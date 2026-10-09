//! A release's notes are its section of CHANGELOG.md, which
//! `release.sh changelog` prints for the release workflow and `release.sh
//! tag` checks for (ADR 40).

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::{Env, install, stderr};

/// `release.sh changelog <args>`, run from `dir`'s copy of it, which reads
/// `dir`'s CHANGELOG.md.
fn changelog(dir: &Path, args: &[&str]) -> Output {
    Command::new(dir.join("release.sh")).arg("changelog").args(args).output().unwrap()
}

/// A scratch directory with release.sh and this CHANGELOG.md.
fn scratch(name: &str, text: &str) -> Env {
    let env = Env::new(name);
    install(&Path::new(env!("CARGO_MANIFEST_DIR")).join("release.sh"), &env.dir.join("release.sh"));
    fs::write(env.dir.join("CHANGELOG.md"), text).unwrap();
    env
}

const CHANGELOG: &str = "# Changelog

What changed.

## Unreleased

## 1.1.0 - 2026-01-02

### Added

- A line that is
  wrapped, with `code`.
  - A nested one,
    wrapped too.

Then a paragraph,
also wrapped.

## 1.1.00 - 2026-01-01

- Not 1.1.0.

## 1.0.0 - 2026-01-01

The first.
";

#[test]
fn adr_0040_a_releases_notes_are_its_changelog_section() {
    let env = scratch("notes", CHANGELOG);
    let out = changelog(&env.dir, &["1.1.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // Up to the next section, without the heading, a line per paragraph or
    // item: GitHub shows a line break where the text has one.
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "### Added\n\n- A line that is wrapped, with `code`.\n  - A nested one, wrapped too.\n\nThen a paragraph, also wrapped.\n"
    );
    let out = changelog(&env.dir, &["1.0.0"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "The first.\n");
}

#[test]
fn adr_0040_a_version_without_a_section_has_no_notes() {
    let env = scratch("no-notes", CHANGELOG);
    // 1.1 isn't 1.1.0, and an empty section is none.
    for version in ["1.2.0", "1.1", "Unreleased"] {
        let out = changelog(&env.dir, &[version]);
        assert_eq!(out.status.code(), Some(1), "{version}");
        assert!(out.stdout.is_empty(), "{version}");
        assert!(
            stderr(&out).contains(&format!("CHANGELOG.md has no section for {version}")),
            "{version}: {}",
            stderr(&out)
        );
    }
}

/// The version Cargo.toml has, on main or a release branch, has its notes,
/// so a release pull request without them fails before it is tagged.
#[test]
fn adr_0040_the_version_being_built_has_its_notes() {
    let out = changelog(Path::new(env!("CARGO_MANIFEST_DIR")), &[env!("CARGO_PKG_VERSION")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = String::from_utf8(out.stdout).unwrap();
    assert!(!notes.trim().is_empty());
    assert!(!notes.lines().any(|l| l.starts_with("## ")), "{notes}");
}
