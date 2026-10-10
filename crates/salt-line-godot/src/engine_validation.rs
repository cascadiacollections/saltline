//! Fixed replay fixtures driven through WATCH inside an exported engine.
//! No clock, input device, user save or fallback assets enter the comparison.
//! The report compares simulation and imported-asset/software-render output;
//! headless rendering does not prove GPU composition or audible playback.

use crate::app::{Effect, Game, Mode, Sound};
use crate::history::{Record, Watchable};
use crate::input::Intents;
use crate::raster::Canvas;
use crate::render::{self, Art, Cache};
use crate::save_format::Save;
use godot::classes::{AudioStream, Image, ImageTexture, ProjectSettings, ResourceLoader};
use godot::prelude::*;
use salt_line_sim::replay::{state_hash, Hasher};
use std::path::Path;

const FIXTURES: &str = include_str!("engine_validation/replays.txt");
const STEP: f64 = 1.0 / 60.0;
const MAX_FRAMES: usize = 60 * 900;

/// Run through Godot's actual resource loaders, image codec and texture API.
/// A missing resource is a failure even if ordinary play could fall back.
pub fn run(art: &Art, directory: &str) -> Result<(), String> {
    let directory = ProjectSettings::singleton()
        .globalize_path(directory)
        .to_string();
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    // A failed retry must not leave a successful report behind.
    let report = directory.join("report.json");
    if report.exists() {
        std::fs::remove_file(&report).map_err(|e| e.to_string())?;
    }
    if !art.pal.sampled || art.title_background.is_none() {
        return Err("palette or title background failed to load".into());
    }
    let sprites = art.sprites.as_ref().ok_or("sprites were not loaded")?;
    if !sprites.missing.is_empty() {
        return Err(format!("missing sprites: {:?}", sprites.missing));
    }
    let mut audio = Hasher::new();
    for sound in Sound::ALL {
        let path = format!("res://art/audio/{}.wav", sound.file());
        let stream = ResourceLoader::singleton()
            .load(&path)
            .ok_or_else(|| format!("missing {path}"))?
            .try_cast::<AudioStream>()
            .map_err(|_| format!("not an audio stream: {path}"))?;
        let length = stream.get_length();
        if !length.is_finite() || length <= 0.0 {
            return Err(format!("empty or invalid audio: {path}"));
        }
        audio.write_f64(length);
    }
    for (name, sprite) in [
        ("base chassis", sprites.tank_base.as_ref()),
        ("heavy chassis", sprites.tank_ballast.as_ref()),
        ("mobile chassis", sprites.tank_forced.as_ref()),
        ("base barrel", sprites.barrel_base.as_ref()),
        ("heavy barrel", sprites.barrel_ballast.as_ref()),
        ("mobile barrel", sprites.barrel_forced.as_ref()),
        ("sabot barrel", sprites.barrel_sabot.as_ref()),
        ("cluster barrel", sprites.barrel_cluster.as_ref()),
        ("auger barrel", sprites.barrel_auger.as_ref()),
        ("intact tower", sprites.tower_intact.as_ref()),
        ("leaning tower", sprites.tower_leaning.as_ref()),
        ("collapsed tower", sprites.tower_collapsed.as_ref()),
        ("derrick", sprites.derrick.as_ref()),
        ("leaning derrick", sprites.derrick_leaning.as_ref()),
        ("collapsed derrick", sprites.derrick_collapsed.as_ref()),
        ("mast", sprites.radio_mast.as_ref()),
        ("leaning mast", sprites.radio_mast_leaning.as_ref()),
        ("collapsed mast", sprites.radio_mast_collapsed.as_ref()),
        ("silo", sprites.silo.as_ref()),
        ("leaning silo", sprites.silo_leaning.as_ref()),
        ("collapsed silo", sprites.silo_collapsed.as_ref()),
        ("pylon", sprites.pylon.as_ref()),
        ("leaning pylon", sprites.pylon_leaning.as_ref()),
        ("collapsed pylon", sprites.pylon_collapsed.as_ref()),
        ("title", art.title_background.as_ref()),
    ] {
        if sprite.and_then(|s| s.detail.as_ref()).is_none() {
            return Err(format!("missing detailed source artwork: {name}"));
        }
    }
    let rows = suite(art, true, |name, canvas, last| {
        let data = PackedByteArray::from(canvas.bytes());
        let image = Image::create_from_data(
            canvas.pixel_w(),
            canvas.pixel_h(),
            false,
            godot::classes::image::Format::RGBA8,
            &data,
        )
        .ok_or("Image::create_from_data failed")?;
        let mut texture = ImageTexture::create_from_image(&image)
            .ok_or("ImageTexture::create_from_image failed")?;
        texture.update(&image);
        if last {
            let path = directory.join(format!("{name}.png"));
            let error = image.save_png(&GString::from(path.to_string_lossy().as_ref()));
            if error != godot::global::Error::OK {
                return Err(format!("could not save {}: {error:?}", path.display()));
            }
        }
        Ok(())
    })?;
    let text = format!(
        "{{\"format\":1,\"audio_count\":{},\"audio_hash\":\"{:016x}\",\"fixtures\":[{}]}}\n",
        Sound::ALL.len(),
        audio.finish(),
        rows.join(",")
    );
    std::fs::write(report, &text).map_err(|e| e.to_string())?;
    godot_print!("salt-engine-validation: {text}");
    Ok(())
}

