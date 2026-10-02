//! Framing on the proxy ↔ host link: `[kind: u8][len: u32 BE][payload]`.
//!
//! ACP bytes travel in `DATA` frames exactly as read; the other kinds carry
//! what plain stdio carries out of band (stderr, signals, exit status).

use std::io::{self, ErrorKind, Read, Write};

/// ACP bytes, either direction.
pub const DATA: u8 = b'D';
/// host → proxy: bytes the agent wrote to its stderr.
pub const STDERR: u8 = b'E';
/// host → proxy: the agent is running. JSON `{"id", "host_pid", "agent_pid"}`.
pub const READY: u8 = b'R';
/// host → proxy: startup failed. JSON `{"error": <message>, "code": <exit code>}`.
pub const FAILED: u8 = b'F';
/// host → proxy: the agent's raw wait status, i32 BE.
pub const EXIT: u8 = b'X';
/// host → proxy: the session carries on without the editor (headless); exit 0.
pub const DETACHED: u8 = b'H';
/// proxy → host: a signal the proxy received, i32 BE.
pub const SIGNAL: u8 = b'S';
/// proxy → host: the proxy's stdin reached EOF.
pub const EOF: u8 = b'Z';
/// proxy → host: writing to the proxy's stdout failed.
pub const STDOUT_CLOSED: u8 = b'C';

pub fn write(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
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
