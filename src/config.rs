//! Profiles from `config.toml` (see paths.rs for where it lives).
//!
//! ```toml
//! [profiles.slack]
//! agent = ["claude-agent-acp"]       # used when no agent is given after --
//! cwd = "~/work/project"             # sessions started headless (brnr start)
//! on_disconnect = "headless"         # direct | headless
//! permissions = "ask"                # ask | auto-allow | auto-deny, with no editor attached
//! log = true                         # transcripts under ~/.brnr
//!
//! [[profiles.slack.bridges]]
//! command = ["~/bin/slack-bridge", "--channel", "#agents"]
//! events = ["permission_request", "turn_ended"]   # omit for every event
//! ```
//!
//! Without `--profile`, the `default` profile applies if there is one.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;

use serde::Deserialize;

use crate::paths;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub agent: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub on_disconnect: Option<String>,
    pub permissions: Option<String>,
    pub log: Option<bool>,
    #[serde(default)]
    pub bridges: Vec<Bridge>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    pub command: Vec<String>,
    pub events: Option<Vec<String>>,
}

/// The named profile, or `default` (or nothing) when no name is given.
pub fn load(name: Option<&str>) -> Result<Profile, String> {
    let path = paths::config_file();
    let profiles = match load_all() {
        Ok(Some(profiles)) => profiles,
        Ok(None) if name.is_none() => return Ok(Profile::default()),
        Ok(None) => return Err(format!("{}: no such file", path.display())),
        Err(err) => return Err(err),
    };
    match name {
        Some(name) => profiles
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no profile {name:?} in {}", path.display())),
        None => Ok(profiles.get("default").cloned().unwrap_or_default()),
    }
}

/// Every profile, or `None` if there is no config file.
pub fn load_all() -> Result<Option<BTreeMap<String, Profile>>, String> {
    let path = paths::config_file();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let file: ConfigFile =
        toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message()))?;
    Ok(Some(file.profiles))
}
