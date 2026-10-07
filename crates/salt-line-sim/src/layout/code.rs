//! Share codes: a layout as a line of text. DESIGN §13.7.
//!
//! ```text
//!   SL 0 1 ............................................ 7 Q
//!   ── ─────────────────── base32 ──────────────────────────
//!   prefix   format byte · packed fields · CRC-16
//! ```
//!
//! A code is `SL` followed by the bytes below in Crockford's base32
//! (`0`–`9` and the letters but `I`, `L`, `O`, `U`). Base32 rather than
//! base64 because every character is one the seed keypad can type, case
//! does not matter, and a code read aloud or retyped from a screenshot has
//! no `l`/`1` or `O`/`0` to confuse: the decoder reads `O` as `0` and `I`
//! or `L` as `1`, and skips spaces and hyphens. It costs a fifth more
//! characters than base64.
//!
//! The bytes are the format number ([`LAYOUT_FORMAT`]), a bit-packed body,
//! and a CRC-16 of both. The body, most significant bit first:
//!
//! | Field | Bits |
//! |---|---|
//! | width | 9 |
//! | name length, then each character's index in the keypad charset | 4, then 6 each |
//! | wind: 0 calm, 1 fixed (sign, then strength), 2 range (min, max) | 2, then 1 + 13 or 13 + 13 |
//! | gun count − 1, then each pivot column | 1, then 9 each |
//! | structure count, then each kind and leftmost column | 3, then 3 + 9 each |
//! | the first height | 16 |
//! | how the rest are coded (below), and its Rice parameters | 1, then 4 or 8 |
//! | each later column's second difference, zigzagged and Rice-coded | 1 + k at best |
//!
//! then zero bits to the end of the byte.
//!
//! **Why second differences.** Ground is smooth: a column is very nearly
//! where the slope of the two before it points. Coding what is left over,
//! `h[i] - 2·h[i-1] + h[i-2]` (the first column's is just its slope), turns
//! a hill into a run of small numbers, and a flat or evenly sloped stretch
//! into zeros. Zigzag folds the sign in (0, −1, 1, −2 … become
//! 0, 1, 2, 3 …), and **Rice coding** spends few bits on small numbers: the
//! value's high part in unary, then its low `k` bits as they are. The
//! encoder tries every `k` and keeps the shortest. A zero costs `1 + k`
//! bits, so a flat stretch at `k = 0` costs one bit a column. A value
//! whose unary part would pass [`ESCAPE`] ones is written raw instead, so
//! no value costs more than 42 bits and the decoder never loops on a long
//! run of ones.
//!
//! That bit a column is still most of the code for ground made in the
//! editor, with its long flat or straight stretches, all zeros. So one bit
//! picks a second way, **run-length**: a count of zeros, then one non-zero
//! value, each Rice-coded with a `k` of its own, and again. The encoder
//! works out both and writes the shorter: level ground codes in 28
//! characters rather than 89, and most generated ground, with few zeros,
//! keeps the first way.
//!
//! **Totality.** [`Layout::from_code`] never panics and never allocates
//! more than a bound: text longer than [`MAX_CODE_CHARS`] is refused before
//! anything is decoded, every count is read from a fixed-width field and
//! checked against its limit before it is used, and every read past the end
//! is an error. A code that decodes re-encodes to the same layout (the fuzz
//! target holds it to that).

use super::{Layout, LayoutName, Placed, WindSpec, NAME_MAX};
use crate::run::StructureKind;
use crate::SCREEN_W_MAX;

/// What every code starts with.
pub const CODE_PREFIX: &str = "SL";

/// The format the bytes are in. A decoder refuses any other: a later format
/// is a new number, and this decoder's job is to say it cannot read it,
/// not to guess. Independent of `SIM_EPOCH` (§13.4).
pub const LAYOUT_FORMAT: u8 = 1;

