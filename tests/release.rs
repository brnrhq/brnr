//! A release's notes are its section of CHANGELOG.md (Keep a Changelog
//! 1.1.0), which `release.sh changelog` prints for the release workflow,
//! `release.sh tag` checks for and `release.sh <bump>` names (ADR 40).

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

const COMPARE: &str = "https://github.com/brnrhq/brnr/compare";

const CHANGELOG: &str = "# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- Something new.

## [1.1.0] - 2026-01-02

### Added

- A line that is
  wrapped, with `code`.
  - A nested one,
    wrapped too.

Then a paragraph,
also wrapped.

## [1.1.00] - 2026-01-01

- Not 1.1.0.

## [1.0.0] - 2026-01-01

### Fixed

- The first.

[Unreleased]: https://github.com/brnrhq/brnr/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/brnrhq/brnr/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/brnrhq/brnr/releases/tag/v1.0.0
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
    // The last section stops at the link definitions.
    let out = changelog(&env.dir, &["1.0.0"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "### Fixed\n\n- The first.\n");
}

#[test]
fn adr_0040_a_version_without_a_section_has_no_notes() {
    let empty = CHANGELOG.replace("### Added\n\n- Something new.\n\n## [1.1.0]", "## [1.1.0]");
    let env = scratch("no-notes", &empty);
    // 1.1 isn't 1.1.0, nor 1.1.00 a prefix of it, and an empty section is none.
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

#[test]
fn adr_0040_a_release_names_unreleased_and_moves_its_links() {
    let env = scratch("named", CHANGELOG);
    let out = changelog(&env.dir, &["--named", "1.2.0", "2026-02-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let expected = CHANGELOG
        .replace("## [Unreleased]\n", "## [Unreleased]\n\n## [1.2.0] - 2026-02-03\n")
        .replace(
            &format!("[Unreleased]: {COMPARE}/v1.1.0...HEAD\n"),
            &format!("[Unreleased]: {COMPARE}/v1.2.0...HEAD\n[1.2.0]: {COMPARE}/v1.1.0...v1.2.0\n"),
        );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
    // It only prints; <bump> writes it on the release branch.
    assert_eq!(fs::read_to_string(env.dir.join("CHANGELOG.md")).unwrap(), CHANGELOG);

    // Without the Unreleased link there's nothing to compare from.
    let env = scratch(
        "unnamed",
        &CHANGELOG.replace(&format!("[Unreleased]: {COMPARE}/v1.1.0...HEAD\n"), ""),
    );
    let out = changelog(&env.dir, &["--named", "1.2.0", "2026-02-03"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("CHANGELOG.md needs '## [Unreleased]'"), "{}", stderr(&out));
}

/// The version Cargo.toml has, on main or a release branch, has its notes,
/// so a release pull request without them fails before it is tagged.
#[test]
fn adr_0040_the_version_being_built_has_its_notes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = changelog(root, &[env!("CARGO_PKG_VERSION")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = String::from_utf8(out.stdout).unwrap();
    assert!(!notes.trim().is_empty());
    assert!(!notes.lines().any(|l| l.starts_with("## ") || l.starts_with('[')), "{notes}");
    // Keep a Changelog: the change types it has, in its order.
    let types = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"];
    let text = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    for section in text.split("\n## ").skip(1) {
        let found: Vec<usize> = section
            .lines()
            .filter_map(|l| l.strip_prefix("### "))
            .map(|t| types.iter().position(|k| *k == t).unwrap_or_else(|| panic!("### {t}")))
            .collect();
        assert!(
            found.windows(2).all(|w| w[0] < w[1]),
            "out of order: ## {}",
            section.lines().next().unwrap()
        );
    }
}
