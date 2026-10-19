//! Bytes into lines, with one rule for a line too long to keep (ADR 10.3 in
//! docs/adr): the editor's input, the agent's stdout and stderr, and a
//! started bridge's and the process's own stderr are all read with it.
//!
//! A line of up to the reader's limit, its newline included, is read whole.
//! A longer one isn't kept whole, so a reader holds at most the limit of a
//! line still coming: from where it passes the limit it goes on in parts, as
//! the reader's [`Long`] says. The first part says it is the first, where
//! the line passed the limit (`line_too_long`, the start of a stderr line in
//! a failed start's tail). The reader does no I/O: it is given what each
//! read returned, and returns what that made.

use std::mem::{replace, take};

/// What a reader does with a line past its limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Long {
    /// It goes on as it comes, byte for byte: a part from each read, until
    /// its newline. The agent's stdout and the editor's input, which the host
    /// relays unread or drops (ADR 2.5).
    Relay,
    /// It is cut into parts of the limit, for the host log: a character a
    /// cut would split starts the next part, and the newline of a line that
    /// filled its last part only ends it, so a line of exactly the limit is
    /// one part, not one and an empty one. Stderr (ADR 2.6, ADR 3.6).
    Cut,
}

/// What a reader made of the bytes it was given.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// A whole line, with its newline unless the input ended first.
    Line(Vec<u8>),
    /// A part of a line past the limit; `first` for the one it passed it in.
    Part { bytes: Vec<u8>, first: bool },
}

impl Piece {
    /// Its bytes, and whether its line starts with them.
    pub(super) fn into_bytes(self) -> (Vec<u8>, bool) {
        match self {
            Piece::Line(line) => (line, true),
            Piece::Part { bytes, first } => (bytes, first),
        }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Piece::Line(bytes) | Piece::Part { bytes, .. } => bytes.len(),
        }
    }
}

/// One stream's bytes, as lines.
pub(super) struct Lines {
    limit: usize,
    long: Long,
    /// The line still coming; cut, it may be the rest of one past the limit.
    line: Vec<u8>,
    /// Inside a line past the limit: its first part has gone.
    past: bool,
}

impl Lines {
    pub(super) fn new(limit: usize, long: Long) -> Lines {
        // A cut keeps up to 3 bytes of a character for the next part: a part
        // of 4 has at least one.
        assert!(limit > 3, "a limit of {limit} bytes");
        Lines { limit, long, line: Vec::new(), past: false }
    }

    /// The lines and parts the bytes a read returned end or fill, in order.
    pub(super) fn push(&mut self, mut bytes: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let most = match self.long {
                Long::Relay => bytes.len(),
                Long::Cut => bytes.len().min(self.limit - self.line.len()),
            };
            // Only the new bytes are looked through: a long line comes in
            // many reads.
            let n = bytes[..most].iter().position(|&b| b == b'\n').map_or(most, |i| i + 1);
            let (now, rest) = bytes.split_at(n);
            bytes = rest;
            let ends = now.ends_with(b"\n");
            if self.past && self.line.is_empty() {
                match self.long {
                    Long::Relay => {
                        out.push(Piece::Part { bytes: now.to_vec(), first: false });
                        self.past = !ends;
                        continue;
                    }
                    Long::Cut if now == b"\n" => {
                        self.past = false;
                        continue;
                    }
                    Long::Cut => {}
                }
            }
            self.line.extend_from_slice(now);
            let full = match self.long {
                Long::Relay => self.line.len() > self.limit,
                Long::Cut => self.line.len() == self.limit && !ends,
            };
            if full {
                let next = match self.long {
                    Long::Relay => Vec::new(),
                    Long::Cut => self.line.split_off(self.limit - unfinished(&self.line)),
                };
                let bytes = replace(&mut self.line, next);
                out.push(Piece::Part { bytes, first: !self.past });
                self.past = !ends;
            } else if ends {
                out.push(self.take());
            }
        }
        out
    }

    /// At the end of the input: what is left of the line still coming. The
    /// reader is empty after.
    pub(super) fn finish(&mut self) -> Option<Piece> {
        let rest = self.take();
        (rest.len() > 0).then_some(rest)
    }

    /// The start of the line still coming, if none of it has gone as a part.
    pub(super) fn partial(&self) -> Option<&[u8]> {
        (!self.past && !self.line.is_empty()).then_some(&self.line)
    }

    fn take(&mut self) -> Piece {
        let bytes = take(&mut self.line);
        if take(&mut self.past) { Piece::Part { bytes, first: false } } else { Piece::Line(bytes) }
    }
}