/// The longest text [`Layout::from_code`] will look at. The longest code
/// any layout can have, 400 columns of noise with every second difference
/// escaped, is about 3,400 characters; this leaves room for spaces and
/// hyphens and still bounds the work.
pub const MAX_CODE_CHARS: usize = 4096;

/// Ones in a row that mean "raw value follows" rather than a unary count.
const ESCAPE: u32 = 24;
/// Bits in a raw (escaped) zigzagged second difference. Heights are `u16`,
/// so a second difference is within ±2·65535 and zigzags below 2^18.
const RAW_BITS: u32 = 18;
/// Bits for a wind strength: up to 8191 thousandths.
const WIND_BITS: u32 = 13;
/// Bits for a column number: up to 511.
const COLUMN_BITS: u32 = 9;

/// Crockford's base32 alphabet.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Why a code could not be written or read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeError {
    /// Encoding: the layout has a field the format cannot hold (no columns,
    /// more than `SCREEN_W_MAX`, a column past 511, more than 7 structures, more than 2 guns or none, a
    /// wind past 8191 thousandths or a range with a negative end).
    Unencodable,
    /// Decoding: the text does not start with [`CODE_PREFIX`].
    Prefix,
    /// Decoding: longer than [`MAX_CODE_CHARS`].
    TooLong,
    /// Decoding: a character that is not base32.
    Character,
    /// Decoding: the last character carries bits past the last whole byte
    /// that are not zero, as no encoder writes them: a cut-off or garbled
    /// code.
    Padding,
    /// Decoding: a format this build does not read.
    Format(u8),
    /// Decoding: the checksum does not match: a typo, or a cut-off code.
    Checksum,
    /// Decoding: the checksum matched but the body is not a layout (a field
    /// out of range, ends early, or carries bits past its end).
    Malformed,
}

impl Layout {
    /// This layout as a share code. Fails only for a layout outside what
    /// the format holds (see [`CodeError::Unencodable`]); anything that
    /// passes placement checks fits.
    pub fn to_code(&self) -> Result<String, CodeError> {
        let mut bytes = vec![LAYOUT_FORMAT];
        bytes.extend(encode_body(self).ok_or(CodeError::Unencodable)?);
        let crc = crc16(&bytes);
        bytes.extend(crc.to_be_bytes());
        let mut out = String::from(CODE_PREFIX);
        out.push_str(&base32(&bytes));
        Ok(out)
    }

    /// The layout a share code holds. Total: any text is a layout or an
    /// error. A decoded layout is not yet checked; run `validate` on it
    /// before playing.
    pub fn from_code(text: &str) -> Result<Layout, CodeError> {
        if text.len() > MAX_CODE_CHARS {
            return Err(CodeError::TooLong);
        }
        let text = text.trim();
        let body = text
            .get(..CODE_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(CODE_PREFIX))
            .map(|_| &text[CODE_PREFIX.len()..])
            .ok_or(CodeError::Prefix)?;
        let bytes = unbase32(body)?;
        if bytes.len() < 3 {
            return Err(CodeError::Malformed);
        }
        let (data, crc) = bytes.split_at(bytes.len() - 2);
        if crc16(data).to_be_bytes() != [crc[0], crc[1]] {
            return Err(CodeError::Checksum);
        }
        if data[0] != LAYOUT_FORMAT {
            return Err(CodeError::Format(data[0]));
        }
        decode_body(&data[1..]).ok_or(CodeError::Malformed)
    }
}

// ------------------------------------------------------------------ body

