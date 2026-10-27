//! The control commands' arguments (ADR 8.1), read one way for every verb
//! from [`VERBS`], a table of each verb's flags, positionals and what may
//! follow them: `--json`, an unknown option, a flag's value, `--`, `-h` and
//! a usage error are each decided here, once.
//!
//! A word is one of the verb's flags, then `--json` where the verb takes it,
//! then `--` where the verb takes words after it; any other word starting
//! `--` is an unknown option, and the rest are positionals (`-x` too, ADR
//! 8.1). A flag that takes a value takes the next word, whatever it is, so
//! a value is never a positional. One positional too many, or one too few,
//! is a usage error, which the command line prints as the verb's usage.

use std::str::FromStr;

use crate::error;

/// What a flag takes after it, and so what `--flag` alone fails with
/// (`<flag> needs <what>`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Takes {
    /// Nothing: a switch.
    Nothing,
    /// A value; a later one replaces it.
    One(&'static str),
    /// A value, the same each time it is given (ADR 6.1).
    Same(&'static str),
    /// A value each time, every one kept.
    Many(&'static str),
    /// A number of seconds; a later one replaces it.
    Seconds,
    /// A number; a later one replaces it.
    Count,
}

impl Takes {
    /// What its error says the flag needs.
    fn what(self) -> &'static str {
        match self {
            Nothing => "nothing",
            One(what) | Same(what) | Many(what) => what,
            Seconds => "a value",
            Count => "a number",
        }
    }

    /// `value`, given with `flag`, checked as it is read.
    fn check(self, flag: &str, value: &str) -> Result<(), String> {
        match self {
            Seconds if value.parse::<u64>().is_err() => {
                Err(format!("{flag}: not a number of seconds: {value}"))
            }
            Count if value.parse::<usize>().is_err() => {
                Err(format!("{flag}: not a number: {value}"))
            }
            _ => Ok(()),
        }
    }
}

/// A flag a verb takes.
#[derive(Debug)]
pub(super) struct Flag(pub(super) &'static str, pub(super) Takes);

/// What may follow a verb's positionals.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Rest {
    /// Nothing: `--` is an unknown option.
    Nothing,
    /// `--` and the words after it: an agent's argument vector, or `event
    /// notify`'s command.
    Dashed,
}

/// A verb's arguments: `brnr <cmd> <verb>`, or `brnr <cmd>` without a group.
#[derive(Debug)]
pub(super) struct Spec {
    pub(super) cmd: &'static str,
    pub(super) verb: Option<&'static str>,
    json: bool,
    flags: &'static [Flag],
    /// As the usage names them; the first `required` must be given.
    positionals: &'static [&'static str],
    required: usize,
    rest: Rest,
    /// A flag given in place of the first positional, as `--pid <pid>` in
    /// `(<session> | --pid <pid>) <request>`.
    instead: Option<&'static str>,
    /// Whether one of `flags` must be given: there is nothing to do without.
    needs_flag: bool,
}

/// A verb whose positionals are all needed, with `--json` and nothing after.
const fn verb(cmd: &'static str, verb: &'static str, positionals: &'static [&'static str]) -> Spec {
    let required = positionals.len();
    Spec {
        cmd,
        verb: Some(verb),
        json: true,
        flags: &[],
        positionals,
        required,
        rest: Rest::Nothing,
        instead: None,
        needs_flag: false,
    }
}

