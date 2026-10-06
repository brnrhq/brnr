//! Events as text, the one way brnr shows a session: `brnr watch`, `brnr
//! log` and `brnr host` in the foreground all use it. Also a tool call in
//! full (input, paths, diffs), for `brnr show`.

use serde_json::Value;

/// Edits bigger than this (old lines × new lines) aren't diffed; the new
/// text is shown instead.
const DIFF_LIMIT: usize = 4_000_000;
/// Unchanged lines around each change in a diff.
const CONTEXT: usize = 3;

pub struct Options {
    /// A column with the session id (for hosts with several sessions).
    pub session: bool,
    /// Start with the event's time.
    pub time: bool,
}

impl Options {
    pub fn foreground() -> Options {
        Options { session: false, time: true }
    }
}

/// One event as `HH:MM:SS  [session]  what happened`, continuation lines
/// indented under the description; `None` for events not worth a line.
pub fn event(e: &Value, o: &Options) -> Option<String> {
    let s = |v: &Value| v.as_str().unwrap_or("?").to_owned();
    let what = match e["event"].as_str()? {
        "user_message" => match e["by"].as_str() {
            Some("editor") => format!("user (editor): {}", s(&e["text"])),
            _ => format!("user: {}", s(&e["text"])),
        },
        "agent_message" => format!("agent: {}", s(&e["text"])),
        "agent_thought" => format!("thinking: {}", s(&e["text"])),
        // A tool call's start and its end; progress in between isn't shown.
        "tool_call" => {
            let title = s(&e["title"]);
            match e["status"].as_str() {
                Some("completed") => format!("tool done: {title}"),
                Some("failed") => format!("tool failed: {title}"),
                _ if e["started"] == true => format!("tool: {title} ({})", s(&e["kind"])),
                _ => return None,
            }
        }
        "plan" => plan(&e["entries"]),
        "session_changed" => match e["what"].as_str() {
            Some("title") => format!("title: {}", s(&e["value"])),
            Some("mode") => format!("mode: {}", s(&e["value"])),
            _ => return None,
        },
        "permission_request" => {
            let options: Vec<String> =
                e["options"].as_array().into_iter().flatten().map(|o| s(&o["optionId"])).collect();
            format!(
                "permission {} ({} answers): {} [{}]",
                s(&e["request"]),
                s(&e["owner"]),
                s(&e["title"]),
                options.join(" ")
            )
        }
        "permission_resolved" => {
            let outcome = &e["outcome"];
            let chosen =
                outcome["optionId"].as_str().or(outcome["outcome"].as_str()).unwrap_or("?");
            format!("permission {} -> {chosen} (by {})", s(&e["request"]), s(&e["by"]))
        }
        "turn_ended" => match e["error"].as_object() {
            Some(error) => format!("turn failed: {} ({})", error["message"], s(&e["by"])),
            None => format!("turn ended: {} ({})", s(&e["stop_reason"]), s(&e["by"])),
        },
        "owner_changed" => format!("owner -> {} ({})", s(&e["owner"]), s(&e["reason"])),
        "exited" => {
            let mut what = format!("agent exited: {}", e["status"]);
            for held in e["undelivered"].as_array().into_iter().flatten() {
                what.push_str(&format!("\nnot delivered: {}", s(&held["text"])));
            }
            what
        }
        // An ACP message (`--events acp`): the direction and the message.
        "acp" => format!("{:<15} {}", s(&e["dir"]), e["msg"]),
        _ => return None,
    };
    let mut prefix = String::new();
    if o.time {
        prefix.push_str(e["ts"].as_str().and_then(|t| t.get(11..19)).unwrap_or("        "));
        prefix.push_str("  ");
    }
    if o.session {
        let session: String = e["session"].as_str().unwrap_or("-").chars().take(8).collect();
        prefix.push_str(&format!("{session:<8}  "));
    }
    let indent = " ".repeat(prefix.len());
    Some(format!("{prefix}{}", what.trim_end().replace('\n', &format!("\n{indent}"))))
}

/// `plan (2/5):` and one line per entry: `[x]` done, `[>]` in progress.
pub fn plan(entries: &Value) -> String {
    let entries = entries.as_array().map_or(&[][..], Vec::as_slice);
    let done = entries.iter().filter(|e| e["status"] == "completed").count();
    let mut out = format!("plan ({done}/{}):", entries.len());
    for entry in entries {
        let mark = match entry["status"].as_str() {
            Some("completed") => "[x]",
            Some("in_progress") => "[>]",
            _ => "[ ]",
        };
        out.push_str(&format!("\n  {mark} {}", entry["content"].as_str().unwrap_or("?")));
    }
    out
}