fn encode_body(l: &Layout) -> Option<Vec<u8>> {
    let w = l.heights.len();
    let fits = |v: usize| v < 1 << COLUMN_BITS;
    if w == 0
        || w > SCREEN_W_MAX
        || !(1..=2).contains(&l.guns.len())
        || l.structures.len() > 7
        || l.guns.iter().any(|&g| !fits(g as usize))
        || l.structures.iter().any(|p| !fits(p.x0 as usize))
    {
        return None;
    }
    let strength = |v: i32| (0..1 << WIND_BITS).contains(&v);
    let mut b = BitWriter::default();
    b.put(w as u32, COLUMN_BITS);
    let name = l.name.as_str().as_bytes();
    b.put(name.len() as u32, 4);
    for c in name {
        let i = LayoutName::CHARSET.iter().position(|x| x == c)?;
        b.put(i as u32, 6);
    }
    match l.wind {
        WindSpec::Calm => b.put(0, 2),
        WindSpec::Fixed(v) => {
            if !strength(v.unsigned_abs() as i32) {
                return None;
            }
            b.put(1, 2);
            b.put((v < 0) as u32, 1);
            b.put(v.unsigned_abs(), WIND_BITS);
        }
        WindSpec::Range { min, max } => {
            if !strength(min) || !strength(max) {
                return None;
            }
            b.put(2, 2);
            b.put(min as u32, WIND_BITS);
            b.put(max as u32, WIND_BITS);
        }
    }
    b.put(l.guns.len() as u32 - 1, 1);
    for &g in &l.guns {
        b.put(g as u32, COLUMN_BITS);
    }
    b.put(l.structures.len() as u32, 3);
    for p in &l.structures {
        b.put(p.kind as u32, 3);
        b.put(p.x0 as u32, COLUMN_BITS);
    }
    b.put(l.heights[0] as u32, 16);
    put_residuals(&mut b, &second_differences(&l.heights));
    Some(b.finish())
}

/// The Rice parameter that spends fewest bits on `values`, and the bits.
fn best_k(values: &[u32]) -> (u32, u32) {
    (0..16)
        .map(|k| (k, values.iter().map(|&z| rice_len(z, k)).sum::<u32>()))
        .min_by_key(|&(_, bits)| bits)
        .unwrap_or((0, 0))
}

/// The residuals as zero runs and the values between them, the way
/// [`get_residuals`] reads them back: a run of zeros, then (unless that
/// run reached the end) one non-zero value, less one, and again.
fn runs_and_values(z: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let (mut runs, mut values) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < z.len() {
        let run = z[i..].iter().take_while(|&&v| v == 0).count();
        runs.push(run as u32);
        i += run;
        if let Some(&v) = z.get(i) {
            values.push(v - 1);
            i += 1;
        }
    }
    (runs, values)
}

/// Write the residuals the shorter of two ways, after one bit saying
/// which: each Rice-coded at the best `k`, or as zero runs and the values
/// between them, each with its own best `k`.
fn put_residuals(b: &mut BitWriter, z: &[u32]) {
    let (k, plain) = best_k(z);
    let (runs, values) = runs_and_values(z);
    let ((run_k, run_bits), (value_k, value_bits)) = (best_k(&runs), best_k(&values));
    if run_bits + value_bits + 4 < plain {
        b.put(1, 1);
        b.put(run_k, 4);
        b.put(value_k, 4);
        for (i, &run) in runs.iter().enumerate() {
            put_rice(b, run, run_k);
            if let Some(&v) = values.get(i) {
                put_rice(b, v, value_k);
            }
        }
    } else {
        b.put(0, 1);
        b.put(k, 4);
        for &v in z {
            put_rice(b, v, k);
        }
    }
}

/// Read `n` residuals written by [`put_residuals`].
fn get_residuals(r: &mut BitReader, n: usize) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(n);
    if r.get(1)? == 0 {
        let k = r.get(4)?;
        for _ in 0..n {
            out.push(get_rice(r, k)?);
        }
        return Some(out);
    }
    let (run_k, value_k) = (r.get(4)?, r.get(4)?);
    while out.len() < n {
        let run = get_rice(r, run_k)? as usize;
        if run > n - out.len() {
            return None;
        }
        out.resize(out.len() + run, 0);
        if out.len() < n {
            out.push(get_rice(r, value_k)?.checked_add(1)?);
        }
    }
    Some(out)
}