/// A command outside the groups, `brnr <cmd>`.
const fn command(cmd: &'static str, positionals: &'static [&'static str]) -> Spec {
    Spec { verb: None, ..verb(cmd, "", positionals) }
}

impl Spec {
    const fn flags(self, flags: &'static [Flag]) -> Spec {
        Spec { flags, ..self }
    }
    const fn optional(self) -> Spec {
        Spec { required: 0, ..self }
    }
    const fn rest(self, rest: Rest) -> Spec {
        Spec { rest, ..self }
    }
    const fn no_json(self) -> Spec {
        Spec { json: false, ..self }
    }
    const fn instead(self, flag: &'static str) -> Spec {
        Spec { instead: Some(flag), ..self }
    }
    const fn needs_flag(self) -> Spec {
        Spec { needs_flag: true, ..self }
    }
}

use Error::Invalid;
use Takes::{Count, Many, Nothing, One, Same, Seconds};

const fn switch(name: &'static str) -> Flag {
    Flag(name, Nothing)
}

/// `session new`'s and `session resume`'s. `new` takes `--take-over` to say
/// it is `resume`'s.
const START: &[Flag] = &[
    Flag("--pid", One("a value")),
    Flag("--profile", One("a value")),
    Flag("--auth", One("a value")),
    switch("--strict"),
    Flag("--stop-when-idle", Seconds),
    Flag("--response-timeout", Seconds),
    switch("--foreground"),
    switch("--quiet"),
    Flag("--cwd", One("a value")),
    Flag("--mode", Same("a value")),
    Flag("--model", Same("a value")),
    Flag("--thought-level", Same("a value")),
    Flag("--option", Many("a value")),
    Flag("--prompt", One("a value")),
    Flag("--file", Many("a value")),
    Flag("--image", Many("a value")),
    switch("--wait"),
    Flag("--timeout", Seconds),
    switch("--take-over"),
];

const ELICITATION: &[Flag] = &[Flag("--pid", One("a pid"))];
const REQUEST: &[&str] = &["<session>", "<request>"];
const SESSION: &[&str] = &["<session>"];

/// Every verb's arguments, as the usage (`USAGE` in ctl.rs) shows them.
pub(super) const VERBS: &[Spec] = &[
    verb("process", "list", &[]),
    verb("process", "stop", &["<pid>"]),
    verb("session", "new", &[]).flags(START).rest(Rest::Dashed),
    verb("session", "resume", SESSION).flags(START).rest(Rest::Dashed),
    verb("session", "status", SESSION),
    verb("session", "fork", SESSION),
    verb("session", "close", SESSION),
    verb("session", "delete", SESSION)
        .flags(&[switch("--purge"), Flag("--profile", One("a name"))])
        .rest(Rest::Dashed),
    verb("session", "list", &[])
        .flags(&[
            Flag("--include", One("a list")),
            Flag("--profile", One("a name")),
            Flag("--cwd", One("a directory")),
        ])
        .rest(Rest::Dashed),
    verb("prompt", "send", SESSION).flags(&[
        switch("--steer"),
        switch("--interrupt"),
        switch("--context"),
        switch("--replace"),
        Flag("--prompt", One("a value")),
        Flag("--file", Many("a value")),
        Flag("--image", Many("a value")),
        switch("--wait"),
        Flag("--timeout", Seconds),
    ]),
    verb("prompt", "cancel", SESSION).flags(&[switch("--keep-held")]),
    verb("prompt", "commands", SESSION),
    verb("queue", "list", SESSION),
    verb("queue", "show", &["<session>", "<message>"]),
    verb("queue", "drop", &["<session>", "<message>"]),
    verb("queue", "clear", SESSION).flags(&[switch("--messages"), switch("--context")]),
    verb("permission", "requests", SESSION).optional(),
    verb("permission", "show", REQUEST),
    verb("permission", "allow", REQUEST)
        .flags(&[switch("--always"), Flag("--option-id", One("an id"))]),
    verb("permission", "reject", REQUEST)
        .flags(&[switch("--always"), Flag("--option-id", One("an id"))]),
    verb("elicitation", "requests", SESSION).optional(),
    verb("elicitation", "show", REQUEST).flags(ELICITATION).instead("--pid"),
    verb("elicitation", "accept", REQUEST)
        .flags(&[Flag("--pid", One("a pid")), Flag("--field", Many("<name>=<value>")), switch("-")])
        .instead("--pid"),
    verb("elicitation", "decline", REQUEST).flags(ELICITATION).instead("--pid"),
    verb("elicitation", "cancel", REQUEST).flags(ELICITATION).instead("--pid"),
    verb("config", "get", SESSION).flags(&[
        switch("--mode"),
        switch("--model"),
        switch("--thought-level"),
        Flag("--option", Many("an option's id")),
    ]),
    verb("config", "set", SESSION)
        .flags(&[
            Flag("--mode", Same("a value")),
            Flag("--model", Same("a value")),
            Flag("--thought-level", Same("a value")),
            Flag("--option", Many("<option>=<value>")),
        ])
        .needs_flag(),
    verb("event", "log", SESSION).flags(&[
        Flag("--last", Count),
        switch("--follow"),
        Flag("--events", One("a list")),
    ]),
    verb("event", "watch", SESSION)
        .flags(&[Flag("--pid", One("a pid")), Flag("--events", One("a list"))])
        .optional(),
    // Its stdout is its command's (ADR 8.2).
    verb("event", "notify", SESSION)
        .flags(&[Flag("--pid", One("a pid")), switch("--stdin"), Flag("--events", One("a list"))])
        .optional()
        .rest(Rest::Dashed)
        .no_json(),
    verb("event", "wait", SESSION)
        .flags(&[Flag("--for", One("a value")), Flag("--timeout", Seconds)]),
    command("doctor", &[]).flags(&[switch("--fix"), switch("--report")]),
    verb("remote", "check", &["<target>"])
        .flags(&[Flag("--profile", One("a local routing profile"))])
        .instead("--profile"),
    command("skill", &["<reference>"]).optional().no_json(),
    verb("skill", "install", &[]).flags(&[Flag("--dir", Many("a directory"))]).no_json(),
];

/// The arguments of `brnr <cmd> <verb>`, if it is a verb.
pub(super) fn spec(cmd: &str, verb: Option<&str>) -> Option<&'static Spec> {
    VERBS.iter().find(|s| s.cmd == cmd && s.verb == verb)
}

