//! A release's notes are its section of CHANGELOG.md (Keep a Changelog
//! 1.1.0), which `release.sh changelog` prints for the release workflow,
//! `release.sh tag` checks for and `release.sh <bump>` names, and a release
//! takes in its release candidates' (ADR 40).

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::{Env, install, stderr};

/// `release.sh changelog <args>`, run from `dir`'s copy of it, which reads
/// `dir`'s CHANGELOG.md.
fn changelog(dir: &Path, args: &[&str]) -> Output {
    release(dir, "changelog", args)
}

/// `release.sh <command> <args>`, run from `dir`'s copy of it.
fn release(dir: &Path, command: &str, args: &[&str]) -> Output {
    Command::new(dir.join("release.sh")).arg(command).args(args).output().unwrap()
}

/// `dir`'s Cargo.toml, at `version`.
fn cargo_toml(dir: &Path, version: &str) {
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"brnr\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
}

/// `git <args>` in `dir`, as nobody in particular: the user's config, which
/// may sign commits, is left out.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
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

#[test]
fn adr_0040_a_version_comes_after_cargo_tomls() {
    let env = scratch("next", CHANGELOG);
    let next = |current: &str, wanted: &str| {
        cargo_toml(&env.dir, current);
        let out = release(&env.dir, "next", &[wanted]);
        match out.status.success() {
            true => Ok(String::from_utf8(out.stdout).unwrap().trim_end().to_owned()),
            false => Err(stderr(&out)),
        }
    };
    assert_eq!(next("0.8.0", "patch").unwrap(), "0.8.1");
    assert_eq!(next("0.8.0", "minor").unwrap(), "0.9.0");
    assert_eq!(next("0.8.0", "major").unwrap(), "1.0.0");
    // 0.8.0 to 1.0.0-rc.1 to rc.2 to 1.0.0.
    assert_eq!(next("0.8.0", "1.0.0-rc.1").unwrap(), "1.0.0-rc.1");
    assert_eq!(next("1.0.0-rc.1", "rc").unwrap(), "1.0.0-rc.2");
    assert_eq!(next("1.0.0-rc.9", "rc").unwrap(), "1.0.0-rc.10");
    assert_eq!(next("1.0.0-rc.9", "1.0.0-rc.10").unwrap(), "1.0.0-rc.10");
    assert_eq!(next("1.0.0-rc.2", "1.0.0").unwrap(), "1.0.0");
    // From a release candidate, major, minor and patch would guess: the
    // release, or past it?
    for wanted in ["major", "minor", "patch"] {
        let err = next("1.0.0-rc.1", wanted).unwrap_err();
        assert!(
            err.contains(
                "1.0.0-rc.1 is a release candidate: rc for 1.0.0-rc.2, or 1.0.0 to release it"
            ),
            "{err}"
        );
    }
    let err = next("0.8.0", "rc").unwrap_err();
    assert!(err.contains("0.8.0 isn't a release candidate"), "{err}");
    // Only X.Y.Z and X.Y.Z-rc.N, as SemVer writes them.
    for wanted in [
        "1.2.3.4",
        "1.0.0rc1",
        "1.0.0-rc1",
        "1.0.0-beta.1",
        "1.0.0-rc.0",
        "01.0.0",
        "1.0",
        "v1.0.0",
    ] {
        let err = next("0.8.0", wanted).unwrap_err();
        assert!(err.contains(&format!("not {wanted}")), "{wanted}: {err}");
    }
    // And only forwards: a release comes after its release candidates.
    for (current, wanted) in [
        ("0.8.0", "0.8.0"),
        ("0.8.0", "0.7.9"),
        ("0.8.0", "0.8.0-rc.1"),
        ("1.0.0-rc.2", "1.0.0-rc.1"),
        ("1.0.0", "1.0.0-rc.3"),
    ] {
        let err = next(current, wanted).unwrap_err();
        assert!(err.contains(&format!("{wanted} doesn't come after {current}")), "{wanted}: {err}");
    }
}