/// How many bytes at the end of `bytes` start a UTF-8 character they don't
/// finish: at most 3.
pub(super) fn unfinished(bytes: &[u8]) -> usize {
    let tail = &bytes[bytes.len().saturating_sub(3)..];
    let starts = |i: &usize| {
        std::str::from_utf8(&tail[*i..])
            .is_err_and(|e| e.valid_up_to() == 0 && e.error_len().is_none())
    };
    (0..tail.len()).find(starts).map_or(0, |i| tail.len() - i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(bytes: &[u8]) -> Piece {
        Piece::Line(bytes.to_vec())
    }

    fn part(bytes: &[u8], first: bool) -> Piece {
        Piece::Part { bytes: bytes.to_vec(), first }
    }

    /// What a reader makes of `reads`, then of the end of them.
    fn read(limit: usize, long: Long, reads: &[&[u8]]) -> Vec<Piece> {
        let mut lines = Lines::new(limit, long);
        let mut out: Vec<Piece> = reads.iter().flat_map(|r| lines.push(r)).collect();
        out.extend(lines.finish());
        out
    }

    #[test]
    fn adr_0010_lines_are_read_whole_however_the_reads_split_them() {
        for long in [Long::Relay, Long::Cut] {
            let want = [line(b"ab\n"), line(b"\n"), line(b"cd\n"), line(b"e")];
            assert_eq!(read(8, long, &[b"ab\n\ncd\ne"]), want);
            assert_eq!(read(8, long, &[b"a", b"b\n", b"\nc", b"d\ne"]), want);
            assert_eq!(read(8, long, &[]), []);
        }
    }

    #[test]
    fn adr_0010_a_line_that_ends_at_the_limit_is_read_whole() {
        for long in [Long::Relay, Long::Cut] {
            assert_eq!(read(4, long, &[b"abc\nd"]), [line(b"abc\n"), line(b"d")], "{long:?}");
        }
        // Relayed, a line of the limit without its newline is held, as the
        // newline would make it too long.
        let mut lines = Lines::new(4, Long::Relay);
        assert_eq!(lines.push(b"abcd"), []);
        assert_eq!(lines.partial(), Some(&b"abcd"[..]));
        assert_eq!(lines.push(b"\n"), [part(b"abcd\n", true)]);
        // Cut, it fills its part: the newline after it ends it (see below).
        let mut lines = Lines::new(4, Long::Cut);
        assert_eq!(lines.push(b"abcd"), [part(b"abcd", true)]);
        assert_eq!(lines.partial(), None);
    }

    #[test]
    fn adr_0010_a_relayed_line_past_the_limit_goes_on_as_it_comes() {
        // Its newline too: the line goes on byte for byte.
        assert_eq!(read(4, Long::Relay, &[b"abcd\nx\n"]), [part(b"abcd\n", true), line(b"x\n")]);
        let want = [
            part(b"abcde", true),
            part(b"fghijk", false),
            part(b"l\n", false),
            line(b"m\n"),
            line(b"n"),
        ];
        assert_eq!(read(4, Long::Relay, &[b"abc", b"de", b"fghijk", b"l\nm\nn"]), want);
        // Nothing of it is held: the input ends with nothing left.
        let mut lines = Lines::new(4, Long::Relay);
        assert_eq!(lines.push(b"abcdefgh"), [part(b"abcdefgh", true)]);
        assert_eq!(lines.partial(), None);
        assert_eq!(lines.finish(), None);
    }

    #[test]
    fn adr_0010_a_cut_line_is_in_parts_of_the_limit() {
        let want = [part(b"abcd", true), part(b"efgh", false), part(b"ij\n", false), line(b"k\n")];
        assert_eq!(read(4, Long::Cut, &[b"abcdefghij\nk\n"]), want);
        assert_eq!(read(4, Long::Cut, &[b"ab", b"cdefg", b"hij\n", b"k\n"]), want);
        let mut lines = Lines::new(4, Long::Cut);
        lines.push(b"abcdef");
        assert_eq!(lines.partial(), None, "its start has gone");
        assert_eq!(lines.finish(), Some(part(b"ef", false)));
        assert_eq!(lines.partial(), None);
    }

    #[test]
    fn adr_0010_the_newline_after_a_cut_only_ends_its_line() {
        // A line of exactly the limit is one part, not one and an empty one.
        assert_eq!(read(4, Long::Cut, &[b"abcd\n\n"]), [part(b"abcd", true), line(b"\n")]);
        assert_eq!(read(4, Long::Cut, &[b"abcd", b"\n", b"e"]), [part(b"abcd", true), line(b"e")]);
        assert_eq!(
            read(4, Long::Cut, &[b"abcdefgh\n"]),
            [part(b"abcd", true), part(b"efgh", false)]
        );
    }

    #[test]
    fn adr_0010_a_character_a_cut_would_split_starts_the_next_part() {
        let euro = "\u{20ac}".as_bytes(); // 3 bytes
        let bytes = [b"ab", euro, b"\n"].concat();
        let want = [part(b"ab", true), part(&[euro, b"\n"].concat(), false)];
        assert_eq!(read(4, Long::Cut, &[&bytes]), want);
        assert_eq!(read(4, Long::Cut, &[&bytes[..3], &bytes[3..4], &bytes[4..]]), want);
        // The input ends in the middle of it: what came is the last part.
        assert_eq!(
            read(4, Long::Cut, &[&bytes[..4]]),
            [part(b"ab", true), part(&euro[..2], false)]
        );
        // Bytes that start no character are cut where the limit is.
        let want = [part(b"\xff\xff\xff\xff", true), part(b"\xff\n", false)];
        assert_eq!(read(4, Long::Cut, &[b"\xff\xff\xff\xff\xff\n"]), want);
        // A relayed line is never cut.
        assert_eq!(read(4, Long::Relay, &[&bytes]), [part(&bytes, true)]);
    }

    #[test]
    fn adr_0010_a_cut_is_between_characters() {
        assert_eq!(unfinished("\u{e9}".as_bytes()), 0);
        assert_eq!(
            (unfinished(b"x\xc3"), unfinished(b"\xe2\x82"), unfinished(b"\xf0\x9f\x98")),
            (1, 2, 3)
        );
        assert_eq!(unfinished(b"\x82\x82\x82"), 0, "no character starts there");
        assert_eq!(unfinished(b""), 0);
    }

    /// However the reads split the input, a reader makes the same of it.
    #[test]
    fn adr_0010_a_byte_at_a_time_reads_as_all_at_once() {
        let input = "a\u{20ac}\u{20ac}\nbcdefgh\n\n\u{1f600}xyz\n\nij".as_bytes();
        for long in [Long::Relay, Long::Cut] {
            let whole = read(5, long, &[input]);
            let bytes: Vec<&[u8]> = input.chunks(1).collect();
            let parts = read(5, long, &bytes);
            // Relayed parts are as the reads came: compare what they hold.
            let flat = |pieces: Vec<Piece>| -> Vec<(Vec<u8>, bool)> {
                let mut out: Vec<(Vec<u8>, bool)> = Vec::new();
                for (bytes, first) in pieces.into_iter().map(Piece::into_bytes) {
                    match out.last_mut() {
                        Some(last) if !first && long == Long::Relay => last.0.extend(bytes),
                        _ => out.push((bytes, first)),
                    }
                }
                out
            };
            assert_eq!(flat(parts), flat(whole), "{long:?}");
        }
    }
}
