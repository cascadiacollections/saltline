//! Sound cues and their PCM. The engine's WAVs are embedded (mono, 16-bit,
//! 22.05 kHz); the SHIFT whoosh is synthesized here, the way the engine's
//! own effects were made. No file loading, so web and desktop behave alike.

/// What the game asks the engine to play.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cue {
    Ui,
    Fire,
    Throw,
    TerrainHit,
    StructureHit,
    Collapse,
    Shift,
    Draft,
    Victory,
    Defeat,
    Title,
    /// Ambient loop under play in the present: gusting coastal wind, surf.
    AmbPresent,
    /// Ambient loop under play in 1347: a calmer wind over a low drone.
    AmbSiege,
}

impl Cue {
    pub const ALL: [Cue; 13] = [
        Cue::Ui,
        Cue::Fire,
        Cue::Throw,
        Cue::TerrainHit,
        Cue::StructureHit,
        Cue::Collapse,
        Cue::Shift,
        Cue::Draft,
        Cue::Victory,
        Cue::Defeat,
        Cue::Title,
        Cue::AmbPresent,
        Cue::AmbSiege,
    ];

    pub fn index(self) -> usize {
        Cue::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// Whether the engine should loop it.
    pub fn looping(self) -> bool {
        matches!(self, Cue::Title | Cue::AmbPresent | Cue::AmbSiege)
    }

    /// The ambient loop for an era.
    pub fn ambience(era: shift_sim::Plane) -> Cue {
        match era {
            shift_sim::Plane::Present => Cue::AmbPresent,
            shift_sim::Plane::Siege => Cue::AmbSiege,
        }
    }
}

pub const MIX_RATE: u32 = 22_050;

fn wav(cue: Cue) -> &'static [u8] {
    match cue {
        Cue::Ui => include_bytes!("../../../art/audio/ui.wav"),
        Cue::Fire => include_bytes!("../../../art/audio/fire.wav"),
        Cue::Throw => include_bytes!("../../../art/audio/flight.wav"),
        Cue::TerrainHit => include_bytes!("../../../art/audio/terrain_impact.wav"),
        Cue::StructureHit => include_bytes!("../../../art/audio/structure_impact.wav"),
        Cue::Collapse => include_bytes!("../../../art/audio/collapse.wav"),
        Cue::Draft => include_bytes!("../../../art/audio/draft.wav"),
        Cue::Victory => include_bytes!("../../../art/audio/versus_win.wav"),
        Cue::Defeat => include_bytes!("../../../art/audio/low_shell.wav"),
        Cue::Title => include_bytes!("../../../art/audio/title_loop.wav"),
        Cue::Shift | Cue::AmbPresent | Cue::AmbSiege => &[],
    }
}

/// The cue's samples as little-endian 16-bit mono PCM bytes at [`MIX_RATE`].
pub fn pcm(cue: Cue) -> Vec<u8> {
    match cue {
        Cue::Shift => return shift_whoosh(),
        Cue::AmbPresent => return ambience(false),
        Cue::AmbSiege => return ambience(true),
        _ => {}
    }
    data_chunk(wav(cue)).to_vec()
}

/// The `data` chunk of a RIFF WAVE file; empty if there is none.
fn data_chunk(b: &[u8]) -> &[u8] {
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        if id == b"data" {
            return &b[body..(body + len).min(b.len())];
        }
        i = body + len + (len & 1);
    }
    &[]
}

