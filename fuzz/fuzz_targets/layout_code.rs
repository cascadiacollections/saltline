//! A layout share code (DESIGN §13.7) is text a player pastes from
//! anywhere, so the decoder meets arbitrary input. It must never panic or
//! allocate without bound, and whatever it decodes must encode back to a
//! code that decodes to the same layout.
//!
//! Random text almost never gets past the checksum, which would leave the
//! body decoder unfuzzed. So the input is used twice: as text, as a player
//! would paste it, and as a body, framed here into a code with the right
//! format byte and checksum so every mutation reaches the bit-level
//! decoder. The framing is written out again below rather than borrowed
//! from the sim, so it checks the format as DESIGN describes it, not just
//! the code against itself.
#![no_main]

use libfuzzer_sys::fuzz_target;
use salt_line_sim::layout::{Layout, CODE_PREFIX, LAYOUT_FORMAT};

fn check(text: &str) {
    if let Ok(layout) = Layout::from_code(text) {
        let again = layout
            .to_code()
            .expect("a decoded layout fits the format it came in");
        assert_eq!(Layout::from_code(&again).as_ref(), Ok(&layout));
    }
}

/// CRC-16/CCITT-FALSE, as `code.rs` documents it.
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
    let mut framed = vec![LAYOUT_FORMAT];
    framed.extend_from_slice(&data[..data.len().min(1024)]);
    let crc = crc16(&framed);
    framed.extend(crc.to_be_bytes());
    check(&format!("{CODE_PREFIX}{}", base32(&framed)));
});
