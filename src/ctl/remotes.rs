//! Remote target setup and management (ADR 12.2).

use super::{Args, args, print_json};
use crate::remote;
use serde_json::json;
use std::process::ExitCode;

pub(super) fn main(verb: &str, a: &Args) -> Result<ExitCode, args::Error> {
    let file = remote::configuration()?;
    match verb {
        "list" => {
            if a.json {
                let remotes: Vec<_> = file
                    .remotes
                    .iter()
                    .map(|(name, settings)| json!({"name": name, "settings": settings}))
                    .collect();
                print_json(&json!({"remotes": remotes}))?;
            } else {
                for (name, remote) in &file.remotes {
                    show(name, remote);
                }
            }
        }
        "check" => {
            return remote::check(&remote::destination(&file, &a.positional[0])?, a.json)
                .map_err(Into::into);
        }
        "init" => {
            let name = &a.positional[0];
            let mut settings = remote::destination(&file, name)?;
            if let Some(host) = a.value("--host") {
                settings.host = host.into();
            }
            if let Some(executable) = a.value("--executable") {
                settings.executable = executable.into();
            }
            for (flag, value) in [
                ("--cwd", &mut settings.cwd),
                ("--runtime-dir", &mut settings.runtime_dir),
                ("--home", &mut settings.home),
                ("--config", &mut settings.config),
            ] {
                if let Some(given) = a.value(flag) {
                    *value = Some(given.into());
                }
            }
            remote::init(name, &settings)?;
            if a.json {
                print_json(&json!({"remote": name, "settings": settings}))?;
            } else {
                show(name, &settings);
            }
        }
        "remove" => {
            let name = &a.positional[0];
            remote::remove(name)?;
            if a.json {
                print_json(&json!({"remote": name, "removed": true}))?;
            } else {
                outln!("remote {name}: removed");
            }
        }
        _ => unreachable!("remote verb was validated"),
    }
    Ok(ExitCode::SUCCESS)
}

fn show(name: &str, remote: &crate::config::Remote) {
    outln!("remote {name}: {} ({})", remote.host, remote.executable);
    for (key, value) in [
        ("cwd", &remote.cwd),
        ("runtime_dir", &remote.runtime_dir),
        ("home", &remote.home),
        ("config", &remote.config),
    ] {
        outln!("  {key}: {}", value.as_deref().unwrap_or("(default)"));
    }
}
