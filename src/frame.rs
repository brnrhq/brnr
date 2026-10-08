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

/// Fails, writing nothing, for a payload too long for the length field.
pub fn write(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "frame payload over 4 GiB"))?;
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
    let mut payload = vec![0; len];
    match r.read_exact(&mut payload) {
        Ok(()) => Ok(Some((head[0], payload))),
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err),
    }
}
