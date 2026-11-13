//! Counterweight's one Godot node. Everything that decides or draws anything
//! is in `shift-game`; this adapter polls keys, runs the game, plays its
//! sound cues and uploads its frame.
//!
//! Carried over from Salt Line's node, because they paid off there:
//! - one software-rastered 320x180 frame in one `ImageTexture`, updated in
//!   place (`ImageTexture::update`), never recreated;
//! - a frame is rastered and uploaded only when the game says it changed
//!   (dirty + busy, one more frame after busy ends), and menus run at a
//!   30 fps cap;
//! - every sound is an `AudioStreamWav` built from embedded PCM, and every
//!   sprite is embedded too, so there is no import step and no resource the
//!   export filter could drop;
//! - the engine classes used are checked at startup by name.

use godot::classes::audio_stream_wav::{Format as WavFormat, LoopMode};
use godot::classes::image::Format;
use godot::classes::{
    AudioStreamPlayer, AudioStreamWav, ClassDb, Engine, INode2D, Image, ImageTexture, Input,
    Node2D, Os, Time,
};
use godot::global::Key;
use godot::prelude::*;

use shift_game::audio::{pcm, Cue, MIX_RATE};
use shift_game::{Button, Canvas, Effect, Game, Held, Input as GameInput, Renderer, H, W};

struct CounterweightExtension;

#[gdextension]
unsafe impl ExtensionLibrary for CounterweightExtension {}

/// Menus need no more than this; play keeps the project's cap.
const IDLE_FPS: i32 = 30;

/// Classes this node calls. A stripped engine without one fails here, by
/// name, instead of somewhere later.
const CLASSES: &[&str] = &[
    "Image",
    "ImageTexture",
    "AudioStreamWAV",
    "AudioStreamPlayer",
    "Input",
    "Time",
];

/// Buttons and the keys that press them.
const BUTTONS: &[(Button, &[Key])] = &[
    (Button::Up, &[Key::UP, Key::W]),
    (Button::Down, &[Key::DOWN, Key::S]),
    (Button::Left, &[Key::LEFT, Key::A]),
    (Button::Right, &[Key::RIGHT, Key::D]),
    (Button::Fire, &[Key::SPACE, Key::ENTER, Key::KP_ENTER]),
    (Button::Shift, &[Key::SHIFT, Key::TAB]),
    (Button::Back, &[Key::ESCAPE]),
    (Button::One, &[Key::KEY_1]),
    (Button::Two, &[Key::KEY_2]),
    (Button::Three, &[Key::KEY_3]),
];

#[derive(GodotClass)]
#[class(base = Node2D)]
struct Counterweight {
    base: Base<Node2D>,
    game: Game,
    renderer: Renderer,
    canvas: Canvas,
    image: Option<Gd<Image>>,
    texture: Option<Gd<ImageTexture>>,
    bytes: PackedByteArray,
    players: Vec<Gd<AudioStreamPlayer>>,
    down: Vec<bool>,
    project_fps: i32,
    was_busy: bool,
    drawn_once: bool,
}

#[godot_api]
impl INode2D for Counterweight {
    fn init(base: Base<Node2D>) -> Self {
        let mut game = Game::new();
        game.entropy = Time::singleton().get_unix_time_from_system().to_bits()
            ^ Time::singleton().get_ticks_usec();
        Counterweight {
            base,
            game,
            renderer: Renderer::new(),
            canvas: Canvas::new(),
            image: None,
            texture: None,
            bytes: PackedByteArray::new(),
            players: Vec::new(),
            down: vec![false; BUTTONS.len()],
            project_fps: 0,
            was_busy: false,
            drawn_once: false,
        }
    }

