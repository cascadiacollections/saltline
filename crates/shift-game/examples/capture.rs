//! Renders review frames to PNG without Godot: the title, then a scripted
//! run played by the solver bot through the real game loop (aim, fire,
//! SHIFT, drafts), saving a frame at each moment worth looking at.
//!
//! `cargo run --release -p shift-game --example capture -- <out dir> [seed]`

use std::io::Write;

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
        if let Mode::Draft { .. } = g.mode {
            if !saw[0] {
                snap(&g, "draft", &mut r, &mut c);
                saw[0] = true;
            }
            step(&mut g, 0.3, vec![Button::One]);
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

/// Minimal PNG writer: RGB, one stored (uncompressed) zlib stream.
fn write_png(path: &str, c: &Canvas) {
    let mut raw = Vec::with_capacity((W * 3 + 1) * H);
    for y in 0..H {
        raw.push(0);
        for x in 0..W {
            let p = c.px[y * W + x];
            raw.extend_from_slice(&[p.0, p.1, p.2]);
        }
    }
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(last as u8);
        let len = chunk.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(W as u32).to_be_bytes());
    ihdr.extend_from_slice(&(H as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut f, b"IHDR", &ihdr);
    chunk(&mut f, b"IDAT", &z);
    chunk(&mut f, b"IEND", &[]);
}

fn chunk(f: &mut std::fs::File, kind: &[u8; 4], data: &[u8]) {
    f.write_all(&(data.len() as u32).to_be_bytes()).unwrap();
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    f.write_all(&crc_in).unwrap();
    f.write_all(&crc32(&crc_in).to_be_bytes()).unwrap();
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
