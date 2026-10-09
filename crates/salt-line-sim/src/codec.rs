//! The pieces share codes are built from: bits packed most significant
//! first, Rice and length-prefixed numbers, Crockford base32 and a CRC-16.
//! DESIGN §13.7 (layout codes, `SL…`) and §9.7 (replay codes, `SR…`).
//!
//! Both codes are the same shape: a prefix, then base32 of a format byte, a
//! bit-packed body and a CRC-16 of both. What differs is the body, which
//! each code lays out for itself (`layout/code.rs`, and the game crate's
//! `history/code.rs`). Kept here, in one place, so the two read and spell
//! alike: a code read aloud, retyped or pasted from a chat is forgiven the
//! same things whichever it is.
//!
//! Every reader here is total: a read past the end is `None`, never a
//! panic, and no loop runs longer than its input or a fixed bound.

/// Crockford's base32 alphabet: `0`–`9` and the letters but `I`, `L`, `O`,
/// `U`.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Ones in a row that mean "raw value follows" rather than a unary count
/// ([`put_rice`]).
pub const ESCAPE: u32 = 24;
/// Bits in a raw (escaped) Rice value. Values written with [`put_rice`]
/// must be below 2^18.
pub const RAW_BITS: u32 = 18;

/// Why base32 text did not read as bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base32Error {
    /// A character that is not base32 (nor a separator).
    Character,
    /// The bits past the last whole byte are not zero, as no encoder
    /// writes them: a cut-off or garbled code.
    Padding,
}

// ------------------------------------------------------------------ bits

/// Packs bits most significant first.
#[derive(Default, Debug)]
pub struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    /// The low `n` bits of `v`, `n <= 32`.
    pub fn put(&mut self, v: u32, n: u32) {
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

    /// Bits written so far.
    pub fn bits(&self) -> usize {
        match self.used {
            0 => self.bytes.len() * 8,
            u => (self.bytes.len() - 1) * 8 + u as usize,
        }
    }

    /// The bytes, the last one padded with zero bits.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Reads what [`BitWriter`] wrote.
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        BitReader { bytes, pos: 0 }
    }

    /// The next `n` bits as a number, `n <= 32`; `None` past the end.
    pub fn get(&mut self, n: u32) -> Option<u32> {
        let mut v: u32 = 0;
        for _ in 0..n {
            let byte = self.bytes.get(self.pos / 8)?;
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }

    /// Bits not yet read.
    pub fn left(&self) -> usize {
        (self.bytes.len() * 8).saturating_sub(self.pos)
    }

    /// Whether what is left is under a byte of zero bits.
    pub fn rest_is_padding(&mut self) -> bool {
        let left = self.left();
        left < 8 && self.get(left as u32) == Some(0)
    }
}

// --------------------------------------------------------------- numbers

/// 0, −1, 1, −2, 2 … as 0, 1, 2, 3, 4 …. Inputs are within ±2^31.
pub fn zigzag(v: i64) -> u32 {
    ((v << 1) ^ (v >> 63)) as u32
}

/// The inverse of [`zigzag`].
pub fn unzigzag(z: u32) -> i64 {
    let z = z as i64;
    (z >> 1) ^ -(z & 1)
}

/// Bits [`put_rice`] spends on `z` at parameter `k`.
pub fn rice_len(z: u32, k: u32) -> u32 {
    let q = z >> k;
    if q < ESCAPE {
        q + 1 + k
    } else {
        ESCAPE + RAW_BITS
    }
}