/// A tool call in full: title, kind, paths, input and content (diffs for
/// edits).
pub fn tool_call(tool: &Value) -> String {
    let mut out = format!("{}\n", tool["title"].as_str().unwrap_or("(untitled tool call)"));
    out.push_str(&format!("kind: {}\n", tool["kind"].as_str().unwrap_or("other")));
    for location in tool["locations"].as_array().into_iter().flatten() {
        let path = location["path"].as_str().unwrap_or("?");
        match location["line"].as_u64() {
            Some(line) => out.push_str(&format!("path: {path}:{line}\n")),
            None => out.push_str(&format!("path: {path}\n")),
        }
    }
    let input = &tool["rawInput"];
    if let Some(command) = input["command"].as_str() {
        out.push_str(&format!("command: {command}\n"));
        if let Some(description) = input["description"].as_str() {
            out.push_str(&format!("why: {description}\n"));
        }
    } else if !input.is_null() {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
        out.push_str(&format!("input:\n{}\n", indent(&pretty, "  ")));
    }
    for content in tool["content"].as_array().into_iter().flatten() {
        match content["type"].as_str() {
            Some("diff") => {
                let path = content["path"].as_str().unwrap_or("?");
                let old = content["oldText"].as_str();
                let new = content["newText"].as_str().unwrap_or_default();
                out.push_str(&format!(
                    "--- {}\n+++ {path}\n",
                    if old.is_some() { path } else { "/dev/null" }
                ));
                out.push_str(&diff(old.unwrap_or_default(), new));
            }
            Some("content") => {
                if let Some(text) = content["content"]["text"].as_str() {
                    out.push_str(&format!("{}\n", text.trim_end()));
                }
            }
            Some("terminal") => {
                out.push_str(&format!(
                    "terminal {}\n",
                    content["terminalId"].as_str().unwrap_or("?")
                ));
            }
            _ => {}
        }
    }
    out
}

fn indent(text: &str, by: &str) -> String {
    text.lines().map(|l| format!("{by}{l}")).collect::<Vec<_>>().join("\n")
}

enum Op {
    Same(usize),
    Del(usize),
    Ins(usize),
}

/// A unified diff of `old` → `new`, hunks only (no file headers).
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len().saturating_mul(b.len()) > DIFF_LIMIT {
        let lines: Vec<String> = b.iter().map(|l| format!("+{l}")).collect();
        return format!("(too large to diff; the new text)\n{}\n", lines.join("\n"));
    }
    // Longest common subsequence, from the end.
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] = if a[i] == b[j] {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push(Op::Same(i));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[at(i + 1, j)] >= lcs[at(i, j + 1)]) {
            // Deletions before insertions, as diff(1) shows them.
            ops.push(Op::Del(i));
            i += 1;
        } else {
            ops.push(Op::Ins(j));
            j += 1;
        }
    }
    // Hunks: changes with up to CONTEXT unchanged lines around them.
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| !matches!(ops[k], Op::Same(_))).collect();
    let mut out = String::new();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CONTEXT);
        let mut end = changed[k];
        while k < changed.len() && changed[k] <= end + 2 * CONTEXT {
            end = changed[k];
            k += 1;
        }
        let end = (end + CONTEXT + 1).min(ops.len());
        let (mut old_at, mut new_at) = (None, None);
        let (mut old_len, mut new_len) = (0, 0);
        let mut body = String::new();
        for op in &ops[start..end] {
            match *op {
                Op::Same(x) => {
                    old_at.get_or_insert(x);
                    new_at.get_or_insert(position_in_new(&ops[..], x));
                    old_len += 1;
                    new_len += 1;
                    body.push_str(&format!(" {}\n", a[x]));
                }
                Op::Del(x) => {
                    old_at.get_or_insert(x);
                    old_len += 1;
                    body.push_str(&format!("-{}\n", a[x]));
                }
                Op::Ins(y) => {
                    new_at.get_or_insert(y);
                    new_len += 1;
                    body.push_str(&format!("+{}\n", b[y]));
                }
            }
        }
        let old_line = old_at.map_or(0, |x| x + 1);
        let new_line = new_at.map_or(0, |y| y + 1);
        out.push_str(&format!("@@ -{old_line},{old_len} +{new_line},{new_len} @@\n{body}"));
    }
    out
}

/// The line in the new text that old line `x` (unchanged) became.
fn position_in_new(ops: &[Op], x: usize) -> usize {
    let mut new = 0;
    for op in ops {
        match *op {
            Op::Same(o) if o == x => return new,
            Op::Same(_) | Op::Ins(_) => new += 1,
            Op::Del(_) => {}
        }
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_shows_changes_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\n";
        assert_eq!(diff(old, new), "@@ -1,7 +1,7 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n");
    }

    #[test]
    fn diff_of_new_file() {
        assert_eq!(diff("", "x\ny\n"), "@@ -0,0 +1,2 @@\n+x\n+y\n");
    }

    #[test]
    fn distant_changes_are_separate_hunks() {
        let line = |i: usize| match i {
            2 => "two".to_owned(),
            17 => "seventeen".to_owned(),
            i => i.to_string(),
        };
        let old: String = (0..20).map(|i| format!("{i}\n")).collect();
        let new: String = (0..20).map(|i| format!("{}\n", line(i))).collect();
        let d = diff(&old, &new);
        assert_eq!(d.matches("@@ -").count(), 2, "{d}");
        assert!(d.contains("-17\n+seventeen\n"), "{d}");
    }

    #[test]
    fn plan_marks_entries() {
        let entries = serde_json::json!([
            { "content": "read", "status": "completed" },
            { "content": "fix", "status": "in_progress" },
            { "content": "test", "status": "pending" },
        ]);
        assert_eq!(plan(&entries), "plan (1/3):\n  [x] read\n  [>] fix\n  [ ] test");
    }
}