    fn ready(&mut self) {
        let args: Vec<String> = Os::singleton()
            .get_cmdline_user_args()
            .to_vec()
            .iter()
            .map(|a| a.to_string())
            .collect();
        self.game.autoplay = args.iter().any(|a| a == "--cw-autoplay");
        if let Some(i) = args.iter().position(|a| a == "--cw-seed") {
            self.game.fixed_seed = args.get(i + 1).and_then(|s| s.parse().ok());
        }
        let db = ClassDb::singleton();
        let missing: Vec<&str> = CLASSES
            .iter()
            .copied()
            .filter(|c| !db.class_exists(&StringName::from(*c)))
            .collect();
        if !missing.is_empty() {
            godot_error!(
                "counterweight: this engine build lacks {}",
                missing.join(", ")
            );
        }
        self.project_fps = Engine::singleton().get_max_fps();
        self.bytes.resize(W * H * 4);
        let image = Image::create_empty(W as i32, H as i32, false, Format::RGBA8);
        if let Some(image) = image {
            self.texture = ImageTexture::create_from_image(&image);
            self.image = Some(image);
        }
        for cue in Cue::ALL {
            let data = PackedByteArray::from(pcm(cue).as_slice());
            let frames = (data.len() / 2) as i32;
            let mut wav = AudioStreamWav::new_gd();
            wav.set_format(WavFormat::FORMAT_16_BITS);
            wav.set_mix_rate(MIX_RATE as i32);
            wav.set_stereo(false);
            wav.set_data(&data);
            if cue.looping() {
                wav.set_loop_mode(LoopMode::FORWARD);
                wav.set_loop_end(frames);
            }
            let mut player = AudioStreamPlayer::new_alloc();
            player.set_stream(&wav);
            if cue == Cue::Title {
                player.set_volume_db(-6.0);
            }
            self.base_mut().add_child(&player);
            self.players.push(player);
        }
        self.play(Cue::Title);
    }

    fn process(&mut self, delta: f64) {
        let input = self.poll();
        let fx = self.game.update(delta as f32, &input);
        for e in fx {
            match e {
                Effect::Play(c) => self.play(c),
                Effect::Music(on) => {
                    if on {
                        self.play(Cue::Title);
                    } else if let Some(p) = self.players.get_mut(Cue::Title.index()) {
                        p.stop();
                    }
                }
                Effect::Quit => {
                    if !Os::singleton().has_feature("web") {
                        self.base().get_tree().quit();
                    }
                }
            }
        }
        let cap = if self.game.on_menu() && self.project_fps != 0 {
            self.project_fps.min(IDLE_FPS)
        } else if self.game.on_menu() {
            IDLE_FPS
        } else {
            self.project_fps
        };
        if Engine::singleton().get_max_fps() != cap {
            Engine::singleton().set_max_fps(cap);
        }
        // Draw when due, plus one frame after something stops moving so
        // nothing that ran out is left frozen on screen.
        let busy = self.game.needs_redraw();
        if busy || self.was_busy || !self.drawn_once {
            self.upload();
        }
        self.was_busy = busy;
    }

    fn draw(&mut self) {
        if let Some(t) = self.texture.clone() {
            self.base_mut().draw_texture_rect(
                &t,
                Rect2::new(Vector2::ZERO, Vector2::new(W as f32, H as f32)),
                false,
            );
        }
    }
}

impl Counterweight {
    fn poll(&mut self) -> GameInput {
        let input = Input::singleton();
        let mut out = GameInput::default();
        for (i, (b, keys)) in BUTTONS.iter().enumerate() {
            let now = keys.iter().any(|k| input.is_key_pressed(*k));
            if now && !self.down[i] {
                out.pressed.push(*b);
            }
            self.down[i] = now;
        }
        let is = |b: Button| {
            BUTTONS
                .iter()
                .position(|(x, _)| *x == b)
                .is_some_and(|i| self.down[i])
        };
        out.held = Held {
            up: is(Button::Up),
            down: is(Button::Down),
            left: is(Button::Left),
            right: is(Button::Right),
            fine: input.is_key_pressed(Key::Z),
        };
        out
    }

    fn play(&mut self, cue: Cue) {
        if let Some(p) = self.players.get_mut(cue.index()) {
            p.play();
        }
    }

    fn upload(&mut self) {
        self.renderer.draw(&self.game, &mut self.canvas);
        {
            let dst = self.bytes.as_mut_slice();
            for (i, p) in self.canvas.px.iter().enumerate() {
                dst[i * 4..i * 4 + 4].copy_from_slice(&[p.0, p.1, p.2, 255]);
            }
        }
        if let (Some(img), Some(tex)) = (self.image.as_mut(), self.texture.as_mut()) {
            img.set_data(W as i32, H as i32, false, Format::RGBA8, &self.bytes);
            tex.update(&*img);
        }
        self.game.drawn();
        self.drawn_once = true;
        self.base_mut().queue_redraw();
    }
}