const TRANSPORT: &[Flag] =
    &[Flag("--remote", One("a destination")), Flag("--profile", One("a profile"))];
const ACP: Spec = command("acp", &[])
    .flags(&[Flag("--profile", One("a profile")), switch("--strict")])
    .rest(Rest::Dashed)
    .no_json();
const VERSION: Spec = command("--version", &[]).no_json();

/// Validated public arguments, with transport/profile selection separated
/// from the words that must reach the remote command (ADR 12.1).
pub(crate) struct Routing {
    pub(crate) remote: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) help: bool,
    pub(crate) default_profile: bool,
    pub(crate) json: bool,
    prefix: Vec<String>,
    rest: Vec<String>,
    takes_profile: bool,
}

impl Routing {
    pub(crate) fn command(&self, profile: Option<&str>) -> Vec<String> {
        let mut words = self.prefix.clone();
        if self.takes_profile
            && let Some(profile) = profile
        {
            words.extend(["--profile".into(), profile.into()]);
        }
        words.extend(self.rest.clone());
        words
    }
}

/// Uses the command table, so prompts and words after `--` can never turn
/// into transport flags. Invalid local syntax stays with its usual parser.
pub(crate) fn routing(words: &[String]) -> Option<Routing> {
    let cmd = words.first()?.as_str();
    if cmd == "remote" {
        return None;
    }
    let (spec, start) = match cmd {
        "acp" => (&ACP, 1),
        "-V" | "--version" => (&VERSION, 1),
        "skill" if words.get(1).is_some_and(|v| v == "install") => (spec(cmd, Some("install"))?, 2),
        _ => match spec(cmd, None) {
            Some(spec) => (spec, 1),
            None => (spec(cmd, words.get(1).map(String::as_str))?, 2),
        },
    };
    let mut rest = words[start..].to_vec();
    // ACP already accepts --profile=p; keep that spelling working remotely.
    if cmd == "acp" {
        let mut normal = Vec::new();
        let mut it = rest.into_iter();
        while let Some(word) = it.next() {
            if word == "--" {
                normal.push(word);
                normal.extend(it);
                break;
            }
            if let Some((key @ ("--profile" | "--remote"), value)) = word.split_once('=') {
                normal.extend([key.into(), value.into()]);
            } else {
                let takes = spec.takes_value(&word);
                normal.push(word);
                if takes && let Some(value) = it.next() {
                    normal.push(value);
                }
            }
        }
        rest = normal;
    }
    let help = spec.asks_help(&rest);
    let parsed = if help { None } else { Some(spec.parse_inner(&rest, true).ok()?) };
    let mut route = Routing {
        remote: None,
        profile: None,
        help,
        default_profile: cmd == "acp"
            || (cmd == "session" && matches!(spec.verb, Some("new" | "resume"))),
        prefix: words[..start].to_vec(),
        rest: Vec::new(),
        takes_profile: spec.flags.iter().any(|f| f.0 == "--profile"),
        json: false,
    };
    if let Some(a) = parsed {
        route.json = a.json;
        route.remote = a.value("--remote").map(str::to_owned);
        route.profile = a.value("--profile").map(str::to_owned);
        for (flag, value) in a.given {
            if flag == "--remote" || flag == "--profile" {
                continue;
            }
            route.rest.push(flag.into());
            route.rest.extend(value);
        }
        if a.json {
            route.rest.push("--json".into());
        }
        route.rest.extend(a.positional);
        if let Some(rest) = a.rest {
            route.rest.push("--".into());
            route.rest.extend(rest);
        }
    }
    Some(route)
}

