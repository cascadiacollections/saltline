//! The duel layouts that ship with the game (DESIGN §13.9), so the GROUND
//! row has something to pick before anyone has made one.
//!
//! Each is a share code, so it reads exactly as a player's would, and each
//! was made with the editor's own tools ([`recipe`]): a blank draft, MIRROR
//! on, stamps of the generator's shapes, a sculpted bump, a wind and a
//! name. The tests hold every code to its recipe, to the duel gate and to
//! the fairness sign, so a change to the editor's tools, the code or the
//! gate that would change one says so.
//!
//! They are compiled in rather than read from `res://`: the same text
//! either way, and a test can read them without an engine.

use salt_line_sim::layout::{Layout, WindSpec};

use crate::editor::{Draft, Editor, Stamp, Tool, WindMode};

/// The bundled layouts' codes, in the order the GROUND row lists them.
pub const BUNDLED: [&str; 4] = [
    "SL06G2M906628004W91X4405041FZZZZ00GPVDPV9CK924C000G911191918H54545444280008H2JDJJVDPTZZZZZG0881M3S",
    "SL06G2K0H4098004W91X440504JFZZZF33MHMH90M218383M1HFZQH1T0T0M0818MHMHT8RVQ1F2XEY5RBWBRQ1FQE2Y5SWCEJ6J542G850D0EG65ZYY478382G1052J6J7933ZZZYG1FVY",
    "SL06G30G0TR73C002E4GYJ202G257ZZZZG06NFPYBHF5V5TVPBHF5RQVFHRN2G8GW60G4FKKNZZWXCYEJ0GC3H250N1TBXQJWBSESEPXJWBSE5YVZZZZZG06J0VMR0",
    "SL06G34HWGH24J5VJ3T880A087ZZZZY00YCWP09BJPC04WEKGR05CAXG2KKZMWP09BJPC04WEKGR05CAXG2KKX75G2AWNK0173MW601B2QC0MWZX75G2AWNK0173MW601B2QC0MWZZZZZ00F00TDX0",
];

/// The bundled layouts, decoded. Every code decodes (a test holds it).
pub fn layouts() -> Vec<Layout> {
    BUNDLED
        .iter()
        .filter_map(|c| Layout::from_code(c).ok())
        .collect()
}

/// How bundled layout `i` was made, with the editor's tools. `None` past
/// the last.
pub fn recipe(i: usize) -> Option<Layout> {
    let mut e = Editor::new(Draft::blank());
    e.mirror = true;
    let stamp = |e: &mut Editor, s: Stamp, at: u16| {
        e.set_tool(Tool::Stamp);
        e.stamp = s;
        e.cursor = at;
        e.nudge(1);
    };
    let sculpt = |e: &mut Editor, at: f32, units: f32| {
        e.set_tool(Tool::Sculpt);
        let y = 100.0;
        e.begin_stroke(at, y);
        e.drag_stroke(at, y - units);
        e.end_stroke();
    };
    let name = match i {
        // One tall ridge between the guns: lob over it or go around.
        0 => {
            stamp(&mut e, Stamp::Ridge, 160);
            "RIDGE"
        }
        // Two mesas, a gun's shelter in front of each.
        1 => {
            stamp(&mut e, Stamp::Mesa, 110);
            sculpt(&mut e, 160.0, -10.0);
            "MESAS"
        }
        // A deep canyon down the middle with raised lips.
        2 => {
            stamp(&mut e, Stamp::Chasm, 160);
            sculpt(&mut e, 120.0, 14.0);
            "CANYON"
        }
        // Spires in a steady wind that turns round each round.
        3 => {
            stamp(&mut e, Stamp::Spires, 150);
            e.set_wind_mode(WindMode::Fixed);
            e.step_wind(-10);
            "SPIRES"
        }
        _ => return None,
    };
    e.set_name(name);
    Some(e.draft.layout)
}

/// What a bundled layout's wind is, for tests of each kind.
pub fn wind(i: usize) -> Option<WindSpec> {
    layouts().get(i).map(|l| l.wind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{verdict, Fairness};

    /// Prints the codes the recipes make, for pasting into [`BUNDLED`]
    /// when a recipe changes: `cargo test -p salt-line-godot print_bundled
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_bundled() {
        for i in 0.. {
            let Some(l) = recipe(i) else { break };
            println!("{}", l.to_code().unwrap());
        }
    }

    /// CI: every bundled code reads, is its recipe, passes the duel gate
    /// and reads FAIR, and carries a name.
    #[test]
    fn every_bundled_layout_validates_and_is_fair() {
        assert_eq!(layouts().len(), BUNDLED.len(), "every code decodes");
        for (i, code) in BUNDLED.iter().enumerate() {
            let l = Layout::from_code(code).unwrap();
            assert_eq!(Some(&l), recipe(i).as_ref(), "bundled {i} is its recipe");
            assert_eq!(crate::custom::validate(&l), Ok(()), "{}", l.name.as_str());
            let v = verdict(&l);
            assert!(v.playable(), "{:?}", v.problems);
            assert_eq!(v.fairness, Some(Fairness::Fair), "{}", l.name.as_str());
            assert!(!l.name.as_str().is_empty());
            assert!(l.is_duel());
        }
        assert!(
            matches!(wind(3), Some(WindSpec::Fixed(_))),
            "one fixed wind"
        );
    }
}
