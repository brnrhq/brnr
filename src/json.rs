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
//!
//! [`members`] finds a top-level member in the line itself, so that a
//! request's id is changed there and the rest goes on byte for byte (ADR 61).

use std::ops::Range;
use std::panic;
use std::thread;

use serde::de::DeserializeOwned;

/// Nesting any thread's stack takes: serde_json's own limit.
pub const SHALLOW: usize = 128;

/// Stack per level of nesting. The deepest use is parsing an object: about
/// 1 KiB a level, 3 KiB in a debug build. Built under a sanitizer (`--cfg
/// sanitized`), frames are bigger: 5 KiB under AddressSanitizer, whose
/// redzones pad every one.
const FRAME: usize = if cfg!(sanitized) {
    24 << 10
} else if cfg!(debug_assertions) {
    8 << 10
} else {
    4 << 10
};

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

/// Where in `text`, a JSON object, the values of its top-level members
/// named `name` are: each of them, a name given twice included, and a name
/// with escapes as a reader decodes it. What isn't an object has none.
pub fn members(text: &[u8], name: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut i = blank(text, 0);
    if text.get(i) != Some(&b'{') {
        return found;
    }
    loop {
        let key = blank(text, i + 1);
        let Some(key_end) = string_end(text, key) else { return found };
        let colon = blank(text, key_end);
        if text.get(colon) != Some(&b':') {
            return found;
        }
        let start = blank(text, colon + 1);
        let Some(end) = value_end(text, start) else { return found };
        if serde_json::from_slice::<String>(&text[key..key_end]).is_ok_and(|k| k == name) {
            found.push(start..end);
        }
        i = blank(text, end);
        if text.get(i) != Some(&b',') {
            return found;
        }
    }
}

/// `text` with each of `spans`, in order and apart (as [`members`] finds
/// them), replaced by `with`.
pub fn replace(text: &[u8], spans: &[Range<usize>], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + spans.len() * with.len());
    let mut from = 0;
    for span in spans {
        out.extend_from_slice(&text[from..span.start]);
        out.extend_from_slice(with);
        from = span.end;
    }
    out.extend_from_slice(&text[from..]);
    out
}

/// Past the whitespace at `i`.
fn blank(text: &[u8], mut i: usize) -> usize {
    while text.get(i).is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// Just past the string that starts at `i`, if one does.
fn string_end(text: &[u8], i: usize) -> Option<usize> {
    if text.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while j < text.len() {
        match text[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Just past the value that starts at `i`, if one does.
fn value_end(text: &[u8], i: usize) -> Option<usize> {
    match *text.get(i)? {
        b'"' => string_end(text, i),
        b'{' | b'[' => {
            let (mut depth, mut j) = (0_usize, i);
            while j < text.len() {
                match text[j] {
                    b'"' => {
                        j = string_end(text, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let end = (i..text.len())
                .find(|&j| matches!(text[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                .unwrap_or(text.len());
            (end > i).then_some(end)
        }
    }
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
    fn adr_0061_an_id_is_found_where_it_is_and_nowhere_else() {
        let ids = |text: &str| -> Vec<String> {
            let found = members(text.as_bytes(), "id");
            found.into_iter().map(|r| text[r].to_owned()).collect()
        };
        assert_eq!(ids(r#"{"jsonrpc":"2.0","id":1e3,"method":"m"}"#), ["1e3"]);
        assert_eq!(ids(r#" { "params" : {"id":"no"} ,"id" : "x\"y" }"#), [r#""x\"y""#]);
        // An escaped name is the name; one given twice, both.
        assert_eq!(
            ids(r#"{"id":-0.0,"a":["id",{"}":"]"}],"id":[1,{"id":2}]}"#),
            ["-0.0", r#"[1,{"id":2}]"#]
        );
        assert_eq!(ids(r#"{"idx":1,"i\ud83dd":2}"#), Vec::<String>::new());
        assert_eq!(ids(r#"["id", 1]"#), Vec::<String>::new());
        assert_eq!(ids("{}"), Vec::<String>::new());
        let text = br#"{"id":1, "id" :2,"x":3}"#;
        let found = members(text, "id");
        assert_eq!(replace(text, &found, br#""w""#), br#"{"id":"w", "id" :"w","x":3}"#);
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
