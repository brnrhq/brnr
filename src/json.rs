//! Reading ACP lines as RFC 8259 has them, to a fixed depth (ADR 2.5 in
//! docs/adr).
//!
//! serde_json refuses a string with a lone surrogate escape (`"\ud83d"`,
//! what `JSON.stringify` makes of a title cut in the middle of an emoji),
//! which RFC 8259's grammar allows. [`parse`] reads it: in the copy it
//! returns, a lone surrogate is U+FFFD; the line itself is forwarded as it
//! came. It reads nesting as deep as it is told to, and no deeper: a line
//! from the agent, the editor or a peer to [`DEPTH`], one brnr wrote to
//! [`OWN_DEPTH`]. Either is a fixed number that any thread's stack takes, to
//! parse a value and to clone, show or drop it afterwards; [`depth`] says how
//! deep a line goes, so that one past it is told apart from one that isn't
//! JSON.
//!
//! [`members`] finds a top-level member in the line itself, so that a
//! request's id is changed there and the rest goes on byte for byte (ADR 2.4).

use std::ops::Range;

use serde::de::DeserializeOwned;

/// How deep a line from the agent, the editor or a peer may nest for brnr
/// to read it: 128 levels, about what serde_json reads by default (127), so
/// as deep as what reads ACP with it, the Rust SDK and editors built on it.
pub const DEPTH: usize = 128;

/// How deep a line brnr wrote may nest for brnr to read it: an event, an
/// answer of the host's, a record. Each wraps what the host read, at most
/// [`DEPTH`] deep, in a few levels of its own; twice [`DEPTH`] leaves room
/// for that many times over, and any thread's stack still takes it.
pub const OWN_DEPTH: usize = 2 * DEPTH;

/// `text` as brnr interprets it, if it is JSON at all and nests no deeper
/// than `most`.
pub fn parse<T: DeserializeOwned>(text: &[u8], most: usize) -> Option<T> {
    debug_assert!(most >= DEPTH, "{most}");
    // What serde_json reads on its own nests 127 deep at most.
    if let Ok(value) = serde_json::from_slice(text) {
        return Some(value);
    }
    let mut fixed = None;
    let deepest = scan(text, |at| {
        fixed.get_or_insert_with(|| text.to_vec())[at..at + 6].copy_from_slice(br"\uFFFD");
    });
    if deepest > most {
        return None;
    }
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

/// `Err` if `text`, `what` it is, nests deeper than `most`, saying so.
pub(crate) fn within(text: &[u8], most: usize, what: &str) -> Result<(), String> {
    match depth(text) {
        depth if depth > most => {
            Err(format!("{what} nests {depth} deep, past the {most} brnr reads"))
        }
        _ => Ok(()),
    }
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
    use std::thread;

    use serde_json::{Value, json};

    use super::*;

    fn read(text: &str) -> Option<Value> {
        parse(text.as_bytes(), DEPTH)
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
    fn adr_0002_an_id_is_found_where_it_is_and_nowhere_else() {
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

    /// Arrays nested `n` deep around `inner`.
    fn deep(n: usize, inner: &str) -> String {
        format!("{}{inner}{}", "[".repeat(n), "]".repeat(n))
    }

    #[test]
    fn adr_0002_a_line_is_read_to_its_limit_and_no_deeper() {
        for most in [DEPTH, OWN_DEPTH] {
            let at = deep(most, "1");
            assert_eq!(depth(at.as_bytes()), most);
            let value: Option<Value> = parse(at.as_bytes(), most);
            assert_eq!(value.map(|v| v.to_string()).as_deref(), Some(at.as_str()), "{most}");
            assert_eq!(parse::<Value>(deep(most + 1, "1").as_bytes(), most), None, "{most}");
            // With a lone surrogate in it, too.
            let lone = deep(most, r#""\ud83d""#);
            let read = parse::<Value>(lone.as_bytes(), most).map(|v| v.to_string());
            assert_eq!(read, Some(deep(most, "\"\u{fffd}\"")), "{most}");
        }
        assert_eq!(parse::<Value>(deep(DEPTH + 1, "1").as_bytes(), DEPTH), None);
        assert_eq!(parse::<Value>(deep(1_000_000, "1").as_bytes(), OWN_DEPTH), None);
    }

    /// Any thread's stack takes a value at the limit: one of a spawned
    /// thread's default size (2 MiB) reads it, objects and arrays, clones
    /// it, shows it (indented too) and lets go of it.
    #[test]
    fn adr_0002_any_stack_takes_a_value_at_the_limit() {
        let n = OWN_DEPTH / 2;
        let text = format!("{}1{}", r#"{"a":["#.repeat(n), "]}".repeat(n));
        assert_eq!(depth(text.as_bytes()), OWN_DEPTH);
        let read = thread::Builder::new().stack_size(2 << 20).spawn(move || {
            let value: Value = parse(text.as_bytes(), OWN_DEPTH).unwrap();
            let copy = value.clone();
            assert!(serde_json::to_string_pretty(&copy).unwrap().len() > text.len());
            drop(copy);
            assert_eq!(value.to_string(), text);
        });
        read.unwrap().join().unwrap();
    }
}
