//! A replay share code (DESIGN §9.7) is text a player pastes from a chat,
//! so the decoder meets arbitrary input. It must never panic or allocate
//! without bound, and:
//!
//! - **What reads, shares again as itself.** A record decoded from a code
//!   encodes (if this build can share it at all) to a code that decodes to
//!   the same record, and its history line reads back as that record.
//! - **A record's check never panics** (`Record::check`, which WATCH runs
//!   before a pasted replay is shown).
//!
//! As for `layout_code`, random text almost never passes the checksum, so
//! the input is used twice: as text, and as a body framed here with the
//! right format byte and checksum (written out again rather than borrowed,
//! so it checks the frame as DESIGN describes it), so mutations reach the
//! bit-level decoder.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_godot::history::code::{CODE_FORMAT, CODE_PREFIX};
use salt_line_godot::history::Record;

fn check(text: &str) {
    let Ok(record) = Record::from_code(text) else {
        return;
    };
    assert!(!record.starred);
    // The list shows it: its row must not panic, whatever its day.
    let _ = record.summary();
    assert_eq!(Record::parse(&record.to_line()).as_ref(), Ok(&record));
    if let Ok(code) = record.to_code() {
        assert_eq!(Record::from_code(&code).as_ref(), Ok(&record));
    }
    // A replay is the slow half: only short ones.
    if text.len() < 1024 {
        let _ = record.check();
    }
}

/// CRC-16/CCITT-FALSE.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Crockford base32, most significant bit first, zero-padded.
fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0);
    for &b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        check(text);
    }
    let mut framed = vec![CODE_FORMAT];
    framed.extend_from_slice(&data[..data.len().min(2048)]);
    let crc = crc16(&framed);
    framed.extend(crc.to_be_bytes());
    check(&format!("{CODE_PREFIX}{}", base32(&framed)));
});
