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
}

impl Cue {
    pub const ALL: [Cue; 11] = [
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
    ];

    pub fn index(self) -> usize {
        Cue::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// Whether the engine should loop it.
    pub fn looping(self) -> bool {
        self == Cue::Title
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
        Cue::Shift => &[],
    }
}

/// The cue's samples as little-endian 16-bit mono PCM bytes at [`MIX_RATE`].
pub fn pcm(cue: Cue) -> Vec<u8> {
    if cue == Cue::Shift {
        return shift_whoosh();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_has_samples() {
        for c in Cue::ALL {
            assert!(pcm(c).len() > 1000, "{c:?}");
        }
    }
}
