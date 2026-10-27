//! `brnr profile` (ADR 6.5 in docs/adr): the profiles, read and never
//! written. `list` has the config file's profiles, `show` one of them as a
//! start would use it, or what a running process was started with (`--pid`,
//! from its socket, not the file), and `keys` every key a profile can have.
//!
//! What they say comes from the one table of keys in profiles.rs that the
//! layout check reads too. The values of MCP servers' `env` and `headers`
//! are `<redacted>`, their keys kept (ADR 9.1); arguments are not.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::error::{self, Code};
use crate::log::REDACTED;
use crate::paths;
use crate::profiles::{self, KEYS, Key, Part};

use super::{Args, discover, doctor, print_json, process};

/// `profile list`: the config file, and each profile with doctor's line for
/// it. No file is no profiles, and defaults apply; a file that doesn't load
/// fails it.
pub(super) fn list(a: &Args) -> Result<(), error::Error> {
    let path = paths::config_file();
    let config = profiles::load_all().map_err(|problems| {
        let error = format!("{}: {}", path.display(), problems.join("; "));
        error::Error::new(Code::config_invalid, error)
    })?;
    let found = config.as_ref().map(|c| c.profiles.iter().collect::<Vec<_>>()).unwrap_or_default();
    if a.json {
        let rows: Vec<Value> = found.iter().map(|(name, p)| summary(name, p)).collect();
        let shown = json!({ "config": path, "exists": config.is_some(), "profiles": rows });
        return print_json(&shown);
    }
    match &config {
        None => outln!("{}: none; defaults apply", path.display()),
        Some(_) => outln!("{}: {} profiles", path.display(), found.len()),
    }
    for (name, profile) in found {
        outln!("{name}: {}", doctor::describe_profile(profile));
    }
    Ok(())
}

/// A profile's `list` row: what doctor's line for it says.
fn summary(name: &str, p: &profiles::Profile) -> Value {
    let names = |it: Vec<&str>| json!(it);
    json!({
        "name": name,
        "agent": p.agent,
        "bridges": p.bridges.len(),
        "mcp_servers": p.headless.mcp_servers.len(),
        "strict": p.strict,
        "experimental": names(p.editor.experimental.iter().map(|e| e.name()).collect()),
        "client_capabilities":
            names(p.editor.client_capabilities.iter().map(|c| c.name()).collect()),
        "echo_as": p.editor.echo_as.name(),
        "features": names(p.editor.features.iter().map(|f| f.name()).collect()),
    })
}

/// `profile show [<name> | --pid <pid>]`.
pub(super) fn show(a: &Args) -> Result<(), error::Error> {
    if let Some(pid) = a.value("--pid") {
        return show_pid(pid, a.json);
    }
    let (name, found, table) = profiles::load_table(a.positional.first().map(String::as_str))?;
    let path = paths::config_file();
    let (mut profile, mut unset) = (Map::new(), Vec::new());
    for part in Part::ALL {
        let mut values = Map::new();
        for key in part.keys() {
            let value = match part_of(&table, part).and_then(|t| t.get(key.name)) {
                Some(value) => as_json(&redacted(key, value)),
                None => {
                    unset.push(path_of(part, key));
                    key.default_value().map_or(Value::Null, |v| as_json(&v))
                }
            };
            values.insert(key.name.into(), value);
        }
        match part.key() {
            None => profile.extend(values),
            Some(at) => drop(profile.insert(at.into(), Value::Object(values))),
        }
    }
    if a.json {
        let shown = json!({
            "config": path, "name": name, "found": found, "profile": profile, "unset": unset,
        });
        return print_json(&shown);
    }
    let mut text = match (found, path.exists()) {
        (true, _) => format!("# profile {name}, in {}\n", path.display()),
        (false, true) => {
            format!("# no profile {name} in {}: the empty profile applies\n", path.display())
        }
        (false, false) => {
            format!("# no config file {}: the empty profile applies\n", path.display())
        }
    };
    let at = format!("profiles.{}", toml_key(&name));
    for part in Part::ALL {
        let values = part_of(&table, part);
        let _ = writeln!(text, "\n{}  # {}", part.table(&at), part.users());
        let mut blocks = Vec::new();
        for key in part.keys() {
            match values.and_then(|t| t.get(key.name)) {
                Some(toml::Value::Array(items))
                    if !items.is_empty() && items.iter().all(toml::Value::is_table) =>
                {
                    blocks.push((key, items));
                }
                Some(value) => {
                    let _ = writeln!(text, "{} = {}", key.name, redacted(key, value));
                }
                None => match (key.default_value(), key.rule) {
                    (Some(value), _) => {
                        let _ = writeln!(text, "{} = {value}  # not set: the default", key.name);
                    }
                    (None, rule) => {
                        let rule = rule.unwrap_or_default();
                        let _ = writeln!(text, "# {}: not set; {rule}", key.name);
                    }
                },
            }
        }
        let here = match part.key() {
            None => at.clone(),
            Some(sub) => format!("{at}.{sub}"),
        };
        for (key, items) in blocks {
            for item in items {
                let _ = writeln!(text, "\n[[{here}.{}]]", key.name);
                let item = redacted(key, &toml::Value::Array(vec![item.clone()]));
                let Some(fields) = item.as_array().and_then(|i| i.first()?.as_table().cloned())
                else {
                    continue;
                };
                for (field, value) in fields {
                    let _ = writeln!(text, "{} = {value}", toml_key(&field));
                }
            }
        }
    }
    out!("{text}");
    Ok(())
}

