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
//! whose unary part would pass `codec::ESCAPE` ones is written raw instead, so
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
use crate::codec::{
    best_k, frame, get_rice, put_rice, unframe, unzigzag, zigzag, Base32Error, BitReader,
    BitWriter, FrameError,
};
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

// Rice values (`codec::put_rice`) escape to 18 raw bits: heights are
// `u16`, so a second difference is within ±2·65535 and zigzags below 2^18.
/// Bits for a wind strength: up to 8191 thousandths.
const WIND_BITS: u32 = 13;
/// Bits for a column number: up to 511.
const COLUMN_BITS: u32 = 9;

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
        Ok(frame(CODE_PREFIX, self.to_bytes()?))
    }

    /// The layout as the bytes inside its code: the format byte and the
    /// packed body, without the prefix, base32 or checksum. A replay code
    /// (DESIGN §9.7) carries a map's ground as these. Fails as
    /// [`Layout::to_code`] does.
    pub fn to_bytes(&self) -> Result<Vec<u8>, CodeError> {
        let mut bytes = vec![LAYOUT_FORMAT];
        bytes.extend(encode_body(self).ok_or(CodeError::Unencodable)?);
        Ok(bytes)
    }

    /// The layout [`Layout::to_bytes`] wrote: [`CodeError::Format`] for
    /// another format byte, [`CodeError::Malformed`] for anything else that
    /// is not one. Total, like [`Layout::from_code`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Layout, CodeError> {
        match bytes.split_first() {
            None => Err(CodeError::Malformed),
            Some((&LAYOUT_FORMAT, body)) => decode_body(body).ok_or(CodeError::Malformed),
            Some((&format, _)) => Err(CodeError::Format(format)),
        }
    }

    /// The layout a share code holds. Total: any text is a layout or an
    /// error. A decoded layout is not yet checked; run `validate` on it
    /// before playing.
    pub fn from_code(text: &str) -> Result<Layout, CodeError> {
        if text.len() > MAX_CODE_CHARS {
            return Err(CodeError::TooLong);
        }
        let bytes = unframe(CODE_PREFIX, text).map_err(|e| match e {
            FrameError::Prefix => CodeError::Prefix,
            FrameError::Base32(Base32Error::Character) => CodeError::Character,
            FrameError::Base32(Base32Error::Padding) => CodeError::Padding,
            FrameError::Short => CodeError::Malformed,
            FrameError::Checksum => CodeError::Checksum,
        })?;
        Layout::from_bytes(&bytes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{base32, crc16};

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
        assert_eq!(
            Layout::from_bytes(&[LAYOUT_FORMAT + 1]),
            Err(CodeError::Format(LAYOUT_FORMAT + 1))
        );
        assert_eq!(Layout::from_bytes(&[]), Err(CodeError::Malformed));
    }
}
