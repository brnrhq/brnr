//! Framing between the proxy and the host: `[kind: u8][len: u32 BE][payload]`.
//!
//! They are joined by two socketpairs. The link carries ACP bytes in `DATA`
//! frames exactly as read, both ways, and what has its place among them: the
//! proxy's stdin ending (`EOF`), which comes after what the editor wrote
//! before it, as it does on a pipe; and everything else the host sends, what
//! plain stdio carries out of band (stderr, the exit status).
//!
//! The signal link, proxy → host only, carries what has no place among the
//! editor's input: a signal (`SIGNAL`) and the proxy's stdout failing
//! (`STDOUT_CLOSED`), which a directly run agent gets at once, however far
//! behind it is reading its stdin. The host reads it on a thread of its own
//! and never stops reading it, even while it holds the link back for the
//! agent's stdin (ADR 6).

use std::io::{self, ErrorKind, Read, Write};

/// ACP bytes, either direction, on the link.
pub const DATA: u8 = b'D';
/// host → proxy: bytes the agent wrote to its stderr.
pub const STDERR: u8 = b'E';
/// host → proxy: the agent is running. JSON `{"id", "host_pid", "agent_pid"}`.
pub const READY: u8 = b'R';
/// host → proxy: startup failed. JSON `{"error": <message>, "code": <exit code>}`.
pub const FAILED: u8 = b'F';
/// host → proxy: the agent's raw wait status, i32 BE.
pub const EXIT: u8 = b'X';
/// proxy → host, on the signal link: a signal the proxy received, i32 BE.
pub const SIGNAL: u8 = b'S';
/// proxy → host, on the link: the proxy's stdin reached EOF.
pub const EOF: u8 = b'Z';
/// proxy → host, on the signal link: writing to the proxy's stdout failed.
pub const STDOUT_CLOSED: u8 = b'C';

/// How much memory a payload is given before its bytes arrive.
const UP_FRONT: usize = 64 << 10;

/// Fails, writing nothing, for a payload too long for the length field. A
/// payload of more than 64 KiB is written as it is, after the head, not
/// copied: the line the host relays can be 32 MiB (ADR 51).
pub fn write(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "frame payload over 4 GiB"))?;
    if payload.len() > 64 << 10 {
        let mut head = [kind, 0, 0, 0, 0];
        head[1..].copy_from_slice(&len.to_be_bytes());
        w.write_all(&head)?;
        return w.write_all(payload);
    }
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)
}

/// Reads one frame; `None` at a clean or mid-frame EOF.
pub fn read(r: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0; 5];
    match r.read_exact(&mut head) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_be_bytes(head[1..].try_into().unwrap()) as usize;
    // Memory as the payload's bytes arrive, not as its length says: a header
    // that is wrong would otherwise take up to 4 GiB at once.
    let mut payload = Vec::with_capacity(len.min(UP_FRONT));
    r.by_ref().take(len as u64).read_to_end(&mut payload)?;
    Ok((payload.len() == len).then_some((head[0], payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `bytes`, failing a read into more than `max` bytes at once.
    struct Small<'a> {
        bytes: &'a [u8],
        max: usize,
    }

    impl Read for Small<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(buf.len() <= self.max, "a read into {} bytes", buf.len());
            self.bytes.read(buf)
        }
    }

    #[test]
    fn a_frame_takes_memory_as_its_bytes_arrive() {
        // Found by the frame fuzz target: five bytes asked for 3.8 GB.
        let mut r = Small { bytes: b"S\xe2\0\0\0", max: UP_FRONT };
        assert_eq!(read(&mut r).unwrap(), None);
        let mut r = Small { bytes: b"D\xff\xff\xff\xffcut short", max: UP_FRONT };
        assert_eq!(read(&mut r).unwrap(), None);
    }

    #[test]
    fn frames_read_as_they_were_written() {
        let big = vec![7; 3 * UP_FRONT + 1];
        let mut link = Vec::new();
        for (kind, payload) in [(DATA, &big[..]), (EOF, &[][..]), (STDERR, b"x")] {
            write(&mut link, kind, payload).unwrap();
        }
        let mut r = &link[..];
        assert_eq!(read(&mut r).unwrap(), Some((DATA, big)));
        assert_eq!(read(&mut r).unwrap(), Some((EOF, Vec::new())));
        assert_eq!(read(&mut r).unwrap(), Some((STDERR, b"x".to_vec())));
        assert_eq!(read(&mut r).unwrap(), None);
        // Cut short, in the header or in the payload.
        assert_eq!(read(&mut &link[..3]).unwrap(), None);
        assert_eq!(read(&mut &link[..9]).unwrap(), None);
    }
}