#[test]
fn adr_0040_a_release_takes_in_its_release_candidates() {
    let text = "# Changelog

## [Unreleased]

### Added

- Added in rc.1.

### Changed

- Changed in rc.1, wrapped
  over two lines.
- **Breaking:** broken in rc.1.

## [0.8.0] - 2026-01-01

### Fixed

- The first.

[Unreleased]: https://github.com/brnrhq/brnr/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/brnrhq/brnr/releases/tag/v0.8.0
";
    let env = scratch("candidates", text);
    let name = |version: &str, date: &str, unreleased: &str| {
        let out = changelog(&env.dir, &["--named", version, date]);
        assert!(out.status.success(), "{version}: {}", stderr(&out));
        let named = String::from_utf8(out.stdout).unwrap();
        let next =
            named.replacen("## [Unreleased]\n", &format!("## [Unreleased]\n{unreleased}"), 1);
        fs::write(env.dir.join("CHANGELOG.md"), next).unwrap();
    };
    name(
        "1.0.0-rc.1",
        "2026-02-01",
        "\n### Changed\n\n- Changed in rc.2.\n- **Breaking:** broken in rc.2.\n\n### Fixed\n\n- Fixed in rc.2.\n",
    );
    name("1.0.0-rc.2", "2026-02-02", "");
    // A release candidate has its own section and link, as a release does.
    let rc2 = String::from_utf8(changelog(&env.dir, &["1.0.0-rc.2"]).stdout).unwrap();
    assert_eq!(
        rc2,
        "### Changed\n\n- Changed in rc.2.\n- **Breaking:** broken in rc.2.\n\n### Fixed\n\n- Fixed in rc.2.\n"
    );
    let file = fs::read_to_string(env.dir.join("CHANGELOG.md")).unwrap();
    assert!(
        file.contains(&format!("[1.0.0-rc.2]: {COMPARE}/v1.0.0-rc.1...v1.0.0-rc.2\n")),
        "{file}"
    );

    // 1.0.0, with nothing under Unreleased, has what both had: by change
    // type, the breaking changes first, newest first; and compares 0.8.0.
    let out = changelog(&env.dir, &["--named", "1.0.0", "2026-03-01"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let expected = format!(
        "# Changelog

## [Unreleased]

## [1.0.0] - 2026-03-01

### Added

- Added in rc.1.

### Changed

- **Breaking:** broken in rc.2.
- **Breaking:** broken in rc.1.
- Changed in rc.2.
- Changed in rc.1, wrapped
  over two lines.

### Fixed

- Fixed in rc.2.

## [0.8.0] - 2026-01-01

### Fixed

- The first.

[Unreleased]: {COMPARE}/v1.0.0...HEAD
[1.0.0]: {COMPARE}/v0.8.0...v1.0.0
[0.8.0]: https://github.com/brnrhq/brnr/releases/tag/v0.8.0
"
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);

    // A later release takes in only its own release candidates.
    fs::write(env.dir.join("CHANGELOG.md"), &expected).unwrap();
    let out = changelog(&env.dir, &["--named", "1.0.1", "2026-03-02"]);
    let named = String::from_utf8(out.stdout).unwrap();
    assert!(named.contains("## [1.0.0] - 2026-03-01\n"), "{named}");
    assert!(named.contains(&format!("[1.0.1]: {COMPARE}/v1.0.0...v1.0.1\n")), "{named}");
}

#[test]
fn adr_0040_a_releases_commits_start_from_the_last_release() {
    let env = scratch("commits", "");
    let dir = &env.dir;
    fs::create_dir(dir.join("adapters")).unwrap();
    fs::write(dir.join("adapters/package.json"), r#"{"dependencies": {}}"#).unwrap();
    let section = |version: &str, from: &str| {
        format!(
            "## [Unreleased]\n\n## [{version}] - 2026-01-01\n\n### Added\n\n- In {version}.\n\n[Unreleased]: {COMPARE}/v{version}...HEAD\n[{version}]: {COMPARE}/{from}...v{version}\n"
        )
    };
    git(dir, &["init", "-q", "-b", "main", "."]);
    for (version, from, commit) in
        [("0.8.0", "v0.7.0", "Before"), ("1.0.0-rc.1", "v0.8.0", "For rc.1")]
    {
        git(dir, &["commit", "-q", "--allow-empty", "-m", commit]);
        cargo_toml(dir, version);
        fs::write(dir.join("CHANGELOG.md"), section(version, from)).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Release {version}")]);
        git(dir, &["tag", &format!("v{version}")]);
    }
    git(dir, &["commit", "-q", "--allow-empty", "-m", "After rc.1"]);

    let notes = |wanted: &str| {
        let out = release(dir, "notes", &[wanted]);
        assert!(out.status.success(), "{wanted}: {}", stderr(&out));
        String::from_utf8(out.stdout).unwrap()
    };
    // rc.2's are since rc.1, and leave the formulae alone.
    let rc2 = notes("rc");
    assert!(rc2.contains("## Commits since v1.0.0-rc.1\n- After rc.1\n\n"), "{rc2}");
    assert!(rc2.contains("leaves the Homebrew formulae alone"), "{rc2}");
    // 1.0.0's are since 0.8.0, and its notes have rc.1's.
    let release = notes("1.0.0");
    assert!(release.contains("## Commits since v0.8.0\n- After rc.1\n- For rc.1\n\n"), "{release}");
    assert!(release.contains("### Added\n\n- In 1.0.0-rc.1.\n"), "{release}");
    assert!(release.contains("## Adapters (Homebrew formula versions)"), "{release}");

    // An old Git must fail before describe's unsupported --exclude can
    // look like a repository with no previous release.
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    install(&dir.join("release.sh"), &bin.join("git"));
    fs::write(
        bin.join("git"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then\n  printf '%s\\n' \"$TEST_GIT_VERSION\"\n  exit 0\nfi\nPATH=$TEST_GIT_PATH exec git \"$@\"\n",
    )
    .unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&path));
    let wrapped_path = std::env::join_paths(paths).unwrap();
    for (version, supported) in [
        ("git version 1.9.5", false),
        ("git version 2.12.9", false),
        ("git version 2.13.0", true),
        ("git version 2.39.5 (Apple Git-154)", true),
        ("git version 3.0.0", true),
    ] {
        let out = Command::new(dir.join("release.sh"))
            .args(["notes", "1.0.0"])
            .env("PATH", &wrapped_path)
            .env("TEST_GIT_PATH", &path)
            .env("TEST_GIT_VERSION", version)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), supported, "{version}: {}", stderr(&out));
        if supported {
            assert_eq!(String::from_utf8(out.stdout).unwrap(), release, "{version}");
        } else {
            assert!(out.stdout.is_empty(), "{version}: {out:?}");
            assert!(stderr(&out).contains("needs Git 2.13 or newer"), "{}", stderr(&out));
        }
    }
}
