//! A minimal PNG writer (RGB, 8-bit, one stored zlib stream, no
//! compression) for review captures and the generated icon. No dependency.

use std::io::Write;

/// Writes `rgb` (`w` x `h`, 3 bytes a pixel) to `path`, each pixel scaled
/// up `scale` times with nearest-neighbour.
pub fn write(path: &str, w: usize, h: usize, rgb: &[u8], scale: usize) -> std::io::Result<()> {
    let (ow, oh) = (w * scale, h * scale);
    let mut raw = Vec::with_capacity((ow * 3 + 1) * oh);
    for y in 0..oh {
        raw.push(0);
        for x in 0..ow {
            let i = ((y / scale) * w + x / scale) * 3;
            raw.extend_from_slice(&rgb[i..i + 3]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x89PNG\r\n\x1a\n")?;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(ow as u32).to_be_bytes());
    ihdr.extend_from_slice(&(oh as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut f, b"IHDR", &ihdr)?;
    chunk(&mut f, b"IDAT", &z)?;
    chunk(&mut f, b"IEND", &[])
}

fn chunk(f: &mut std::fs::File, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    f.write_all(&(data.len() as u32).to_be_bytes())?;
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    f.write_all(&crc_in)?;
    f.write_all(&crc32(&crc_in).to_be_bytes())
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc_matches_the_png_spec_example() {
        // CRC of "IEND" with no data, as in every PNG.
        assert_eq!(super::crc32(b"IEND"), 0xAE42_6082);
    }
}