fn decode_body(bytes: &[u8]) -> Option<Layout> {
    let mut r = BitReader::new(bytes);
    let w = r.get(COLUMN_BITS)? as usize;
    if w == 0 || w > SCREEN_W_MAX {
        return None;
    }
    let n = r.get(4)? as usize;
    if n > NAME_MAX {
        return None;
    }
    let mut name = String::with_capacity(n);
    for _ in 0..n {
        name.push(*LayoutName::CHARSET.get(r.get(6)? as usize)? as char);
    }
    let name = LayoutName::parse(&name)?;
    let wind = match r.get(2)? {
        0 => WindSpec::Calm,
        1 => {
            let negative = r.get(1)? == 1;
            let v = r.get(WIND_BITS)? as i32;
            WindSpec::Fixed(if negative { -v } else { v })
        }
        2 => WindSpec::Range {
            min: r.get(WIND_BITS)? as i32,
            max: r.get(WIND_BITS)? as i32,
        },
        _ => return None,
    };
    let guns = (0..r.get(1)? + 1)
        .map(|_| r.get(COLUMN_BITS).map(|g| g as u16))
        .collect::<Option<Vec<u16>>>()?;
    let structures = (0..r.get(3)?)
        .map(|_| {
            let kind = *StructureKind::ALL.get(r.get(3)? as usize)?;
            let x0 = r.get(COLUMN_BITS)? as u16;
            Some(Placed { kind, x0 })
        })
        .collect::<Option<Vec<Placed>>>()?;
    let mut heights = Vec::with_capacity(w);
    heights.push(r.get(16)? as u16);
    let mut slope: i64 = 0;
    for z in get_residuals(&mut r, w - 1)? {
        slope += unzigzag(z);
        let h = *heights.last()? as i64 + slope;
        heights.push(u16::try_from(h).ok()?);
    }
    // Only zero padding may follow, and less than a byte of it.
    if !r.rest_is_padding() {
        return None;
    }
    Some(Layout {
        name,
        heights,
        structures,
        guns,
        wind,
    })
}

/// Each column's second difference after the first, zigzagged: the first
/// later column's is its slope from the first, every other one's is how
/// much the slope changed.
fn second_differences(h: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(h.len().saturating_sub(1));
    let mut slope: i64 = 0;
    for pair in h.windows(2) {
        let d = pair[1] as i64 - pair[0] as i64;
        out.push(zigzag(d - slope));
        slope = d;
    }
    out
}

/// 0, −1, 1, −2, 2 … as 0, 1, 2, 3, 4 …. Inputs are within ±2·65535.
fn zigzag(v: i64) -> u32 {
    ((v << 1) ^ (v >> 63)) as u32
}

fn unzigzag(z: u32) -> i64 {
    let z = z as i64;
    (z >> 1) ^ -(z & 1)
}

/// Bits [`put_rice`] spends on `z` at parameter `k`.
fn rice_len(z: u32, k: u32) -> u32 {
    let q = z >> k;
    if q < ESCAPE {
        q + 1 + k
    } else {
        ESCAPE + RAW_BITS
    }
}

fn put_rice(b: &mut BitWriter, z: u32, k: u32) {
    let q = z >> k;
    if q < ESCAPE {
        for _ in 0..q {
            b.put(1, 1);
        }
        b.put(0, 1);
        b.put(z & ((1 << k) - 1), k);
    } else {
        for _ in 0..ESCAPE {
            b.put(1, 1);
        }
        b.put(z, RAW_BITS);
    }
}

fn get_rice(r: &mut BitReader, k: u32) -> Option<u32> {
    let mut q = 0;
    while q < ESCAPE {
        if r.get(1)? == 0 {
            return Some((q << k) | r.get(k)?);
        }
        q += 1;
    }
    r.get(RAW_BITS)
}

// ------------------------------------------------------------------ bits

