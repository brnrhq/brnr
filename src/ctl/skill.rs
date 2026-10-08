//! `brnr skill`: the skill for agents that use brnr (ADR 46 in docs/adr),
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

use brnr::paths;

use super::USAGE;

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

pub(super) fn skill(args: &[String]) -> Result<(), String> {
    match args {
        [] => out!("{SKILL}"),
        [cmd, rest @ ..] if cmd == "install" => return install(rest),
        [name] if !name.starts_with('-') => match REFERENCES.iter().find(|(n, _)| n == name) {
            Some((_, text)) => out!("{text}"),
            None => {
                let names: Vec<&str> = REFERENCES.iter().map(|(n, _)| *n).collect();
                return Err(format!("no reference {name} (references: {})", names.join(", ")));
            }
        },
        _ => return Err(USAGE.to_owned()),
    }
    Ok(())
}

fn install(args: &[String]) -> Result<(), String> {
    let mut dirs = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => dirs.push(paths::expand(it.next().ok_or("--dir needs a directory")?)),
            _ => return Err(USAGE.to_owned()),
        }
    }
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
