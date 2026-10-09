//! The host fed ACP from both sides, without its processes (see
//! src/host/fuzz.rs).
//!
//! The input is lines. The first says which host: with `h` in it a headless
//! one, else an editor's; `s` strict mode, `S` shared sessions, `r` a
//! headless start that resumes `sess-1`. Each line after it is a step, by its
//! first byte:
//!
//! - `E`: the rest is a line from the editor; `e`: bytes from the editor
//!   with no newline after them (the line goes on in the next step);
//! - `A`: the rest is a line from the agent; `a`: the agent's last bytes,
//!   with no newline;
//! - `T`: time passes (permission requests and idle sessions time out);
//! - `Z`: the editor closes its stdin, and sends nothing more.
//!
//! A headless host has no editor: `E`, `e` and `Z` do nothing there.
//!
//! A line that isn't a JSON object goes to the other side as it came
//! (ADR 26).

#![no_main]

use std::sync::Once;

use brnr::host::fuzz::Harness;
use brnr::{frame, json};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// Whether `line` is a message the host interprets: a JSON object.
fn object(line: &[u8]) -> bool {
    let depth = json::depth(line);
    json::on_stack(depth, || matches!(json::parse::<Value>(line), Some(Value::Object(_))))
        .unwrap_or(false)
}

fuzz_target!(|data: &[u8]| {
    static DIR: Once = Once::new();
    DIR.call_once(|| {
        // Session locks go here, not where real sessions are.
        let dir = std::env::temp_dir().join(format!("brnr-fuzz-{}", std::process::id()));
        // SAFETY: done first, before the host makes any thread; nothing else
        // in the process reads or writes the environment meanwhile.
        unsafe { std::env::set_var("BRNR_DIR", dir) };
    });
    let mut lines = data.split(|&b| b == b'\n');
    let header = lines.next().unwrap_or_default();
    let has = |c: u8| header.contains(&c);
    let editor = !has(b'h');
    let mut host = if editor {
        Harness::editor(has(b's'), has(b'S'))
    } else {
        Harness::headless(has(b's'), has(b'r').then(|| "sess-1".to_owned()))
    };
    let (mut partial, mut closed) = (false, false);
    for step in lines {
        let Some((&op, rest)) = step.split_first() else { continue };
        host.sent();
        match op {
            b'E' | b'e' if editor && !closed => {
                let mut bytes = rest.to_vec();
                if op == b'E' {
                    bytes.push(b'\n');
                }
                host.editor_bytes(&bytes);
                // A whole line, not the end of one begun before.
                if op == b'E' && !partial && !object(rest) {
                    assert_eq!(host.sent().agent, [bytes]);
                }
                partial = op == b'e';
            }
            b'A' | b'a' => {
                let mut line = rest.to_vec();
                if op == b'A' {
                    line.push(b'\n');
                }
                host.agent_line(&line);
                if editor && !object(rest) {
                    assert_eq!(host.sent().editor, [(frame::DATA, line)]);
                }
            }
            b'T' => host.tick(),
            b'Z' if editor && !closed => {
                host.editor_eof();
                closed = true;
            }
            _ => {}
        }
    }
});
