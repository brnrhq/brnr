//! `frame::read` on any bytes: the link as the proxy and the host read it.
//! Every frame read is one that was there: written back, it is the bytes it
//! was read from.

#![no_main]

use std::io::Cursor;

use brnr::frame;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut r = Cursor::new(data);
    let mut at = 0;
    while let Ok(Some((kind, payload))) = frame::read(&mut r) {
        let mut back = Vec::new();
        frame::write(&mut back, kind, &payload).unwrap();
        assert_eq!(back, data[at..at + back.len()]);
        at += back.len();
    }
});
