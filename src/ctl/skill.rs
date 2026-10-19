//! `brnr skill`: the skill for agents that use brnr (ADR 8.6 in docs/adr),
//! embedded at build time from `skills/brnr/`, so it is always the one for
//! this brnr.
//!
//! - `brnr skill` prints `SKILL.md`; `brnr skill <reference>` one of its
//!   references.
//! - `brnr skill install [--dir <dir>]...` writes it to `<dir>/brnr/`, by
//!   default in `~/.claude/skills` (Claude Code) and `~/.agents/skills`
//!   (Codex).

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

use super::Args;
use super::args::Error;

const SKILL: &str = include_str!("../../skills/brnr/SKILL.md");

/// The references, by the name `brnr skill <name>` takes.
const REFERENCES: &[(&str, &str)] = &[
    ("orchestrate", include_str!("../../skills/brnr/references/orchestrate.md")),
    ("approvals", include_str!("../../skills/brnr/references/approvals.md")),
    ("observe", include_str!("../../skills/brnr/references/observe.md")),
    ("setup", include_str!("../../skills/brnr/references/setup.md")),
];

/// Where `install` writes without `--dir`: the skills directories of
/// Claude Code and Codex.
const DIRS: &[&str] = &["~/.claude/skills", "~/.agents/skills"];

/// `brnr skill [<reference>]`; a reference is never `-h` or another
/// single-dash word, which is a usage error.
pub(super) fn skill(a: &Args) -> Result<(), Error> {
    match a.positional.first() {
        None => out!("{SKILL}"),
        Some(name) if name.starts_with('-') => return Err(Error::Usage),
        Some(name) => match REFERENCES.iter().find(|(n, _)| n == name) {
            Some((_, text)) => out!("{text}"),
            None => {
                let names: Vec<&str> = REFERENCES.iter().map(|(n, _)| *n).collect();
                let names = names.join(", ");
                return Err(format!("no reference {name} (references: {names})").into());
            }
        },
    }
    Ok(())
}

/// `brnr skill install [--dir <dir>]...`.
pub(super) fn install(a: &Args) -> Result<(), String> {
    let mut dirs: Vec<_> = a.values("--dir").iter().map(|d| paths::expand(d)).collect();
    if dirs.is_empty() {
        dirs = DIRS.iter().map(|d| paths::expand(d)).collect();
    }
    for dir in dirs {
        let skill = dir.join("brnr");
        write(&skill).map_err(|e| format!("{}: {e}", skill.display()))?;
        outln!("installed {}", skill.display());
    }
    Ok(())
}

/// Writes the skill into `skill`, replacing what an earlier install wrote
/// there, references it no longer has included.
fn write(skill: &Path) -> std::io::Result<()> {
    let references = skill.join("references");
    fs::create_dir_all(&references)?;
    fs::write(skill.join("SKILL.md"), SKILL)?;
    for entry in fs::read_dir(&references)? {
        let path = entry?.path();
        let ours = |p: &PathBuf| REFERENCES.iter().any(|(n, _)| p.file_stem() == Some(n.as_ref()));
        if path.extension().is_some_and(|x| x == "md") && !ours(&path) {
            fs::remove_file(&path)?;
        }
    }
    for (name, text) in REFERENCES {
        fs::write(references.join(format!("{name}.md")), text)?;
    }
    Ok(())
}