/// Packs bits most significant first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    /// The low `n` bits of `v`, `n <= 32`.
    fn put(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let bit = ((v >> i) & 1) as u8;
            if let Some(last) = self.bytes.last_mut() {
                *last |= bit << (7 - self.used);
            }
            self.used = (self.used + 1) % 8;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        BitReader { bytes, pos: 0 }
    }

    /// The next `n` bits as a number, `n <= 32`; `None` past the end.
    fn get(&mut self, n: u32) -> Option<u32> {
        let mut v: u32 = 0;
        for _ in 0..n {
            let byte = self.bytes.get(self.pos / 8)?;
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }

    /// Whether what is left is under a byte of zero bits.
    fn rest_is_padding(&mut self) -> bool {
        let left = self.bytes.len() * 8 - self.pos;
        left < 8 && self.get(left as u32) == Some(0)
    }
}

// ---------------------------------------------------------------- base32

fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let mut acc: u32 = 0;
    let mut bits = 0;
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

/// Base32 back to bytes. Case-insensitive; `O` reads as `0`, `I` and `L`
/// as `1`; spaces and hyphens are skipped. The bits left over past the last
/// whole byte must be zero, so every byte string has one spelling (bar
/// case, the characters read alike and separators).
fn unbase32(text: &str) -> Result<Vec<u8>, CodeError> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in text.bytes() {
        let c = match c.to_ascii_uppercase() {
            b' ' | b'-' => continue,
            b'O' => b'0',
            b'I' | b'L' => b'1',
            c => c,
        };
        let v = ALPHABET
            .iter()
            .position(|&a| a == c)
            .ok_or(CodeError::Character)? as u32;
        acc = ((acc << 5) | v) & 0xFFF;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits < 5 && acc & ((1 << bits) - 1) == 0 {
        Ok(out)
    } else {
        Err(CodeError::Padding)
    }
}

/// CRC-16/CCITT-FALSE (polynomial 0x1021, initial 0xFFFF): catches every
/// single and double character error and all but one in 65,536 others.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_is_the_ccitt_false_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn base32_round_trips_and_reads_similar_characters() {
        for len in 0..12 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = base32(&bytes);
            assert_eq!(unbase32(&text), Ok(bytes.clone()));
            assert_eq!(unbase32(&text.to_ascii_lowercase()), Ok(bytes));
        }
        assert_eq!(unbase32("O0"), unbase32("00"));
        assert_eq!(unbase32("il00"), unbase32("1100"));
        assert_eq!(unbase32("00-0 0"), unbase32("0000"));
        assert_eq!(unbase32("U"), Err(CodeError::Character));
        assert_eq!(unbase32("01"), Err(CodeError::Padding));
        assert_eq!(
            unbase32("000"),
            Err(CodeError::Padding),
            "a stray character"
        );
    }

    #[test]
    fn another_format_is_named_not_guessed() {
        let mut bytes = vec![LAYOUT_FORMAT + 1, 0, 0];
        let crc = crc16(&bytes);
        bytes.extend(crc.to_be_bytes());
        let code = format!("{CODE_PREFIX}{}", base32(&bytes));
        assert_eq!(
            Layout::from_code(&code),
            Err(CodeError::Format(LAYOUT_FORMAT + 1))
        );
    }

    #[test]
    fn zigzag_folds_the_sign_in() {
        for (v, z) in [(0, 0), (-1, 1), (1, 2), (-2, 3), (131_070, 262_140)] {
            assert_eq!(zigzag(v), z);
            assert_eq!(unzigzag(z), v);
        }
    }

    #[test]
    fn an_escaped_value_reads_back() {
        for k in [0, 3, 15] {
            // Every real value is under 2^18 (see RAW_BITS).
            for z in [
                0,
                1,
                23,
                (24 << k).min((1 << RAW_BITS) - 1),
                (1 << RAW_BITS) - 1,
            ] {
                let mut b = BitWriter::default();
                put_rice(&mut b, z, k);
                assert!(b.bytes.len() as u32 * 8 >= rice_len(z, k));
                let bytes = b.finish();
                assert_eq!(get_rice(&mut BitReader::new(&bytes), k), Some(z));
            }
        }
    }
}
