//! Reading ACP lines as RFC 8259 has them (ADR 26 in docs/adr).
//!
//! serde_json refuses two kinds of text RFC 8259's grammar allows: a string
//! with a lone surrogate escape (`"\ud83d"`, what `JSON.stringify` makes of a
//! title cut in the middle of an emoji), and nesting deeper than 128.
//! [`parse`] reads both. In the copy it returns, a lone surrogate is U+FFFD
//! and there is no depth limit; the line itself is forwarded as it came.
//!
//! A deep value takes a deep stack, to parse and also to clone, show or drop
//! afterwards. [`depth`] says how deep a line goes, and [`on_stack`] runs
//! code on a stack that takes values that deep.

use std::panic;
use std::thread;

use serde::de::DeserializeOwned;

/// Nesting any thread's stack takes: serde_json's own limit.
pub const SHALLOW: usize = 128;

/// Stack per level of nesting. The deepest use is parsing an object: about
/// 1 KiB a level, 3 KiB in a debug build.
const FRAME: usize = if cfg!(debug_assertions) { 8 << 10 } else { 4 << 10 };

/// Stack for everything else a thread does: a spawned thread's default.
const BASE: usize = 2 << 20;

/// `text` as brnr interprets it, if it is JSON at all. Run it on a stack
/// that takes [`depth`]`(text)`.
pub fn parse<T: DeserializeOwned>(text: &[u8]) -> Option<T> {
    if let Ok(value) = serde_json::from_slice(text) {
        return Some(value);
    }
    let mut fixed = None;
    scan(text, |at| {
        fixed.get_or_insert_with(|| text.to_vec())[at..at + 6].copy_from_slice(br"\uFFFD");
    });
    let mut de = serde_json::Deserializer::from_slice(fixed.as_deref().unwrap_or(text));
    de.disable_recursion_limit();
    let value = T::deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(value)
}

/// How deeply `text` nests: as deep as a parse of it can go.
pub fn depth(text: &[u8]) -> usize {
    scan(text, |_| {})
}

/// Runs `f` on a stack that takes values nested `depth` deep: this thread's,
/// up to [`SHALLOW`], else a thread's of its own. `None` if there is no such
/// stack to be had.
pub fn on_stack<R: Send>(depth: usize, f: impl FnOnce() -> R + Send) -> Option<R> {
    if depth <= SHALLOW {
        return Some(f());
    }
    let size = depth.checked_mul(FRAME)?.checked_add(BASE)?;
    thread::scope(|scope| {
        let thread = thread::Builder::new().stack_size(size).spawn_scoped(scope, f).ok()?;
        Some(thread.join().unwrap_or_else(|e| panic::resume_unwind(e)))
    })
}

/// The deepest nesting in `text`, calling `lone` with where each lone
/// surrogate escape starts: `\uD800` to `\uDBFF` not followed by a low
/// surrogate escape, or `\uDC00` to `\uDFFF` not preceded by a high one.
/// Text that isn't JSON is looked through as far as it can be.
fn scan(text: &[u8], mut lone: impl FnMut(usize)) -> usize {
    let (mut depth, mut deepest) = (0_usize, 0);
    let mut in_string = false;
    let mut i = 0;
    while i < text.len() {
        match (in_string, text[i]) {
            (false, b'[' | b'{') => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            (false, b']' | b'}') => depth = depth.saturating_sub(1),
            (_, b'"') => in_string = !in_string,
            (true, b'\\') => {
                let first = unit(text, i);
                if first.is_some_and(is_high) && unit(text, i + 6).is_some_and(is_low) {
                    i += 12;
                    continue;
                }
                if first.is_some_and(|u| is_high(u) || is_low(u)) {
                    lone(i);
                }
                // Past the escaped byte (a `"` doesn't end the string); the
                // hex digits of a `\u` escape are nothing to look at.
                i += 2;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    deepest
}

/// The UTF-16 code unit of the `\uXXXX` escape at `at`, if there is one.
fn unit(text: &[u8], at: usize) -> Option<u16> {
    let hex = text.get(at..at + 6)?.strip_prefix(br"\u")?;
    let hex = std::str::from_utf8(hex).ok().filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    u16::from_str_radix(hex, 16).ok()
}

fn is_high(unit: u16) -> bool {
    (0xD800..0xDC00).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn read(text: &str) -> Option<Value> {
        parse(text.as_bytes())
    }

    #[test]
    fn lone_surrogates_become_replacement_characters() {
        assert_eq!(read(r#""cut \ud83d""#), Some(json!("cut \u{fffd}")));
        assert_eq!(read(r#""\ude00 trails""#), Some(json!("\u{fffd} trails")));
        assert_eq!(read(r#"{"\uD83D":"\uD83D\uDE00"}"#), Some(json!({ "\u{fffd}": "\u{1f600}" })));
        assert_eq!(read(r#""\ud83d\ud83d\ude00\ude00""#), Some(json!("\u{fffd}\u{1f600}\u{fffd}")));
        assert_eq!(read(r#""\ud83d\n""#), Some(json!("\u{fffd}\n")));
    }

    #[test]
    fn only_escapes_are_looked_at() {
        // An escaped backslash, then text that looks like an escape.
        assert_eq!(read(r#""\\ud83d""#), Some(json!(r"\ud83d")));
        assert_eq!(read(r#"["\"[", "\ud83d"]"#), Some(json!(["\"[", "\u{fffd}"])));
        assert_eq!(depth(br#"[{"a": "[[[{{"}, "\"]]"]"#), 2);
    }

    #[test]
    fn what_isnt_json_stays_unread() {
        assert_eq!(read(r#"{"id": 1"#), None);
        assert_eq!(read(r#""\ud83d" trailing"#), None);
        assert_eq!(read(r#""\uZZZZ""#), None);
        assert_eq!(read(""), None);
    }

    #[test]
    fn nesting_has_no_limit() {
        let n = 20_000;
        let text = format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert_eq!(depth(text.as_bytes()), n);
        let back = on_stack(n, || {
            let value: Value = parse(text.as_bytes()).unwrap();
            value.to_string()
        });
        assert_eq!(back.as_deref(), Some(text.as_str()));
    }
}
