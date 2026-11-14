//! Renders review frames to PNG without Godot: the title, then a scripted
//! run played by the solver bot through the real game loop (aim, fire,
//! SHIFT, drafts), saving a frame at each moment worth looking at.
//!
//! `cargo run --release -p shift-game --example capture -- <out dir> [seed]`

use shift_game::app::{Mode, Play};
use shift_game::{Button, Canvas, Game, Held, Input, Pointer, Renderer, H, W};
use shift_sim::{bot, Input as RunInput};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "captures".into());
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0x5A17);
    std::fs::create_dir_all(&dir).unwrap();
    let mut g = Game::new();
    g.fixed_seed = Some(seed);
    g.stamp = "review capture".into();
    let mut r = Renderer::new();
    let mut c = Canvas::new();
    let mut n = 0;
    let mut snap = |g: &Game, name: &str, r: &mut Renderer, c: &mut Canvas| {
        r.draw(g, c);
        n += 1;
        let path = format!("{dir}/{n:02}-{name}.png");
        write_png(&path, c);
        println!("{path}");
    };
    let step = |g: &mut Game, secs: f32, pressed: Vec<Button>| {
        g.update(
            1.0 / 60.0,
            &Input {
                held: Held::default(),
                pressed,
                pointer: None,
            },
        );
        let mut t = 0.0;
        while t < secs {
            g.update(1.0 / 60.0, &Input::default());
            t += 1.0 / 60.0;
        }
    };

    step(&mut g, 1.0, vec![]);
    snap(&g, "title", &mut r, &mut c);
    step(&mut g, 4.5, vec![]);
    snap(&g, "title-siege", &mut r, &mut c);
    step(&mut g, 0.1, vec![Button::Fire]);
    snap(&g, "present-aim", &mut r, &mut c);
    // Pointer aiming mid-drag, then a release over the bottom band cancels.
    let at = |g: &mut Game, x: f32, y: f32, down: bool| {
        g.update(
            1.0 / 60.0,
            &Input {
                pointer: Some(Pointer { x, y, down }),
                ..Input::default()
            },
        );
    };
    at(&mut g, 220.0, 70.0, true);
    at(&mut g, 150.0, 60.0, true);
    snap(&g, "drag-aim", &mut r, &mut c);
    at(&mut g, 150.0, 176.0, true);
    at(&mut g, 150.0, 176.0, false);
    step(&mut g, 0.1, vec![Button::Back]);
    snap(&g, "paused", &mut r, &mut c);
    step(&mut g, 0.1, vec![Button::Back]);

    let mut shots = 0;
    let mut saw = [false; 8];
    for _ in 0..600 {
        let Some(p) = g.play() else { break };
        if matches!(g.mode, Mode::Over { .. }) {
            snap(&g, "over", &mut r, &mut c);
            break;
        }
        if let Mode::Draft { play, .. } = &g.mode {
            if !saw[0] {
                snap(&g, "draft", &mut r, &mut c);
                saw[0] = true;
            }
            // Prefer the drafts whose gun marks the review should show.
            let want = match play.run.state() {
                shift_sim::State::Drafting(o) => o
                    .iter()
                    .position(|x| {
                        x.name().eq_ignore_ascii_case("airburst")
                            || x.name().eq_ignore_ascii_case("vernier")
                    })
                    .unwrap_or(0),
                _ => 0,
            };
            let key = [Button::One, Button::Two, Button::Three][want.min(2)];
            step(&mut g, 0.3, vec![key]);
            if g.play()
                .is_some_and(|p| p.era() == shift_sim::Plane::Present)
            {
                snap(&g, "after-draft", &mut r, &mut c);
            }
            continue;
        }
        let Some(next) = bot::next_input(&p.run) else {
            // The run is over: let the ending play out, then the end screen.
            step(&mut g, 0.3, vec![]);
            snap(&g, "ending", &mut r, &mut c);
            step(&mut g, 2.2, vec![]);
            snap(&g, "over", &mut r, &mut c);
            break;
        };
        match next {
            RunInput::Shift => {
                step(&mut g, 0.17, vec![Button::Shift]);
                if !saw[1] {
                    snap(&g, "shift-crossfade", &mut r, &mut c);
                    saw[1] = true;
                }
                step(&mut g, 0.3, vec![]);
                let siege = g.play().is_some_and(|p| p.era() == shift_sim::Plane::Siege);
                if siege && !saw[2] {
                    snap(&g, "siege-aim", &mut r, &mut c);
                    saw[2] = true;
                }
            }
            RunInput::Fire {
                angle_ticks,
                power_ticks,
            } => {
                set_aim(&mut g, angle_ticks, power_ticks);
                step(&mut g, 0.0, vec![Button::Fire]);
                shots += 1;
                let siege = g.play().is_some_and(|p| p.era() == shift_sim::Plane::Siege);
                if shots == 1 || (siege && !saw[3]) {
                    step(&mut g, 0.45, vec![]);
                    snap(
                        &g,
                        if siege {
                            "siege-flight"
                        } else {
                            "present-flight"
                        },
                        &mut r,
                        &mut c,
                    );
                    saw[3] |= siege;
                }
                // Let it land; catch the echo note once.
                for _ in 0..400 {
                    step(&mut g, 0.0, vec![]);
                    if g.play().is_some_and(|p| p.flight.is_none()) {
                        break;
                    }
                }
                let echo = g
                    .play()
                    .is_some_and(|p| p.note.as_ref().is_some_and(|n| n.text.contains("ECHO")));
                if echo && !saw[4] {
                    step(&mut g, 0.1, vec![]);
                    snap(&g, "echo-rises", &mut r, &mut c);
                    saw[4] = true;
                }
                let hold = g.play().is_some_and(|p| p.hold > 0.0);
                if hold && !saw[5] {
                    step(&mut g, 0.2, vec![]);
                    snap(&g, "line-broken", &mut r, &mut c);
                    saw[5] = true;
                }
                if g.play().is_some_and(|p| p.run.stage_index() == 11) && !saw[6] {
                    step(&mut g, 1.2, vec![]);
                    snap(&g, "bastion", &mut r, &mut c);
                    saw[6] = true;
                }
                step(&mut g, 1.2, vec![]);
            }
            other => {
                let _ = other;
                break;
            }
        }
    }
    if let Some(p) = g.play() {
        println!(
            "ended: stage {} shells {} shots {}",
            p.run.stage_index() + 1,
            p.run.shells(),
            p.run.total_shots()
        );
    }
}

fn set_aim(g: &mut Game, a: i32, pw: i32) {
    if let Mode::Play(p) = &mut g.mode {
        let p: &mut Play = p;
        p.angle_ticks = a;
        p.power_ticks = pw;
    }
}

fn write_png(path: &str, c: &Canvas) {
    let rgb: Vec<u8> = c.px.iter().flat_map(|p| [p.0, p.1, p.2]).collect();
    shift_game::png::write(path, W, H, &rgb, 1).unwrap();
}
