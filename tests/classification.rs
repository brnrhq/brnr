//! Every command, entry point and transport form brnr's parser has is
//! classified in the agents guide, and nothing the guide classifies is
//! gone: the `<!-- classification: … -->` in docs/guide/agents.md. This
//! checks that each has been considered, not that the guide is a complete
//! policy (the OS sandbox is what holds, as the guide says).
//!
//! The list is read from the parser itself: the verb table and the other
//! `Spec`s and transport flags in src/ctl/args.rs, and the words `brnr::run`
//! (src/lib.rs) dispatches on before them. `brnr --help` is read too, so a
//! command it lists that the reading of the source missed fails.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;

const ARGS: &str = include_str!("../src/ctl/args.rs");
const LIB: &str = include_str!("../src/lib.rs");
const GUIDE: &str = include_str!("../docs/guide/agents.md");

/// The guide's classes, as its command-rules paragraphs have them.
const CLASSES: &[&str] = &["deny", "writes", "disrupts", "allow", "reads"];

/// The string literals in `line`.
fn literals(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

/// `cmd verb` or `cmd`, from a `verb("cmd", "verb", …)` or `command("cmd",
/// …)` call starting `line`; `None` for anything else.
fn spec_of(line: &str) -> Option<String> {
    let words = literals(line);
    if line.starts_with("verb(\"") {
        Some(format!("{} {}", words[0], words[1]))
    } else if line.starts_with("command(\"") {
        Some(words[0].to_owned())
    } else {
        None
    }
}

/// What the parser has: each verb of `VERBS`, each other `Spec` (`acp`,
/// `--version`), the transport flags, and the entry points `run` takes
/// before the parser (`_remote`, `acp`, `host`).
fn parsed() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let start = "pub(super) const VERBS: &[Spec] = &[\n";
    let table = &ARGS[ARGS.find(start).expect("VERBS in src/ctl/args.rs") + start.len()..];
    let table = &table[..table.find("\n];\n").expect("the end of VERBS")];
    // An entry starts at the table's indent; what is deeper continues it,
    // and `])` there closes one.
    for line in table.lines().filter(|l| l.starts_with("    ") && !l.starts_with("     ")) {
        let line = line.trim_start();
        if line.starts_with("//") || line.starts_with([')', ']']) {
            continue;
        }
        let spec = spec_of(line);
        found.insert(spec.unwrap_or_else(|| panic!("a VERBS entry this test can't read: {line}")));
    }
    for line in ARGS.lines().filter(|l| l.contains(": Spec = ")) {
        let call = line.split(": Spec = ").nth(1).unwrap();
        found
            .insert(spec_of(call).unwrap_or_else(|| panic!("a Spec this test can't read: {line}")));
    }
    let transport = ARGS.lines().find(|l| l.starts_with("const TRANSPORT: &[Flag]"));
    let transport = transport.expect("TRANSPORT in src/ctl/args.rs").split("Flag(\"").skip(1);
    found.extend(transport.map(|s| s[..s.find('"').unwrap()].to_owned()));
    // `brnr::run`'s dispatch, before the parser: `arg == "_remote"`,
    // `cmd == "acp"`, `cmd == "host"`.
    let run = &LIB[LIB.find("pub fn run(").expect("brnr::run in src/lib.rs")..];
    let run = &run[..run.find("\n}\n").expect("the end of brnr::run")];
    let dispatched: Vec<&str> =
        run.split("== \"").skip(1).map(|s| &s[..s.find('"').unwrap()]).collect();
    assert!(!dispatched.is_empty(), "no entry points read from brnr::run");
    found.extend(dispatched.into_iter().map(str::to_owned));
    found
}

/// The guide's classification: each name, with its class.
fn classified() -> BTreeMap<String, String> {
    let start = GUIDE.find("<!-- classification:").expect("a classification in the guide");
    let marker = &GUIDE[start..];
    let marker = &marker[..marker.find("-->").expect("the classification's end")];
    let mut names = BTreeMap::new();
    for line in marker.lines() {
        let Some((class, list)) = line.split_once(": ") else { continue };
        if !CLASSES.contains(&class) {
            continue;
        }
        for name in list.split(',').map(str::trim) {
            let was = names.insert(name.to_owned(), class.to_owned());
            assert!(was.is_none(), "{name} classified twice: {was:?} and {class}");
        }
    }
    assert!(!names.is_empty(), "the guide's classification has no names");
    names
}

/// The commands `brnr --help` lists: `brnr <cmd> <verb>`, or `brnr <cmd>`
/// where the next word is an option, placeholder or none.
fn in_help() -> BTreeSet<String> {
    let env = Env::new("classify");
    let out = env.run(&["--help"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let help = String::from_utf8(out.stdout).unwrap();
    let mut found = BTreeSet::new();
    for line in help.lines().filter(|l| l.starts_with("  brnr ")) {
        let mut words = line.split_whitespace().skip(1);
        let cmd = words.next().unwrap();
        match words.next() {
            Some(verb) if verb.chars().all(|c| c.is_ascii_lowercase()) => {
                found.insert(format!("{cmd} {verb}"))
            }
            _ => found.insert(cmd.to_owned()),
        };
    }
    found
}

#[test]
fn every_command_and_entry_point_has_a_security_classification() {
    let parsed = parsed();
    let classified = classified();
    let unclassified: Vec<_> = parsed.iter().filter(|n| !classified.contains_key(*n)).collect();
    assert!(
        unclassified.is_empty(),
        "brnr has these, and docs/guide/agents.md's classification doesn't: {unclassified:?}"
    );
    let gone: Vec<_> = classified.keys().filter(|n| !parsed.contains(*n)).collect();
    assert!(
        gone.is_empty(),
        "the guide classifies these, which brnr's parser no longer has: {gone:?}"
    );
    let missed: Vec<_> = in_help().into_iter().filter(|n| !parsed.contains(n)).collect();
    assert!(
        missed.is_empty(),
        "brnr --help has these, and this test's reading of the parser missed them: {missed:?}"
    );
}

/// What the classification denies, the guide's paragraphs say to deny: each
/// is in them, in backticks, with or without `brnr `.
#[test]
fn what_the_classification_denies_the_guide_names() {
    let prose = &GUIDE[..GUIDE.find("<!-- classification:").unwrap()];
    let prose = prose.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, class) in classified() {
        if class == "deny" {
            let named =
                prose.contains(&format!("`{name}`")) || prose.contains(&format!("`brnr {name}`"));
            assert!(named, "{name} is classified deny, and the guide's prose doesn't name it");
        }
    }
}
