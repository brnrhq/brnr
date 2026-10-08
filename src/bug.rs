//! What brnr says for a bug report (ADR 45 in docs/adr): the OS it runs on,
//! and, on a panic, a link to a pre-filled "new issue" form. Nothing is
//! sent: the link is printed, and opening it is the user's choice.
//!
//! The link goes where the user of the panicking process looks: the CLI's
//! stderr, as Rust's own message does (see [`install`]); for a host, its
//! stderr only in the foreground (death.rs), and `brnr start`'s or `brnr
//! acp`'s when a start fails with a panic. A detached host's stderr is its
//! host log, which nobody reads as it happens.

use std::env;
use std::fs;
use std::panic::{self, PanicHookInfo};

use crate::sys;

/// The form a link opens, `.github/ISSUE_TEMPLATE/bug.yml`.
const NEW_ISSUE: &str = "https://github.com/brnrhq/brnr/issues/new?template=bug.yml";

/// How much of a panic's message goes in the link: with the rest of it, a
/// URL GitHub takes (8 KB).
const MESSAGE_CHARS: usize = 1000;

/// `brnr 0.6.0`, as `brnr --version` says it.
pub fn version() -> String {
    format!("brnr {}", env!("CARGO_PKG_VERSION"))
}

/// The OS, its version and the kernel's: `macOS 26.4 (Darwin 25.4.0,
/// arm64)`, `Ubuntu 24.04.1 LTS (Linux 6.8.0-45-generic, x86_64)`. Read
/// from files and `uname`, running nothing, so a panic hook can call it.
pub fn os() -> String {
    let (kernel, release, machine) = sys::uname()
        .unwrap_or_else(|| (env::consts::OS.into(), "?".into(), env::consts::ARCH.into()));
    let kernel = format!("{kernel} {release}, {machine}");
    match product() {
        Some(name) => format!("{name} ({kernel})"),
        None => kernel,
    }
}

/// The OS's own name and version: macOS's SystemVersion.plist, Linux's
/// os-release.
fn product() -> Option<String> {
    if cfg!(target_vendor = "apple") {
        let plist = fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
        let value = |key: &str| {
            let after = &plist[plist.find(&format!("<key>{key}</key>"))?..];
            let start = after.find("<string>")? + "<string>".len();
            let end = after[start..].find("</string>")?;
            Some(after[start..start + end].trim().to_owned())
        };
        return Some(format!("{} {}", value("ProductName")?, value("ProductVersion")?));
    }
    let text = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    Some(line.trim_matches(['"', '\'']).to_owned()).filter(|n| !n.is_empty())
}

/// `brnr panicked at src/host/acp.rs:120:5: <message>`.
pub fn describe(info: &PanicHookInfo) -> String {
    let payload = info.payload();
    let message = (payload.downcast_ref::<&str>().copied())
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("?");
    match info.location() {
        Some(at) => {
            format!("brnr panicked at {}:{}:{}: {message}", at.file(), at.line(), at.column())
        }
        None => format!("brnr panicked: {message}"),
    }
}

/// Whether an error is a panic's, as [`describe`] puts it.
pub fn is_panic(error: &str) -> bool {
    error.starts_with("brnr panicked")
}

/// The pre-filled form for a panic, `panic` as [`describe`] says it, in
/// `brnr <command>`: the title is where it panicked, and the version, the OS
/// and the message are filled in.
pub fn issue_url(panic: &str, command: &str) -> String {
    let short: String = panic.chars().take(MESSAGE_CHARS).collect();
    let cut = if short.len() < panic.len() { " …" } else { "" };
    let at = panic.strip_prefix("brnr panicked at ").and_then(|rest| rest.split(": ").next());
    let title = match at {
        Some(at) => format!("Panic at {at}"),
        None => "Panic".to_owned(),
    };
    let what = format!(
        "`brnr {command}` panicked:\n\n```\n{short}{cut}\n```\n\nWhat I ran, and what I expected:\n"
    );
    let fields = [("title", title), ("version", version()), ("setup", os()), ("what", what)];
    let mut url = NEW_ISSUE.to_owned();
    for (key, value) in fields {
        url.push_str(&format!("&{key}={}", encode(&value)));
    }
    url
}

/// What brnr prints after a panic's message: one line, then the link.
pub fn link(panic: &str, command: &str) -> String {
    format!(
        "brnr: a bug in brnr. This link opens a report of it, filled in \
         (nothing is sent until you submit it):\n{}",
        issue_url(panic, command)
    )
}

/// For `brnr <command>` but `host`: a panic prints the link on stderr after
/// Rust's message. For the tests, `BRNR_TEST_PANIC=cli` panics here.
pub fn install(command: &str) {
    let command = command.to_owned();
    let shown = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        shown(info);
        eprintln!("{}", link(&describe(info), &command));
    }));
    if env::var_os("BRNR_TEST_PANIC").is_some_and(|v| v == "cli") {
        panic!("a test asked for it");
    }
}

/// Percent-encoding for a query's value: all but RFC 3986's unreserved
/// characters, byte by byte.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_leaves_only_unreserved_characters() {
        assert_eq!(encode("a b&c=d/é~"), "a%20b%26c%3Dd%2F%C3%A9~");
    }

    #[test]
    fn the_link_is_prefilled() {
        let url = issue_url("brnr panicked at src/ctl.rs:1:2: it broke: badly", "list");
        assert!(url.starts_with(&format!("{NEW_ISSUE}&title=Panic%20at%20src%2Fctl.rs%3A1%3A2&")));
        assert!(url.contains(&format!("&version=brnr%20{}&", env!("CARGO_PKG_VERSION"))));
        assert!(url.contains("&setup="), "{url}");
        assert!(url.contains("%60brnr%20list%60%20panicked"), "{url}");
        assert!(url.contains("it%20broke%3A%20badly"), "{url}");
        // A long message is cut, and says so.
        let long = issue_url(&format!("brnr panicked: {}", "x".repeat(5000)), "list");
        assert!(long.len() < 4000 && long.contains("%20%E2%80%A6"), "{}", long.len());
    }
}