/// `part`'s table in a profile's `table`: the profile's own for its shared
/// part. `None` where the profile has none.
fn part_of(table: &toml::Table, part: Part) -> Option<&toml::Table> {
    match part.key() {
        None => Some(table),
        Some(at) => table.get(at).and_then(toml::Value::as_table),
    }
}

/// `key`'s path in a profile, as `unset` has it: `log`, `headless.cwd`.
fn path_of(part: Part, key: &Key) -> String {
    match part.key() {
        None => key.name.to_owned(),
        Some(at) => format!("{at}.{}", key.name),
    }
}

/// `value` of `key`, the MCP servers' `env` and `headers` values
/// `<redacted>`, their names kept (ADR 9.1).
fn redacted(key: &Key, value: &toml::Value) -> toml::Value {
    let mut value = value.clone();
    if key.name != "mcp_servers" {
        return value;
    }
    for server in value.as_array_mut().into_iter().flatten() {
        let Some(server) = server.as_table_mut() else { continue };
        for field in ["env", "headers"] {
            if let Some(toml::Value::Table(map)) = server.get_mut(field) {
                map.iter_mut().for_each(|(_, v)| *v = toml::Value::String(REDACTED.into()));
            }
        }
    }
    value
}

/// A TOML value as JSON.
fn as_json(value: &toml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// `name` as a TOML key: bare where it can be, else quoted.
fn toml_key(name: &str) -> String {
    let bare = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    match !name.is_empty() && name.chars().all(bare) {
        true => name.to_owned(),
        false => toml::Value::String(name.into()).to_string(),
    }
}

/// `profile show --pid <pid>`: the request the process was started with,
/// from its socket (`start_request`), not the file: it is true of the
/// process whatever the file says now, and works where the file can't be
/// read (P10).
fn show_pid(pid: &str, json_out: bool) -> Result<(), error::Error> {
    let hosts = discover()?;
    let host = process(&hosts, pid)?;
    let reply = host.call(&json!({ "cmd": "start_request" }))?;
    let request = reply.get("start_request").cloned().unwrap_or(Value::Null);
    if json_out {
        let pid: Value = host.id().parse::<u64>().map_or(json!(host.id()), Value::from);
        return print_json(&json!({ "pid": pid, "start_request": request }));
    }
    let profile = match request["profile"].as_str() {
        Some(name) => format!("profile {name}"),
        None => "no profile".to_owned(),
    };
    let mut text =
        format!("# process {}, started with {profile}; fields that are null left out\n", host.id());
    if let Some(toml::Value::Table(table)) = as_toml(&request) {
        write_table(&mut text, "", &table);
    }
    out!("{text}");
    Ok(())
}

/// JSON as TOML, which has no null: a null is left out.
fn as_toml(value: &Value) -> Option<toml::Value> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64()?),
        },
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(items) => toml::Value::Array(items.iter().filter_map(as_toml).collect()),
        Value::Object(map) => toml::Value::Table(
            map.iter().filter_map(|(k, v)| Some((k.clone(), as_toml(v)?))).collect(),
        ),
    })
}

/// `table`, at `at` (empty: the top), as a TOML document: its values, then
/// its tables, then its arrays of tables.
fn write_table(text: &mut String, at: &str, table: &toml::Table) {
    let dot = |k: &str| match at {
        "" => toml_key(k),
        _ => format!("{at}.{}", toml_key(k)),
    };
    let blocks = |v: &toml::Value| {
        v.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(toml::Value::is_table))
    };
    for (k, v) in table.iter().filter(|(_, v)| !v.is_table() && !blocks(v)) {
        let _ = writeln!(text, "{} = {v}", toml_key(k));
    }
    for (k, v) in table {
        if let toml::Value::Table(sub) = v {
            let _ = writeln!(text, "\n[{}]", dot(k));
            write_table(text, &dot(k), sub);
        }
    }
    for (k, v) in table.iter().filter(|(_, v)| blocks(v)) {
        for item in v.as_array().into_iter().flatten().filter_map(toml::Value::as_table) {
            let _ = writeln!(text, "\n[[{}]]", dot(k));
            write_table(text, &dot(k), item);
        }
    }
}

/// `profile keys`: every key a profile can have, from the table the layout
/// check reads.
pub(super) fn keys(a: &Args) -> Result<(), error::Error> {
    if a.json {
        let keys: Vec<Value> = KEYS
            .iter()
            .map(|k| {
                json!({
                    "key": k.name,
                    "part": k.part.name(),
                    "type": k.kind,
                    "default": k.default_value().map_or(Value::Null, |v| as_json(&v)),
                    "rule": k.rule,
                    "names": k.names(),
                    "adr": k.adr,
                })
            })
            .collect();
        return print_json(&json!({ "keys": keys }));
    }
    let mut text = String::new();
    for part in Part::ALL {
        let _ = writeln!(text, "{}  # {}", part.table("profiles.<name>"), part.users());
        for k in part.keys() {
            let unset = match (k.default, k.rule) {
                (Some(value), _) => format!("default {value}"),
                (None, rule) => format!("not set: {}", rule.unwrap_or_default()),
            };
            let names = k.names();
            let names = match names.is_empty() {
                true => String::new(),
                false => format!("; names: {}", names.join(", ")),
            };
            let _ = writeln!(text, "  {}: {}; {unset}{names} (ADR {})", k.name, k.kind, k.adr);
        }
    }
    out!("{text}");
    Ok(())
}
