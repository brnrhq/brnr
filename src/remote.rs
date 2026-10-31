//! Explicit remote execution through the user's SSH client (ADR 12).
//! Routing happens before local profile resolution and socket discovery.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use crate::error::{Code, Error};
use crate::{ctl, paths};

/// None leaves a local command to its existing parser and implementation:
/// without `--remote`, only `remote check` connects through brnr's
/// transport (ADR 12.2).
pub(crate) fn dispatch(args: &[OsString], received: bool) -> Option<ExitCode> {
    let words: Vec<String> =
        args.iter().map(|s| s.to_str().map(str::to_owned)).collect::<Option<_>>()?;
    let route = match ctl::args::routing(&words) {
        Some(route) => route,
        None if words.first().is_some_and(|w| matches!(w.as_str(), "-V" | "--version"))
            && words
                .iter()
                .take_while(|w| *w != "--")
                .any(|w| matches!(w.as_str(), "--remote" | "--profile")) =>
        {
            eprintln!(
                "brnr: invalid remote version command\nusage: brnr --version [--remote <target>]"
            );
            return Some(ExitCode::from(2));
        }
        None => return None,
    };
    if route.help {
        return if words[0] == "acp" {
            Some(crate::proxy::main(["--help".into()].into_iter()))
        } else {
            None
        };
    }
    // An explicit destination belongs entirely to SSH. It needs no local
    // configuration, not even a valid config file: the destination loads
    // `--profile`, from its own.
    let target = route.remote.as_deref()?;
    if received {
        return Some(failed(
            &"nested remote routing is refused: the destination runs the command itself".into(),
            route.json,
        ));
    }
    match ssh(target, &route.command(), route.json) {
        Ok(mut command) => {
            // No extra relay, buffering or child process: the editor now owns
            // ssh. Its standard streams and exit status are SSH's (ADR 12.1).
            let error = command.exec();
            Some(failed(
                &format!("cannot execute ssh: {error}; install the OpenSSH client").into(),
                route.json,
            ))
        }
        Err(error) => Some(failed(&error, route.json)),
    }
}

fn failed(error: &Error, json: bool) -> ExitCode {
    ctl::failed(&error.clone().about("remote"), json);
    ExitCode::FAILURE
}

/// Destination is one SSH argument, never an option or a command string.
pub(crate) fn validate_host(host: &str) -> Result<(), String> {
    if host.is_empty()
        || host.starts_with('-')
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || "-._@:%[]+".contains(c))
    {
        return Err(format!("invalid SSH destination {host:?}; use [user@]host or an SSH alias"));
    }
    Ok(())
}

/// The shell words that fail the remote command with `code` and `message`:
/// on stderr as brnr says a failure, and with `--json` as its error object
/// too (ADR 8.7).
fn fail(code: Code, message: &str, json: bool) -> String {
    let error = Error::new(code, format!("remote: {message}"));
    let line = format!("brnr: {}", crate::render::coded(&error.message, code.name()));
    let mut words = format!("printf '%s\\n' {} >&2; ", paths::shell_word(&line));
    if json {
        let object = serde_json::json!({ "error": error.json() }).to_string();
        words.push_str(&format!("printf '%s\\n' {}; ", paths::shell_word(&object)));
    }
    words + "exit 1"
}

/// A POSIX shell command, with every user-provided word quoted. brnr is the
/// one on the remote shell's PATH, its environment the shell's (ADR 12.2).
/// The version probe has no access to the ACP/prompt input; only the final
/// exec does.
fn script(args: &[String], json: bool) -> Result<String, Error> {
    if args.iter().any(|a| a.contains('\0')) {
        return Err("an argument contains NUL".into());
    }
    let expected = concat!("brnr ", env!("CARGO_PKG_VERSION"));
    let missing = fail(
        Code::remote_brnr_not_found,
        "brnr is not on the remote shell's PATH. ssh runs the command in a non-interactive \
         shell, which reads ~/.zshenv (zsh) or ~/.bashrc (bash), not ~/.zprofile or \
         ~/.bash_profile: add brnr's folder to PATH there, on the destination",
        json,
    );
    let old = fail(
        Code::remote_version_mismatch,
        &format!("the remote brnr has no remote support: install {expected} on the destination"),
        json,
    );
    let differs = fail(
        Code::remote_version_mismatch,
        &format!("the remote brnr is another version: install {expected} on the destination"),
        json,
    );
    let mut script = format!(
        "command -v brnr >/dev/null 2>&1 || {{ {missing}; }}; \
         v=$(brnr _remote --version </dev/null 2>/dev/null) || {{ {old}; }}; \
         [ \"$v\" = {} ] || {{ {differs}; }}; exec brnr _remote",
        paths::shell_word(expected)
    );
    for arg in args {
        script.push(' ');
        script.push_str(&paths::shell_word(arg));
    }
    Ok(script)
}

fn ssh(target: &str, args: &[String], json: bool) -> Result<Command, Error> {
    validate_host(target)?;
    let mut command = Command::new("ssh");
    command.args(["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"]);
    command.arg(target).arg(script(args, json)?);
    Ok(command)
}

/// `remote check <target>`: the version preflight, then the destination's
/// doctor (ADR 12.2).
pub(crate) fn check(target: &str, json: bool) -> Result<ExitCode, Error> {
    let mut args = vec!["doctor".into()];
    if json {
        args.push("--json".into());
    }
    let error = ssh(target, &args, json)?.exec();
    Err(format!("cannot execute ssh: {error}; install the OpenSSH client").into())
}