/// A 0.45 s whoosh: noise through a resonant band-pass whose centre sweeps
/// down, with a soft sine "tide" underneath. Deterministic (fixed LCG).
fn shift_whoosh() -> Vec<u8> {
    let n = (MIX_RATE as f32 * 0.45) as usize;
    let mut out = Vec::with_capacity(n * 2);
    let mut seed: u32 = 0x5EED_1347;
    let (mut lp, mut bp) = (0.0f32, 0.0f32);
    for k in 0..n {
        let t = k as f32 / n as f32;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
        let fc = 2400.0 * (1.0 - t) + 300.0;
        let f = 2.0 * (std::f32::consts::PI * fc / MIX_RATE as f32).sin();
        let q = 0.35;
        lp += f * bp;
        let hp = noise - lp - q * bp;
        bp += f * hp;
        let env = (t * 12.0).min(1.0) * (1.0 - t).powf(1.6);
        let tide = (std::f32::consts::TAU * (220.0 - 120.0 * t) * k as f32 / MIX_RATE as f32).sin();
        let s = (bp * 0.55 + tide * 0.18) * env;
        let v = (s.clamp(-1.0, 1.0) * 30_000.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Seconds in an ambient loop. Every tone and LFO in [`ambience`] runs a
/// whole number of cycles in it, and the noise is crossfaded across the
/// seam, so the loop has no click.
pub const AMB_SECS: u32 = 8;

/// An era's ambient loop as PCM bytes. Deterministic (fixed LCG).
fn ambience(siege: bool) -> Vec<u8> {
    use std::f32::consts::TAU;
    let n = (MIX_RATE * AMB_SECS) as usize;
    let x = MIX_RATE as usize / 2; // crossfade length
    let rate = MIX_RATE as f32;
    let mut seed: u32 = if siege { 0x1347_0001 } else { 0x2026_0001 };
    let mut gen = Vec::with_capacity(n + x);
    // One-pole low-passes: wind (cutoff swept by gusts) and surf.
    let (mut wind, mut wind2, mut surf) = (0.0f32, 0.0f32, 0.0f32);
    for k in 0..n + x {
        let t = k as f32 / rate;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
        // Gusts: two slow LFOs with whole cycles per loop.
        let loop_t = t / AMB_SECS as f32;
        let gust = 0.5 + 0.3 * (TAU * 2.0 * loop_t).sin() + 0.2 * (TAU * 5.0 * loop_t + 1.3).sin();
        let gust = gust.clamp(0.0, 1.0);
        let fc = if siege {
            250.0 + 350.0 * gust
        } else {
            300.0 + 900.0 * gust
        };
        let a = 1.0 - (-TAU * fc / rate).exp();
        wind += a * (noise - wind);
        wind2 += a * (wind - wind2);
        let mut s = wind2 * (if siege { 0.9 } else { 1.4 }) * (0.4 + 0.6 * gust);
        if siege {
            // A low open fifth (A2, E3), swelling slowly; whole cycles in 8 s.
            let swell = 0.6 + 0.4 * (TAU * loop_t).sin();
            let drone = (TAU * 110.0 * t).sin() * 0.6
                + (TAU * 165.0 * t).sin() * 0.4
                + (TAU * 220.0 * t).sin() * 0.12;
            s += drone * 0.05 * swell;
        } else {
            // Surf: a low rumble that rolls in twice a loop.
            let a2 = 1.0 - (-TAU * 160.0 / rate).exp();
            surf += a2 * (noise - surf);
            let roll = (TAU * 2.0 * loop_t - 1.2).sin().max(0.0).powf(2.0);
            s += surf * 2.2 * roll;
        }
        gen.push(s);
    }
    // Normalize to a fixed peak; the engine sets the playback level.
    let peak = gen.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.6 / peak;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = if i < x {
            let k = i as f32 / x as f32;
            gen[i] * k + gen[n + i] * (1.0 - k)
        } else {
            gen[i]
        } * gain;
        let v = (v.clamp(-1.0, 1.0) * 30_000.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_has_samples() {
        for c in Cue::ALL {
            assert!(pcm(c).len() > 1000, "{c:?}");
        }
    }

    #[test]
    fn ambience_is_audible_and_does_not_clip() {
        for c in [Cue::AmbPresent, Cue::AmbSiege] {
            let b = pcm(c);
            assert_eq!(b.len(), (MIX_RATE * AMB_SECS) as usize * 2);
            let s: Vec<i16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| i16::from_le_bytes(*p))
                .collect();
            let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
            let rms = (s.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
            assert!(peak < 30_000, "{c:?} clips: {peak}");
            assert!(rms > 600.0, "{c:?} too quiet: {rms:.0}");
            // The seam: the last sample runs into the first without a jump.
            let jump = (s[s.len() - 1] as i32 - s[0] as i32).abs();
            assert!(jump < 2_000, "{c:?} seam jumps {jump}");
        }
    }
}