fn suite(
    art: &Art,
    detailed: bool,
    mut capture: impl FnMut(&str, &Canvas, bool) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let mut rows = Vec::new();
    for line in FIXTURES
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (name, code) = line.split_once(' ').ok_or("bad fixture line")?;
        let record = Record::from_code(code).map_err(|e| format!("{name}: {e:?}"))?;
        let expected = match record.check().map_err(|e| format!("{name}: {e:?}"))? {
            Watchable::Run(replay, ..) => {
                state_hash(&replay.reconstruct().map_err(|e| format!("{e:?}"))?)
            }
            Watchable::Duel(saved, ..) => {
                crate::match_log::state_hash(&saved.replay().ok_or("match did not replay")?)
            }
        };
        let mut game = Game::new(false, Save::default());
        let intents = Intents {
            window: (1280, 720),
            today: 20_734,
            ..Intents::default()
        };
        game.fit_display(intents.window, (0, 0));
        game.mode = Mode::Replays;
        if game.pasted(code).iter().any(Effect::touches_disk) {
            return Err(format!("{name}: pasting requested a disk write"));
        }
        if !game.watching() {
            return Err(format!("{name}: paste did not start WATCH"));
        }
        let mut cache = Cache::default();
        let mut buttons = Vec::new();
        let mut pixels = Hasher::new();
        let mut sounds = Hasher::new();
        let mut samples = 0;
        let mut completed = false;
        for frame in 0..MAX_FRAMES {
            let effects = game.tick(STEP, &intents);
            if effects.iter().any(Effect::touches_disk) {
                return Err(format!("{name}: WATCH requested a disk write"));
            }
            for effect in effects {
                if let Effect::Play(sound) = effect {
                    sounds.write_usize(sound as usize);
                }
            }
            let watch = game
                .watch
                .as_ref()
                .ok_or_else(|| format!("{name}: WATCH diverged"))?;
            let last = watch.ended;
            if frame % 60 == 0 || last {
                let mut canvas = if detailed {
                    Canvas::detailed(game.display.buffer_w, game.display.buffer_h)
                } else {
                    Canvas::new(game.display.buffer_w, game.display.buffer_h)
                };
                render::frame(&game, art, &mut canvas, &mut buttons, &mut cache);
                pixels.write_usize(frame);
                for &byte in canvas.bytes() {
                    pixels.write_u8(byte);
                }
                capture(name, &canvas, last)?;
                samples += 1;
            }
            if last {
                let actual = if record.is_run() {
                    state_hash(game.play.run.as_ref().ok_or("run disappeared")?)
                } else {
                    crate::match_log::state_hash(
                        &game.versus.live.as_ref().ok_or("match disappeared")?.duel,
                    )
                };
                if actual != expected {
                    return Err(format!(
                        "{name}: final state {actual:016x} != {expected:016x}"
                    ));
                }
                rows.push(format!(
                    "{{\"name\":\"{name}\",\"frames\":{},\"samples\":{samples},\"state_hash\":\"{actual:016x}\",\"frame_hash\":\"{:016x}\",\"sound_hash\":\"{:016x}\"}}",
                    frame + 1, pixels.finish(), sounds.finish()
                ));
                completed = true;
                break;
            }
        }
        if !completed {
            return Err(format!("{name}: exceeded {MAX_FRAMES} frames"));
        }
    }
    if rows.len() != 3 {
        return Err("expected three replay fixtures".into());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_finish_through_watch_without_writing_player_files() {
        let mut captures = 0;
        let rows = suite(&Art::default(), false, |_, _, last| {
            captures += usize::from(last);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(captures, 3);
    }
}