/// `z` Rice-coded at parameter `k`: its high part in unary, then its low
/// `k` bits. A value whose unary part would reach [`ESCAPE`] ones is
/// written as the ones and then [`RAW_BITS`] raw, so no value costs more
/// than 42 bits and a reader never loops on a long run of ones. `z` must be
/// below 2^[`RAW_BITS`], `k` below 16.
pub fn put_rice(b: &mut BitWriter, z: u32, k: u32) {
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

/// Read a value written by [`put_rice`].
pub fn get_rice(r: &mut BitReader, k: u32) -> Option<u32> {
    let mut q = 0;
    while q < ESCAPE {
        if r.get(1)? == 0 {
            return Some((q << k) | r.get(k)?);
        }
        q += 1;
    }
    r.get(RAW_BITS)
}

/// The Rice parameter (0 to 15) that spends fewest bits on `values`, and
/// the bits.
pub fn best_k(values: &[u32]) -> (u32, u32) {
    (0..16)
        .map(|k| (k, values.iter().map(|&z| rice_len(z, k)).sum::<u32>()))
        .min_by_key(|&(_, bits)| bits)
        .unwrap_or((0, 0))
}

/// `v` as Elias gamma of `v + 1`: as many zeros as `v + 1` has bits after
/// its first, then `v + 1`. One bit for 0, three for 1 and 2, five for 3 to
/// 6: for small counts. `v` below `u32::MAX`.
pub fn put_gamma(b: &mut BitWriter, v: u32) {
    let v = v.saturating_add(1);
    let n = 32 - v.leading_zeros();
    b.put(0, n - 1);
    b.put(v, n);
}

/// Read a value written by [`put_gamma`]. At most 31 zeros are read before
/// the value, so the read is bounded.
pub fn get_gamma(r: &mut BitReader) -> Option<u32> {
    let mut zeros = 0;
    while r.get(1)? == 0 {
        zeros += 1;
        if zeros > 31 {
            return None;
        }
    }
    let rest = r.get(zeros)?;
    Some(((1u64 << zeros) | rest as u64).checked_sub(1)? as u32)
}

/// Bits [`put_exp_golomb`] spends on `v` at parameter `k`.
pub fn exp_golomb_len(v: u32, k: u32) -> u32 {
    let q = (v >> k).saturating_add(1);
    2 * (31 - q.leading_zeros()) + 1 + k
}

/// `v` exp-Golomb coded at parameter `k`: `v >> k` as [`put_gamma`], then
/// its low `k` bits. Like Rice for small values (`1 + k` bits for any
/// below 2^k), but an outlier costs about twice its length in bits rather
/// than a long unary run: a shot dialled far from the last costs a dozen
/// bits more, not forty. `k` below 16, `v >> k` below `u32::MAX`.
pub fn put_exp_golomb(b: &mut BitWriter, v: u32, k: u32) {
    put_gamma(b, v >> k);
    b.put(v & ((1 << k) - 1), k);
}

/// Read a value written by [`put_exp_golomb`].
pub fn get_exp_golomb(r: &mut BitReader, k: u32) -> Option<u32> {
    let q = get_gamma(r)?;
    let v = (q as u64) << k | r.get(k)? as u64;
    u32::try_from(v).ok()
}

/// The exp-Golomb parameter (0 to 15) that spends fewest bits on `values`,
/// and the bits.
pub fn best_exp_golomb_k(values: &[u32]) -> (u32, u32) {
    (0..16)
        .map(|k| (k, values.iter().map(|&z| exp_golomb_len(z, k)).sum::<u32>()))
        .min_by_key(|&(_, bits)| bits)
        .unwrap_or((0, 0))
}

/// `v` as its length in bits (7 bits, 0 to 64) and then those bits: a
/// whole `u64` (a seed, a day) in as few bits as it needs, plus seven.
pub fn put_wide(b: &mut BitWriter, v: u64) {
    let n = 64 - v.leading_zeros();
    b.put(n, 7);
    if n > 32 {
        b.put((v >> 32) as u32, n - 32);
        b.put(v as u32, 32);
    } else {
        b.put(v as u32, n);
    }
}

/// Read a value written by [`put_wide`]. One spelling per value: a length
/// past 64, or a value whose top bit is not set, is `None`.
pub fn get_wide(r: &mut BitReader) -> Option<u64> {
    let n = r.get(7)?;
    if n > 64 {
        return None;
    }
    let v = if n > 32 {
        let hi = r.get(n - 32)? as u64;
        (hi << 32) | r.get(32)? as u64
    } else {
        r.get(n)? as u64
    };
    (64 - v.leading_zeros() == n).then_some(v)
}

// ---------------------------------------------------------------- base32

/// Bytes as Crockford base32, most significant bit first, the last
/// character zero-padded.
pub fn base32(bytes: &[u8]) -> String {
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
pub fn unbase32(text: &str) -> Result<Vec<u8>, Base32Error> {
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
            .ok_or(Base32Error::Character)? as u32;
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
        Err(Base32Error::Padding)
    }
}

/// CRC-16/CCITT-FALSE (polynomial 0x1021, initial 0xFFFF): catches every
/// single and double character error and all but one in 65,536 others.
pub fn crc16(bytes: &[u8]) -> u16 {
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

/// `prefix`, then base32 of `bytes` and their CRC-16: a whole code.
pub fn frame(prefix: &str, mut bytes: Vec<u8>) -> String {
    let crc = crc16(&bytes);
    bytes.extend(crc.to_be_bytes());
    let mut out = String::from(prefix);
    out.push_str(&base32(&bytes));
    out
}

/// Why [`unframe`] refused text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameError {
    /// It does not start with the prefix.
    Prefix,
    /// Not base32.
    Base32(Base32Error),
    /// Too short to hold a format byte and a checksum.
    Short,
    /// The checksum does not match: a typo, or a cut-off code.
    Checksum,
}

/// The bytes inside a code [`frame`] made with `prefix` (any case, spaces
/// round it trimmed), checked against their CRC-16. At least one byte,
/// the format byte, comes back.
pub fn unframe(prefix: &str, text: &str) -> Result<Vec<u8>, FrameError> {
    let text = text.trim();
    let body = text
        .get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
        .ok_or(FrameError::Prefix)?;
    let mut bytes = unbase32(body).map_err(FrameError::Base32)?;
    if bytes.len() < 3 {
        return Err(FrameError::Short);
    }
    let crc = bytes.split_off(bytes.len() - 2);
    if crc16(&bytes).to_be_bytes() != [crc[0], crc[1]] {
        return Err(FrameError::Checksum);
    }
    Ok(bytes)
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
        assert_eq!(unbase32("U"), Err(Base32Error::Character));
        assert_eq!(unbase32("01"), Err(Base32Error::Padding));
        assert_eq!(
            unbase32("000"),
            Err(Base32Error::Padding),
            "a stray character"
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
                assert_eq!(b.bits() as u32, rice_len(z, k));
                let bytes = b.finish();
                assert_eq!(get_rice(&mut BitReader::new(&bytes), k), Some(z));
            }
        }
    }

    #[test]
    fn gamma_and_wide_numbers_read_back_with_one_spelling() {
        let mut b = BitWriter::default();
        let small = [0, 1, 2, 3, 6, 7, 1000, u32::MAX - 1];
        let wide = [0, 1, 2, 20_734, 1 << 32, (1 << 33) + 5, u64::MAX];
        for &v in &small {
            put_gamma(&mut b, v);
        }
        for &v in &wide {
            put_wide(&mut b, v);
        }
        let bytes = b.finish();
        let mut r = BitReader::new(&bytes);
        for &v in &small {
            assert_eq!(get_gamma(&mut r), Some(v));
        }
        for &v in &wide {
            assert_eq!(get_wide(&mut r), Some(v));
        }
        assert!(r.rest_is_padding());
        // A gamma of 1, 3 and 5 bits.
        for (v, bits) in [(0, 1), (1, 3), (2, 3), (3, 5)] {
            let mut b = BitWriter::default();
            put_gamma(&mut b, v);
            assert_eq!(b.bits(), bits);
        }
        // A wide number with a leading zero, or a length past 64.
        let mut b = BitWriter::default();
        b.put(3, 7);
        b.put(0b011, 3);
        assert_eq!(get_wide(&mut BitReader::new(&b.finish())), None);
        let mut b = BitWriter::default();
        b.put(65, 7);
        assert_eq!(get_wide(&mut BitReader::new(&b.finish())), None);
        // Thirty-two zeros and no one is never a number.
        assert_eq!(get_gamma(&mut BitReader::new(&[0; 8])), None);
    }

    #[test]
    fn exp_golomb_reads_back_at_its_length() {
        for k in [0, 1, 4, 15] {
            for v in [0, 1, 2, 7, 8, 120, 3600, 262_143, u32::MAX >> 1] {
                let mut b = BitWriter::default();
                put_exp_golomb(&mut b, v, k);
                assert_eq!(b.bits() as u32, exp_golomb_len(v, k), "{v} {k}");
                let bytes = b.finish();
                assert_eq!(get_exp_golomb(&mut BitReader::new(&bytes), k), Some(v));
            }
        }
        assert_eq!(best_exp_golomb_k(&[0, 0, 0, 120]).0, 0);
        // 7 bits each but 70, which needs 9: 30 against 32 at k = 5.
        assert_eq!(best_exp_golomb_k(&[40, 50, 60, 70]), (6, 30));
    }

    #[test]
    fn a_frame_reads_back_and_refuses_damage() {
        let code = frame("SR", vec![1, 2, 3]);
        assert_eq!(unframe("SR", &code), Ok(vec![1, 2, 3]));
        assert_eq!(unframe("SR", &code.to_ascii_lowercase()), Ok(vec![1, 2, 3]));
        assert_eq!(unframe("SL", &code), Err(FrameError::Prefix));
        let mut bad = code.clone().into_bytes();
        let last = bad.len() - 2;
        bad[last] = if bad[last] == b'0' { b'1' } else { b'0' };
        let bad = String::from_utf8(bad).unwrap();
        assert_eq!(unframe("SR", &bad), Err(FrameError::Checksum));
        assert_eq!(unframe("SR", "SR"), Err(FrameError::Short));
    }
}
