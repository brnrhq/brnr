//! Explicit remote execution through the user's SSH client (ADR 12).
//! Routing happens before local profile resolution and socket discovery.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use crate::config::{self, ConfigFile, Remote};
use crate::{ctl, paths};

/// None leaves a local command to its existing parser and implementation.
pub(crate) fn dispatch(args: &[OsString], received: bool) -> Option<ExitCode> {
    let words: Vec<String> =
        args.iter().map(|s| s.to_str().map(str::to_owned)).collect::<Option<_>>()?;
    let route = match ctl::args::routing(&words) {
        Some(route) => route,
        None if words.first().is_some_and(|w| matches!(w.as_str(), "-V" | "--version"))
            && words.iter().take_while(|w| *w != "--").any(|w| w == "--remote") =>
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
    if route.remote.is_none() && route.profile.is_none() && !route.default_profile {
        return None;
    }
    let file = match config::load_file() {
        Ok(file) => file.unwrap_or_default(),
        // A local command retains its existing error code and diagnostics.
        Err(_) if route.remote.is_none() => return None,
        Err(problems) => {
            let error = crate::error::Error::new(
                crate::error::Code::config_invalid,
                format!("{}: {}", paths::config_file().display(), problems.join("; ")),
            );
            ctl::failed(&error, route.json);
            return Some(ExitCode::FAILURE);
        }
    };
    let result = (|| -> Result<Option<Command>, String> {
        let mut profile = route.profile.as_deref();
        let target = match route.remote.as_deref() {
            Some(target) => target,
            None => {
                let name = profile.unwrap_or("default");
                let Some(remote) = file.profiles.get(name).and_then(|p| p.remote.as_ref()) else {
                    return Ok(None);
                };
                profile = remote.profile.as_deref();
                &remote.target
            }
        };
        if received {
            return Err(
                "nested remote routing is refused; select an execution profile on the destination"
                    .into(),
            );
        }
        let remote = destination(&file, target)?;
        Ok(Some(ssh(&remote, &route.command(profile), route.json)?))
    })();
    match result {
        Ok(None) => None,
        Ok(Some(mut command)) => {
            // No extra relay, buffering or child process: the editor now owns
            // ssh. Its standard streams and exit status are SSH's (ADR 12.1).
            let error = command.exec();
            Some(failed(
                &format!("cannot execute ssh: {error}; install the OpenSSH client"),
                route.json,
            ))
        }
        Err(error) => Some(failed(&error, route.json)),
    }
}

fn failed(message: &str, json: bool) -> ExitCode {
    ctl::failed(&format!("remote: {message}").into(), json);
    ExitCode::FAILURE
}

pub(crate) fn configuration() -> Result<ConfigFile, String> {
    config::load_file().map_err(|e| e.join("; ")).map(Option::unwrap_or_default)
}

pub(crate) fn destination(file: &ConfigFile, name: &str) -> Result<Remote, String> {
    let remote = file.remotes.get(name).cloned().unwrap_or_else(|| Remote {
        host: name.into(),
        executable: "brnr".into(),
        cwd: None,
        runtime_dir: None,
        home: None,
        config: None,
    });
    validate(&remote)?;
    Ok(remote)
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

pub(crate) fn validate(remote: &Remote) -> Result<(), String> {
    validate_host(&remote.host)?;
    if remote.executable.is_empty()
        || remote.executable.starts_with('-')
        || remote.executable.contains('\0')
        || remote.executable.starts_with('~')
    {
        return Err(
            "executable must be a program on the remote PATH or an absolute path (no ~ expansion)"
                .into(),
        );
    }
    if remote.executable.contains('/') && !remote.executable.starts_with('/') {
        return Err("executable must be a program on the remote PATH or an absolute path".into());
    }
    for (key, value) in [
        ("cwd", &remote.cwd),
        ("runtime_dir", &remote.runtime_dir),
        ("home", &remote.home),
        ("config", &remote.config),
    ] {
        if let Some(value) = value
            && (!value.starts_with('/') || value.contains('\0'))
        {
            return Err(format!("{key} must be an absolute remote path (no ~ expansion)"));
        }
    }
    Ok(())
}

/// A POSIX shell command, with every user-provided word quoted. The version
/// probe has no access to the ACP/prompt input; only the final exec does.
fn script(remote: &Remote, args: &[String], json: bool) -> Result<String, String> {
    validate(remote)?;
    if args.iter().any(|a| a.contains('\0')) {
        return Err("an argument contains NUL".into());
    }
    let mut program = vec!["env".into()];
    for (name, value) in [
        ("BRNR_DIR", &remote.runtime_dir),
        ("BRNR_HOME", &remote.home),
        ("BRNR_CONFIG", &remote.config),
    ] {
        if let Some(value) = value {
            program.push(format!("{name}={value}"));
        }
    }
    program.push(remote.executable.clone());
    let program = program.iter().map(|w| paths::shell_word(w)).collect::<Vec<_>>().join(" ");
    let expected = paths::shell_word(concat!("brnr ", env!("CARGO_PKG_VERSION")));
    let error_json = if json {
        format!(
            "printf '%s\\n' {}; ",
            paths::shell_word(
                r#"{"error":{"code":"unclassified","message":"remote brnr is unavailable or its version differs; run brnr remote check"}}"#
            )
        )
    } else {
        String::new()
    };
    let mut script = String::new();
    if let Some(cwd) = &remote.cwd {
        script.push_str(&format!("cd {} || {{ printf '%s\\n' 'brnr: remote cwd unavailable [unclassified]' >&2; {error_json}exit 1; }}; ", paths::shell_word(cwd)));
    }
    script.push_str(&format!(
        "v=$({program} _remote --version </dev/null) || {{ printf '%s\\n' 'brnr: remote executable unavailable or lacks remote support; check the configured remote executable [unclassified]' >&2; {error_json}exit 1; }}; \
         if [ \"$v\" != {expected} ]; then printf '%s\\n' \"brnr: remote version mismatch: expected \"{expected}\", got $v; install matching brnr versions [unclassified]\" >&2; {error_json}exit 1; fi; exec {program} _remote"
    ));
    for arg in args {
        script.push(' ');
        script.push_str(&paths::shell_word(arg));
    }
    Ok(script)
}

fn ssh(remote: &Remote, args: &[String], json: bool) -> Result<Command, String> {
    let mut command = Command::new("ssh");
    command.args(["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"]);
    command.arg(&remote.host).arg(script(remote, args, json)?);
    Ok(command)
}

pub(crate) fn check(remote: &Remote, json: bool) -> Result<ExitCode, String> {
    let mut args = vec!["doctor".into()];
    if json {
        args.push("--json".into());
    }
    let error = ssh(remote, &args, json)?.exec();
    Err(format!("cannot execute ssh: {error}; install the OpenSSH client"))
}
