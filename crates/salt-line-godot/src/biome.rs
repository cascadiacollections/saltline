//! Cosmetic world themes. Biomes only change presentation colours; the
//! simulation, terrain heights, seeds and replay data remain unchanged.

use crate::palette::{slot, Palette, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    Salt,
    Snow,
    Forest,
}

impl Biome {
    /// Challenges have stable themes; normal runs derive one from existing
    /// run data so no new seed or save field is required.
    pub fn for_run(seed: u64, screen: u32, challenge: Option<usize>) -> Self {
        if let Some(index) = challenge {
            return match index % 3 {
                0 => Self::Snow,
                1 => Self::Forest,
                _ => Self::Salt,
            };
        }
        match (seed ^ screen as u64).wrapping_mul(0x9e37_79b9) % 5 {
            0 => Self::Snow,
            1 => Self::Forest,
            _ => Self::Salt,
        }
    }

    /// Apply the biome to the world-facing palette roles. UI and hull colours
    /// stay unchanged so menus and existing sprites retain their identity.
    pub fn palette(self, base: &Palette) -> Palette {
        let mut themed = base.clone();
        match self {
            Self::Salt => {}
            Self::Snow => {
                themed.colours[slot::SAND] = rgba(224, 232, 236);
                themed.colours[slot::ROCK] = rgba(143, 157, 169);
                themed.colours[slot::BEDROCK] = rgba(76, 91, 108);
                themed.colours[slot::SHADOW] = rgba(48, 61, 76);
                themed.colours[slot::SKY_HI] = rgba(188, 207, 224);
                themed.colours[slot::SKY_MID] = rgba(117, 151, 180);
                themed.colours[slot::SKY_LO] = rgba(73, 106, 137);
            }
            Self::Forest => {
                themed.colours[slot::SAND] = rgba(139, 151, 103);
                themed.colours[slot::ROCK] = rgba(91, 83, 62);
                themed.colours[slot::BEDROCK] = rgba(49, 58, 43);
                themed.colours[slot::SHADOW] = rgba(31, 39, 31);
                themed.colours[slot::SKY_HI] = rgba(52, 78, 74);
                themed.colours[slot::SKY_MID] = rgba(68, 105, 87);
                themed.colours[slot::SKY_LO] = rgba(112, 139, 101);
            }
        }
        themed
    }
}

const fn rgba(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_runs_choose_biomes_deterministically() {
        assert_eq!(Biome::for_run(42, 7, None), Biome::for_run(42, 7, None));
        assert_eq!(Biome::for_run(0, 0, Some(0)), Biome::Snow);
        assert_eq!(Biome::for_run(0, 0, Some(1)), Biome::Forest);
    }

    #[test]
    fn themes_leave_ui_colours_untouched() {
        let base = Palette::default();
        let snow = Biome::Snow.palette(&base);
        assert_eq!(snow.get(slot::UI_FG), base.get(slot::UI_FG));
        assert_ne!(snow.get(slot::SAND), base.get(slot::SAND));
    }
}