/// Why a command didn't do what it was asked (ADR 8.3): a usage error,
/// which exits 2 with the command's usage on stderr, or a failure, 1, with
/// its code (ADR 8.7).
#[derive(Debug, PartialEq)]
pub(super) enum Error {
    /// Used wrongly: the command line prints the verb's usage.
    Usage,
    /// Used wrongly, as the message says: printed before the usage.
    Invalid(String),
    /// The operation failed, as the error says.
    Failed(error::Error),
}

impl Error {
    /// The exit status it ends the command with.
    pub(super) fn code(&self) -> u8 {
        match self {
            Error::Usage | Error::Invalid(_) => 2,
            Error::Failed(_) => 1,
        }
    }
}

/// A failure brnr gives no code of its own: `unclassified`.
impl From<String> for Error {
    fn from(msg: String) -> Error {
        Error::Failed(msg.into())
    }
}

impl From<&str> for Error {
    fn from(msg: &str) -> Error {
        Error::Failed(msg.into())
    }
}

impl From<error::Error> for Error {
    fn from(error: error::Error) -> Error {
        Error::Failed(error)
    }
}

/// A verb's arguments, read as its [`Spec`] has them.
#[derive(Debug)]
pub(super) struct Args {
    spec: &'static Spec,
    /// The flags given, in order, each with its value if it takes one.
    given: Vec<(&'static str, Option<String>)>,
    pub(super) positional: Vec<String>,
    /// What followed `--`, if it was given.
    pub(super) rest: Option<Vec<String>>,
    pub(super) json: bool,
}

impl Spec {
    /// `args`, the words after the verb, read; a mistake the first one
    /// found, left to right.
    pub(super) fn parse(&'static self, args: &[String]) -> Result<Args, Error> {
        self.parse_inner(args, false)
    }

    fn parse_inner(&'static self, args: &[String], transport: bool) -> Result<Args, Error> {
        let mut a =
            Args { spec: self, given: Vec::new(), positional: Vec::new(), rest: None, json: false };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let flags = self.flags.iter().chain(if transport { TRANSPORT } else { &[] }.iter());
            if let Some(&Flag(name, takes)) = flags.into_iter().find(|f| f.0 == arg) {
                let value = match takes {
                    Nothing => None,
                    _ => Some(
                        it.next()
                            .ok_or_else(|| Invalid(format!("{name} needs {}", takes.what())))?
                            .clone(),
                    ),
                };
                if let Some(value) = &value {
                    takes.check(name, value).map_err(Invalid)?;
                }
                if let (Same(_), Some(value)) = (takes, &value)
                    && let Some(was) = a.value(name)
                    && was != value
                {
                    return Err(Invalid(format!("{name} {was} and {name} {value} disagree")));
                }
                a.given.push((name, value));
            } else if arg == "--json" && self.json {
                a.json = true;
            } else if arg == "--" && self.rest != Rest::Nothing {
                a.rest = Some(it.by_ref().cloned().collect());
            } else if arg.starts_with("--") {
                return Err(Invalid(format!("unknown option: {arg}")));
            } else if a.positional.len() < self.positionals.len() {
                a.positional.push(arg.clone());
            } else {
                return Err(Error::Usage);
            }
        }
        let instead = self.instead.is_some_and(|flag| a.has(flag)) as usize;
        let (min, max) = (self.required.saturating_sub(instead), self.positionals.len() - instead);
        let flags = !self.needs_flag
            || a.given.iter().any(|(name, _)| self.flags.iter().any(|f| f.0 == *name));
        match (min..=max).contains(&a.positional.len()) && flags {
            true => Ok(a),
            false => Err(Error::Usage),
        }
    }

    /// Whether `args` ask for the verb's usage: `-h` or `--help` as an
    /// option, not a flag's value, before any `--`, after which they are the
    /// agent's or the command's (ADR 8.1).
    pub(super) fn asks_help(&self, args: &[String]) -> bool {
        let help = |a: &str| a == "-h" || a == "--help";
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "--" {
                break;
            }
            if help(arg) {
                return true;
            }
            // A flag's value is skipped, whatever it is; `--` ends brnr's
            // options even where a value was due (ADR 8.1).
            if self.takes_value(arg) && it.next().is_none_or(|v| v == "--") {
                break;
            }
        }
        false
    }

    /// Whether `flag` is one of the verb's that takes a value.
    fn takes_value(&self, flag: &str) -> bool {
        self.flags.iter().chain(TRANSPORT).any(|f| f.0 == flag && f.1 != Nothing)
    }
}

impl Args {
    /// The values `flag` was given with, in order; once for each time, `None`
    /// for a switch. A flag the verb doesn't take is a mistake here, not
    /// the user's.
    fn all(&self, flag: &str) -> impl Iterator<Item = &Option<String>> {
        assert!(
            self.spec.flags.iter().chain(TRANSPORT).any(|f| f.0 == flag),
            "{flag} isn't {:?}'s",
            self.spec
        );
        self.given.iter().filter(move |(name, _)| *name == flag).map(|(_, value)| value)
    }

    /// Whether `flag` was given.
    pub(super) fn has(&self, flag: &str) -> bool {
        self.all(flag).next().is_some()
    }

    /// The flags given, in order.
    pub(super) fn given(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.given.iter().map(|(name, _)| *name)
    }

    /// `flag`'s value: the last given.
    pub(super) fn value(&self, flag: &str) -> Option<&str> {
        self.all(flag).filter_map(Option::as_deref).last()
    }

    /// Every value `flag` was given, in order.
    pub(super) fn values(&self, flag: &str) -> Vec<String> {
        self.all(flag).flatten().cloned().collect()
    }

    /// `flag`'s value as `read` reads it: each one given, in order, so a
    /// wrong one fails though a later one replaces it; the last.
    pub(super) fn value_as<T>(
        &self,
        flag: &str,
        mut read: impl FnMut(&str) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let mut last = None;
        for value in self.all(flag).flatten() {
            last = Some(read(value)?);
        }
        Ok(last)
    }

    /// `flag`'s value, a number (`Seconds` or `Count`), checked as it was read.
    pub(super) fn number<T: FromStr>(&self, flag: &str) -> Option<T> {
        self.value(flag).and_then(|v| v.parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{USAGE, usage_of};
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// The positionals a verb needs, `--pid` standing for the first where it
    /// can, and the words of each.
    fn needed(spec: &Spec, flag: &str) -> Vec<String> {
        let skip = (spec.instead == Some(flag)) as usize;
        spec.positionals[skip..spec.required.max(skip)].iter().map(|p| format!("{p}-arg")).collect()
    }

    /// For every verb, a flag's value is the flag's, before the positionals
    /// or after them, whatever it looks like: never a positional, `--json`,
    /// or help.
    #[test]
    fn adr_0008_a_flags_value_is_never_a_positional() {
        for spec in VERBS {
            for &Flag(flag, takes) in spec.flags.iter().filter(|f| f.1 != Nothing) {
                let at = format!("{} {:?} {flag}", spec.cmd, spec.verb);
                let positionals = needed(spec, flag);
                for value in ["1", "value", "--json", "-h", "--"] {
                    let before =
                        [vec![flag.to_owned(), value.to_owned()], positionals.clone()].concat();
                    let after =
                        [positionals.clone(), vec![flag.to_owned(), value.to_owned()]].concat();
                    for args in [before, after] {
                        // A number's, though it isn't one.
                        let a = match spec.parse(&args) {
                            Err(Error::Invalid(m)) if matches!(takes, Seconds | Count) => {
                                assert!(m.starts_with(&format!("{flag}: not a number")), "{m}");
                                assert!(m.ends_with(&format!(": {value}")), "{at} {args:?}: {m}");
                                continue;
                            }
                            a => a.unwrap_or_else(|e| panic!("{at} {args:?}: {e:?}")),
                        };
                        assert_eq!(a.positional, positionals, "{at} {args:?}");
                        assert_eq!(a.value(flag), Some(value), "{at} {args:?}");
                        assert_eq!((a.json, &a.rest), (false, &None), "{at} {args:?}");
                        assert!(!spec.asks_help(&args), "{at} {args:?}");
                    }
                }
                let alone = [positionals.clone(), vec![flag.to_owned()]].concat();
                let needs = Error::Invalid(format!("{flag} needs {}", takes.what()));
                assert_eq!(spec.parse(&alone).err(), Some(needs), "{at}");
            }
        }
    }

    /// The words of `text`: flags, `<placeholders>` and `-` alone.
    fn tokens(text: &str) -> Vec<&str> {
        text.split(|c: char| !(c.is_alphanumeric() || "-<>".contains(c)))
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Every verb is in the usage, with each of its flags and positionals,
    /// `--json` where it takes it, and `--` where words follow it. The one
    /// exception: `session new` takes `--take-over` to say it is `resume`'s.
    #[test]
    fn adr_0008_every_flag_is_in_its_verbs_usage() {
        for spec in VERBS {
            let usage = usage_of(spec.cmd, spec.verb);
            assert_ne!(usage, USAGE, "{} {:?} has no usage", spec.cmd, spec.verb);
            let words = tokens(&usage);
            let at = format!("{} {:?}: {usage}", spec.cmd, spec.verb);
            for Flag(flag, _) in spec.flags {
                let exception =
                    (spec.cmd, spec.verb, *flag) == ("session", Some("new"), "--take-over");
                assert_eq!(words.contains(flag), !exception, "{flag} in {at}");
            }
            for positional in spec.positionals {
                assert!(words.contains(positional), "{positional} in {at}");
            }
            assert_eq!(words.contains(&"--json"), spec.json, "--json in {at}");
            assert_eq!(spec.rest == Rest::Dashed, words.contains(&"--"), "-- in {at}");
        }
        for v in VERBS {
            let same = VERBS.iter().filter(|s| (s.cmd, s.verb) == (v.cmd, v.verb));
            assert_eq!(same.count(), 1, "{} {:?} twice", v.cmd, v.verb);
        }
    }

    /// A positional too many or too few is a usage error; `--pid` takes the
    /// first's place where it can; a flag the verb doesn't take is unknown,
    /// `--json` and `--` too where it doesn't take them; `-x` is a positional.
    #[test]
    fn adr_0008_positionals_and_unknown_options() {
        let parse = |cmd, verb, args: &[&str]| spec(cmd, Some(verb)).unwrap().parse(&words(args));
        assert_eq!(parse("session", "status", &[]).err(), Some(Error::Usage));
        assert_eq!(parse("session", "status", &["s", "t"]).err(), Some(Error::Usage));
        assert_eq!(parse("session", "status", &["-x"]).unwrap().positional, ["-x"]);
        let unknown = |flag: &str| Some(Error::Invalid(format!("unknown option: {flag}")));
        assert_eq!(parse("session", "status", &["s", "--"]).err(), unknown("--"));
        assert_eq!(parse("event", "notify", &["s", "--json"]).err(), unknown("--json"));
        assert_eq!(parse("session", "new", &["--take-over", "--bogus"]).err(), unknown("--bogus"));
        assert!(parse("permission", "requests", &[]).is_ok());
        assert!(parse("elicitation", "show", &["--pid", "1", "r"]).is_ok());
        assert_eq!(
            parse("elicitation", "show", &["--pid", "1", "s", "r"]).err(),
            Some(Error::Usage)
        );
        assert_eq!(parse("elicitation", "show", &["r"]).err(), Some(Error::Usage));
        assert_eq!(parse("config", "set", &["s", "--json"]).err(), Some(Error::Usage));
        let err = parse("config", "set", &["s", "--mode", "a", "--mode", "b"]).err();
        assert_eq!(err, Some(Error::Invalid("--mode a and --mode b disagree".into())));
        let a = parse(
            "session",
            "new",
            &["--file", "a", "--cwd", "x", "--file", "b", "--cwd", "y", "--", "x"],
        );
        let a = a.unwrap();
        assert_eq!((a.values("--file"), a.value("--cwd")), (words(&["a", "b"]), Some("y")));
        assert_eq!(a.rest, Some(words(&["x"])));
    }

    /// `prompt send`'s prompt is `--prompt`'s value, taken as given, as a
    /// start's is: a word left over is a usage error, and `--` is unknown.
    #[test]
    fn adr_0008_a_prompt_is_a_flags_value() {
        let send = spec("prompt", Some("send")).unwrap();
        let a = send.parse(&words(&["--wait", "s", "--file", "f", "--prompt", "-h"])).unwrap();
        assert_eq!(a.positional, words(&["s"]));
        assert_eq!((a.values("--file"), a.value("--prompt")), (words(&["f"]), Some("-h")));
        assert_eq!(send.parse(&words(&["s", "hi"])).err(), Some(Error::Usage));
        assert_eq!(send.parse(&words(&["s", "--prompt", "hi", "there"])).err(), Some(Error::Usage));
        let unknown = Some(Error::Invalid("unknown option: --".into()));
        assert_eq!(send.parse(&words(&["s", "--", "hi"])).err(), unknown);
        assert!(send.asks_help(&words(&["s", "--wait", "-h"])));
        assert!(!send.asks_help(&words(&["s", "--prompt", "-h"])));
        assert!(!send.asks_help(&words(&["s", "--prompt", "--help"])));
        let status = spec("session", Some("status")).unwrap();
        assert!(status.asks_help(&words(&["--help"])));
        assert!(!spec("session", Some("new")).unwrap().asks_help(&words(&["--", "-h"])));
        let new = spec("session", Some("new")).unwrap();
        assert!(!new.asks_help(&words(&["--prompt", "-h"])));
        assert!(!new.asks_help(&words(&["--cwd", "--", "-h"])));
    }
}
