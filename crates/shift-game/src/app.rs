//! The game: modes, aiming, the shot in flight, the SHIFT crossfade, the
//! draft, the end screen. Engine-free: the Godot node feeds it [`Input`] and
//! elapsed time, plays the [`Effect`]s it returns, and uploads what
//! [`crate::render`] paints.

use salt_line_sim::ballistics::{End, Trace};
use salt_line_sim::dmath::MAX_ANGLE_TICKS;
use salt_line_sim::run::shot;
use salt_line_sim::{Vec2, DT, POWER_TICKS_MAX, SCREEN_W_REF};
use shift_sim::expedition::STAGES;
use shift_sim::{Event, Expedition, Input as RunInput, Plane, Stage, State};

use crate::audio::Cue;
use crate::H;

/// One frame's input. `held` is what is down now; `pressed` what went down
/// since the last frame.
#[derive(Clone, Default, Debug)]
pub struct Input {
    pub held: Held,
    pub pressed: Vec<Button>,
    /// Mouse or touch, if the engine has one.
    pub pointer: Option<Pointer>,
}

/// The mouse (or a touch, which engines report as the mouse) in frame
/// pixels: 0..W across, 0..H down, as drawn.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub down: bool,
}

/// A pointer this frame, with its edges worked out by [`Game`].
#[derive(Clone, Copy, Debug)]
struct Touch {
    x: f32,
    y: f32,
    down: bool,
    pressed: bool,
    released: bool,
}

/// Screen regions that answer the pointer. The renderer draws its buttons
/// in the same rectangles.
pub mod hit {
    use crate::{H, W};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Rect {
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    impl Rect {
        pub fn contains(&self, x: f32, y: f32) -> bool {
            x >= self.x as f32
                && y >= self.y as f32
                && x < (self.x + self.w) as f32
                && y < (self.y + self.h) as f32
        }
    }

    /// The SHIFT button in the bottom band, sized to its label (as drawn).
    pub fn shift(label: &str) -> Option<Rect> {
        if label.is_empty() {
            return None;
        }
        let w = crate::font::width(label, 1) + 6;
        Some(Rect {
            x: W as i32 - 2 - w,
            y: H as i32 - 11,
            w: w + 2,
            h: 11,
        })
    }
    /// The pause button at the right end of the top band.
    pub const PAUSE: Rect = Rect {
        x: W as i32 - 13,
        y: 0,
        w: 13,
        h: 10,
    };
    /// Where a drag can aim: between the bands.
    pub const FIELD: Rect = Rect {
        x: 0,
        y: 10,
        w: W as i32,
        h: H as i32 - 21,
    };

    pub const CARD_W: i32 = 96;
    pub const CARD_GAP: i32 = 6;

    /// Draft card `i` of `n`.
    pub fn card(i: usize, n: usize) -> Rect {
        let total = n as i32 * CARD_W + (n as i32 - 1) * CARD_GAP;
        Rect {
            x: 160 - total / 2 + i as i32 * (CARD_W + CARD_GAP),
            y: 40,
            w: CARD_W,
            h: 92,
        }
    }

    /// The draft's skip button.
    pub const SKIP: Rect = Rect {
        x: 130,
        y: 150,
        w: 60,
        h: 13,
    };

    /// Pause menu item `i`.
    pub fn pause_item(i: usize) -> Rect {
        Rect {
            x: 104,
            y: 74 + i as i32 * 16,
            w: 112,
            h: 13,
        }
    }
}

/// A pointer drag in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// The aim before the drag, restored if it is cancelled.
    pub aim: (i32, i32),
    /// Where the pointer went down.
    pub from: (f32, f32),
    /// Left the dead zone: the drag is aiming.
    pub moved: bool,
}

/// Frame pixels a press must travel before it aims (and can fire): a tap
/// on the field does nothing.
pub const DRAG_DEAD_ZONE: f32 = 4.0;

/// Pause menu entries, top to bottom.
pub const PAUSE_ITEMS: [&str; 2] = ["RESUME", "ABANDON RUN"];

/// Pointer aiming: power 0 at this many pixels from the muzzle...
pub const AIM_NEAR: f64 = 12.0;
/// ...and full power this many further out.
pub const AIM_SPAN: f64 = 150.0;

#[derive(Clone, Copy, Default, Debug)]
pub struct Held {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// Fine aim: slower steps while held.
    pub fine: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    /// Fire in play; select in menus. Space or Enter.
    Fire,
    /// Swap eras. Shift or Tab.
    Shift,
    /// Esc.
    Back,
    One,
    Two,
    Three,
}

/// What the engine should do for the game.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Effect {
    Play(Cue),
    /// Title music on or off. On also stops any ambience.
    Music(bool),
    /// Play this era's ambient loop (and stop the other's); `None` stops
    /// both (run over, paused).
    Ambience(Option<Plane>),
    /// Quit the application (title screen, Esc).
    Quit,
    /// A new best score: the engine keeps it for next time.
    SaveBest(u32),
}

/// A shell (or stone) on its way, drawn over the stage as it was before the
/// shot; the run has already resolved it.
#[derive(Clone, Debug)]
pub struct Flight {
    pub era: Plane,
    pub traces: Vec<Trace>,
    /// Seconds since release.
    pub t: f64,
    /// The events the shot produced, released on landing.
    pub events: Vec<Event>,
}

/// Animation speed of a flight relative to sim time.
pub const FLIGHT_SPEED: f64 = 1.7;
/// Seconds the SHIFT crossfade takes.
pub const FADE_SECS: f32 = 0.35;
/// Seconds the cleared stage stays on screen before the next appears.
pub const HOLD_SECS: f32 = 1.1;

impl Flight {
    /// Index into each trace's path for the current time.
    pub fn step(&self) -> usize {
        (self.t * FLIGHT_SPEED / DT) as usize
    }
    pub fn landed(&self) -> bool {
        self.traces.iter().all(|t| self.step() + 1 >= t.path.len())
    }
}

/// A blast being drawn: where, in which era, how long ago.
#[derive(Clone, Copy, Debug)]
pub struct Blast {
    pub era: Plane,
    pub pos: Vec2,
    pub t: f32,
    pub big: bool,
}

/// A line of text that shows for a while in the middle of the screen.
#[derive(Clone, Debug)]
pub struct Note {
    pub text: String,
    pub t: f32,
}

/// Play state: the run and everything about how it is being shown.
#[derive(Clone)]
pub struct Play {
    pub run: Expedition,
    /// The stage to draw instead of the run's, while a shot flies (as it was)
    /// or a cleared stage is held (as it ended).
    pub view: Option<Stage>,
    pub hold: f32,
    pub flight: Option<Flight>,
    /// SHIFT crossfade: era faded from and progress 0..=1.
    pub fade: Option<(Plane, f32)>,
    pub angle_ticks: i32,
    pub power_ticks: i32,
    /// Seconds of play, for the score's time bonus.
    pub elapsed: f64,
    pub blasts: Vec<Blast>,
    pub note: Option<Note>,
    /// The last flight path in each era, kept faint as a chalk line.
    pub last_path: [Vec<Vec2>; 2],
    /// Auto-repeat timer for held aim keys.
    repeat: f32,
    held_for: f32,
    /// Screen shake: seconds left and strength in pixels.
    pub shake: (f32, f32),
    /// Paused, with this menu item selected.
    pub paused: Option<usize>,
    /// A pointer is down on the field: a drag aims, and releasing over the
    /// field fires. A press that never leaves [`DRAG_DEAD_ZONE`] is a tap,
    /// which fires nothing.
    pub drag: Option<Drag>,
    /// After the run ends, seconds until the end screen.
    pub ending: Option<f32>,
}

#[derive(Clone)]
pub enum Mode {
    Title {
        t: f32,
    },
    Play(Box<Play>),
    /// `armed`: `sel` was chosen by a tap, so tapping it again takes it.
    Draft {
        play: Box<Play>,
        sel: usize,
        armed: bool,
    },
    Over {
        play: Box<Play>,
        victory: bool,
    },
}

pub struct Game {
    pub mode: Mode,
    /// Supplied by the engine (wall clock); picks each run's seed.
    pub entropy: u64,
    /// A fixed seed for every run, for tests and review captures.
    pub fixed_seed: Option<u64>,
    /// Build stamp shown on the title, e.g. "gameoff-v1.0.0 1a2b3c4".
    pub stamp: String,
    /// Frames always differ while this is true (something animates).
    dirty: bool,
    /// The solver bot plays, through the same buttons a player presses:
    /// attract mode and engine-side review captures (`--cw-autoplay`).
    pub autoplay: bool,
    auto_wait: f32,
    ptr_was_down: bool,
    /// Running in a browser: no quit, no F11 (the host page has fullscreen).
    pub web: bool,
    /// Best score so far; the engine loads it at start.
    pub best: u32,
    /// The run on the end screen set [`Game::best`].
    pub new_best: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub fn new() -> Self {
        Game {
            mode: Mode::Title { t: 0.0 },
            entropy: 0x00C0_FFEE,
            fixed_seed: None,
            stamp: option_env!("CW_BUILD").unwrap_or("dev build").to_string(),
            dirty: true,
            autoplay: false,
            auto_wait: 0.0,
            ptr_was_down: false,
            web: false,
            best: 0,
            new_best: false,
        }
    }

    /// The buttons the autopilot presses this frame.
    fn autopilot(&mut self, dt: f32) -> Vec<Button> {
        self.auto_wait -= dt;
        if self.auto_wait > 0.0 {
            return Vec::new();
        }
        let wait = |me: &mut Self, s: f32, b: Button| {
            me.auto_wait = s;
            vec![b]
        };
        match &mut self.mode {
            Mode::Title { t } if *t > 2.0 => wait(self, 0.5, Button::Fire),
            Mode::Title { .. } => Vec::new(),
            Mode::Draft { .. } => wait(self, 1.2, Button::One),
            Mode::Over { .. } => wait(self, 4.0, Button::Fire),
            Mode::Play(p) => {
                if p.flight.is_some() || p.view.is_some() || p.fade.is_some() || p.ending.is_some()
                {
                    return Vec::new();
                }
                match shift_sim::bot::next_input(&p.run) {
                    Some(RunInput::Shift) => wait(self, 0.6, Button::Shift),
                    Some(RunInput::Fire {
                        angle_ticks,
                        power_ticks,
                    }) => {
                        // Swing the gun toward the solution, then fire.
                        let (da, dp) = (angle_ticks - p.angle_ticks, power_ticks - p.power_ticks);
                        if da == 0 && dp == 0 {
                            return wait(self, 0.35, Button::Fire);
                        }
                        p.angle_ticks += da.signum() * da.abs().min(20);
                        p.power_ticks += dp.signum() * dp.abs().min(14);
                        self.dirty = true;
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
        }
    }

    /// Whether the next frame must be redrawn: something animates, or input
    /// or state changed since the last draw (frame pacing, as in Salt Line:
    /// an unchanged frame is never rastered or uploaded).
    pub fn needs_redraw(&self) -> bool {
        self.dirty || self.busy()
    }

    /// Something moves on its own.
    pub fn busy(&self) -> bool {
        match &self.mode {
            Mode::Title { .. } => true, // the era dissolve behind the title
            // Paused: the frame is frozen; input marks it dirty.
            Mode::Play(p) if p.paused.is_some() => false,
            Mode::Play(p) => {
                p.flight.is_some()
                    || p.fade.is_some()
                    || !p.blasts.is_empty()
                    || p.note.is_some()
                    || p.hold > 0.0
                    || p.shake.0 > 0.0
                    || p.ending.is_some()
            }
            Mode::Draft { .. } | Mode::Over { .. } => false,
        }
    }

    /// Menus can run at a low frame cap; play wants the display's rate.
    pub fn on_menu(&self) -> bool {
        !matches!(self.mode, Mode::Play(_))
    }

    /// Called after a frame was drawn.
    pub fn drawn(&mut self) {
        self.dirty = false;
    }

    pub fn update(&mut self, dt: f32, input: &Input) -> Vec<Effect> {
        let auto;
        let input = if self.autoplay {
            auto = Input {
                held: Held::default(),
                pressed: self.autopilot(dt),
                pointer: None,
            };
            &auto
        } else {
            input
        };
        let mut fx = Vec::new();
        let h = input.held;
        let touch = input.pointer.map(|p| Touch {
            x: p.x,
            y: p.y,
            down: p.down,
            pressed: p.down && !self.ptr_was_down,
            released: !p.down && self.ptr_was_down,
        });
        self.ptr_was_down = input.pointer.is_some_and(|p| p.down);
        if !input.pressed.is_empty()
            || h.up
            || h.down
            || h.left
            || h.right
            || touch.is_some_and(|t| t.down || t.released)
        {
            self.dirty = true;
        }
        // A tap on a menu screen is a button press there.
        let tapped = touch.filter(|t| t.pressed);
        let mode = std::mem::replace(&mut self.mode, Mode::Title { t: 0.0 });
        let was_over = matches!(mode, Mode::Over { .. });
        self.mode = match mode {
            Mode::Title { t } => {
                let input = with_tap(input, tapped.map(|_| Button::Fire));
                self.title(t + dt, &input, &mut fx)
            }
            Mode::Play(p) => play_update(p, dt, input, touch, &mut fx),
            Mode::Draft {
                play,
                mut sel,
                mut armed,
            } => {
                let n = match play.run.state() {
                    State::Drafting(o) => o.len(),
                    _ => 0,
                };
                // Tap a card to select it, tap it again to take it. A card
                // selected by default or by the keys still takes two taps.
                let mut b = None;
                if let Some(t) = tapped {
                    if hit::SKIP.contains(t.x, t.y) {
                        b = Some(Button::Back);
                    } else if let Some(i) = (0..n).find(|&i| hit::card(i, n).contains(t.x, t.y)) {
                        if armed && i == sel {
                            b = Some(Button::Fire);
                        } else {
                            sel = i;
                            armed = true;
                            fx.push(Effect::Play(Cue::Ui));
                        }
                    }
                }
                draft_update(play, sel, armed, &with_tap(input, b), &mut fx)
            }
            Mode::Over { play, victory } => {
                if tapped.is_some()
                    || input.pressed.contains(&Button::Fire)
                    || input.pressed.contains(&Button::Back)
                {
                    fx.push(Effect::Play(Cue::Ui));
                    fx.push(Effect::Music(true));
                    Mode::Title { t: 0.0 }
                } else {
                    Mode::Over { play, victory }
                }
            }
        };
        if let (Mode::Over { play, .. }, false) = (&self.mode, was_over) {
            let score = play.run.score(play.elapsed as u32);
            self.new_best = score > self.best;
            if self.new_best {
                self.best = score;
                fx.push(Effect::SaveBest(score));
            }
        }
        if !fx.is_empty() {
            self.dirty = true;
        }
        fx
    }

    fn title(&mut self, t: f32, input: &Input, fx: &mut Vec<Effect>) -> Mode {
        if input.pressed.contains(&Button::Fire) {
            fx.push(Effect::Play(Cue::Ui));
            fx.push(Effect::Music(false));
            fx.push(Effect::Ambience(Some(Plane::Present)));
            let seed = self
                .fixed_seed
                .unwrap_or_else(|| salt_line_sim::rng::mix64(self.entropy ^ (t.to_bits() as u64)));
            return Mode::Play(Box::new(Play::new(seed)));
        }
        if input.pressed.contains(&Button::Back) {
            fx.push(Effect::Quit);
        }
        Mode::Title { t }
    }

    /// The play state, if a run is on screen.
    pub fn play(&self) -> Option<&Play> {
        match &self.mode {
            Mode::Play(p) | Mode::Draft { play: p, .. } | Mode::Over { play: p, .. } => Some(p),
            Mode::Title { .. } => None,
        }
    }

    /// Test and capture hook: a run on `seed` already in play.
    pub fn start(&mut self, seed: u64) {
        self.mode = Mode::Play(Box::new(Play::new(seed)));
        self.dirty = true;
    }
}

impl Play {
    pub fn new(seed: u64) -> Self {
        Play {
            run: Expedition::new(seed, SCREEN_W_REF),
            view: None,
            hold: 0.0,
            flight: None,
            fade: None,
            angle_ticks: 900,
            power_ticks: 560,
            elapsed: 0.0,
            blasts: Vec::new(),
            note: None,
            last_path: [Vec::new(), Vec::new()],
            repeat: 0.0,
            held_for: 0.0,
            shake: (0.0, 0.0),
            paused: None,
            drag: None,
            ending: None,
        }
    }

    /// The stage to draw.
    pub fn shown(&self) -> &Stage {
        self.view.as_ref().unwrap_or(self.run.stage())
    }

    /// The era on screen (during a fade, the one being faded to).
    pub fn era(&self) -> Plane {
        self.shown().active()
    }

    /// Whether SHIFT would be accepted now.
    pub fn can_shift(&self) -> bool {
        self.flight.is_none()
            && self.view.is_none()
            && self.ending.is_none()
            && self.run.can_shift()
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.note = Some(Note {
            text: s.into(),
            t: 0.0,
        });
    }

    fn aim(&mut self, dt: f32, h: Held) {
        let any = h.up || h.down || h.left || h.right;
        if !any {
            self.held_for = 0.0;
            self.repeat = 0.0;
            return;
        }
        // First step at once, then repeat, faster the longer it's held.
        let first = self.held_for == 0.0;
        self.held_for += dt;
        self.repeat -= dt;
        if !first && self.repeat > 0.0 {
            return;
        }
        let fast = self.held_for > 0.6;
        self.repeat = if fast {
            0.02
        } else if first {
            0.25
        } else {
            0.06
        };
        let q = self.run.active_loadout().angle_quantum_ticks.max(1);
        let a_step = if h.fine {
            q
        } else if fast {
            q * 4
        } else {
            q * 2
        };
        let p_step = if h.fine {
            2
        } else if fast {
            12
        } else {
            5
        };
        if h.up {
            self.angle_ticks += a_step;
        }
        if h.down {
            self.angle_ticks -= a_step;
        }
        if h.right {
            self.power_ticks += p_step;
        }
        if h.left {
            self.power_ticks -= p_step;
        }
        self.angle_ticks = self.angle_ticks.clamp(0, MAX_ANGLE_TICKS);
        self.power_ticks = self.power_ticks.clamp(0, POWER_TICKS_MAX);
    }

    /// A pointer drag is aiming (out of its dead zone).
    pub fn dragging(&self) -> bool {
        self.drag.is_some_and(|d| d.moved)
    }

    /// Aim at a point on screen: the angle from the muzzle toward it, the
    /// power from how far away it is (see [`AIM_NEAR`], [`AIM_SPAN`]).
    pub fn aim_at(&mut self, x: f32, y: f32) {
        let m = self.shown().screen(self.era()).muzzle();
        let (dx, dy) = (x as f64 - m.x, (H as f64 - y as f64) - m.y);
        let q = self.run.active_loadout().angle_quantum_ticks.max(1);
        let deg = dy.atan2(dx).to_degrees().clamp(0.0, 90.0);
        let ticks = (deg / 0.05 / q as f64).round() as i32 * q;
        self.angle_ticks = ticks.clamp(0, MAX_ANGLE_TICKS);
        let d = dx.hypot(dy);
        let pw = (d - AIM_NEAR) / AIM_SPAN * POWER_TICKS_MAX as f64;
        self.power_ticks = (pw.round() as i32).clamp(0, POWER_TICKS_MAX);
    }

    fn fire(&mut self, fx: &mut Vec<Effect>) {
        let era = self.run.stage().active();
        let before = self.run.stage().clone();
        let input = RunInput::Fire {
            angle_ticks: self.angle_ticks,
            power_ticks: self.power_ticks,
        };
        let Some(events) = self.run.apply(input) else {
            return;
        };
        let traces = events
            .iter()
            .find_map(|e| match e {
                Event::Fired(r) => Some(r.active.traces.clone()),
                _ => None,
            })
            .unwrap_or_default();
        fx.push(Effect::Play(if era == Plane::Siege {
            Cue::Throw
        } else {
            Cue::Fire
        }));
        self.view = Some(before);
        self.flight = Some(Flight {
            era,
            traces,
            t: 0.0,
            events,
        });
    }

    fn shift(&mut self, fx: &mut Vec<Effect>) {
        if !self.can_shift() {
            return;
        }
        let from = self.run.stage().active();
        if let Some(events) = self.run.apply(RunInput::Shift) {
            fx.push(Effect::Play(Cue::Shift));
            fx.push(Effect::Ambience(Some(from.other())));
            self.fade = Some((from, 0.0));
            let cost = events.iter().find_map(|e| match e {
                Event::Shifted { cost, .. } => Some(*cost),
                _ => None,
            });
            if cost.unwrap_or(0) > 0 {
                self.note("PLATING DRAINS 1 SHELL");
            }
            self.resolve(events, fx);
        }
    }

    /// Land a flight: blasts, sounds, then what the shot did to the run.
    fn land(&mut self, fx: &mut Vec<Effect>) {
        let Some(f) = self.flight.take() else { return };
        let mut struck = false;
        for tr in &f.traces {
            if let Some(p) = tr.detonation() {
                self.blasts.push(Blast {
                    era: f.era,
                    pos: p,
                    t: 0.0,
                    big: f.era == Plane::Siege,
                });
            }
            struck |= matches!(tr.end, End::Structure { .. });
        }
        if let Some(tr) = f.traces.first() {
            self.last_path[f.era.index()] = tr.path.clone();
        }
        let fell = f.events.iter().any(|e| match e {
            Event::Fired(r) => {
                !r.active.collapsed.is_empty()
                    || r.phased.as_ref().is_some_and(|p| !p.collapsed.is_empty())
            }
            _ => false,
        });
        // Shake: a fall shakes most, a stone more than a shell.
        let heavy = if f.era == Plane::Siege { 1.0 } else { 0.0 };
        if fell {
            self.shake = (0.35, 2.0 + heavy);
        } else if struck {
            self.shake = (0.2, 1.0 + heavy);
        }
        fx.push(Effect::Play(if fell {
            Cue::Collapse
        } else if struck {
            Cue::StructureHit
        } else {
            Cue::TerrainHit
        }));
        self.view = None;
        self.resolve(f.events, fx);
    }

    /// React to run events after they have been shown.
    fn resolve(&mut self, events: Vec<Event>, fx: &mut Vec<Effect>) {
        for e in &events {
            match e {
                Event::Fired(r) => {
                    let n = r.echoes.len();
                    if n > 0 {
                        let there = r.echoes[0].0;
                        let year = if there == Plane::Siege {
                            "1347"
                        } else {
                            "THE PRESENT"
                        };
                        self.note(if n == 1 {
                            format!("AN ECHO RISES IN {year}")
                        } else {
                            format!("{n} ECHOES RISE IN {year}")
                        });
                    }
                }
                Event::StageCleared { refund } => {
                    // Keep the finished stage up for a moment: rebuild it as it
                    // ended from the stage before the shot.
                    self.hold = HOLD_SECS;
                    self.note(if *refund > 0 {
                        format!("LINE BROKEN  +{refund} SHELLS")
                    } else {
                        "LINE BROKEN".to_string()
                    });
                    self.last_path = [Vec::new(), Vec::new()];
                }
                Event::Victory => {
                    fx.push(Effect::Ambience(None));
                    fx.push(Effect::Play(Cue::Victory));
                    self.note("THE BASTION FALLS");
                    self.ending = Some(2.0);
                }
                Event::Defeat => {
                    fx.push(Effect::Ambience(None));
                    fx.push(Effect::Play(Cue::Defeat));
                    self.note("OUT OF SHELLS");
                    self.ending = Some(2.0);
                }
                Event::Shifted { .. } | Event::Drafted(_) => {}
            }
        }
    }
}

/// `input` with one more button pressed, if any.
fn with_tap(input: &Input, b: Option<Button>) -> Input {
    let mut out = input.clone();
    out.pressed.extend(b);
    out
}

fn pause_update(
    mut p: Box<Play>,
    input: &Input,
    touch: Option<Touch>,
    fx: &mut Vec<Effect>,
) -> Mode {
    let n = PAUSE_ITEMS.len();
    let mut sel = p.paused.unwrap_or(0);
    let mut chosen = None;
    for b in &input.pressed {
        match b {
            Button::Up | Button::Left => {
                sel = (sel + n - 1) % n;
                fx.push(Effect::Play(Cue::Ui));
            }
            Button::Down | Button::Right => {
                sel = (sel + 1) % n;
                fx.push(Effect::Play(Cue::Ui));
            }
            Button::Fire => chosen = Some(sel),
            Button::Back => chosen = Some(0),
            _ => {}
        }
    }
    if let Some(t) = touch.filter(|t| t.pressed) {
        chosen = (0..n).find(|&i| hit::pause_item(i).contains(t.x, t.y));
        if chosen.is_none() && hit::PAUSE.contains(t.x, t.y) {
            chosen = Some(0);
        }
    }
    match chosen {
        Some(0) => {
            fx.push(Effect::Play(Cue::Ui));
            p.paused = None;
            if p.ending.is_none() {
                fx.push(Effect::Ambience(Some(p.era())));
            }
        }
        Some(_) => {
            fx.push(Effect::Play(Cue::Ui));
            fx.push(Effect::Music(true));
            return Mode::Title { t: 0.0 };
        }
        None => p.paused = Some(sel),
    }
    Mode::Play(p)
}

fn play_update(
    mut p: Box<Play>,
    dt: f32,
    input: &Input,
    touch: Option<Touch>,
    fx: &mut Vec<Effect>,
) -> Mode {
    if p.paused.is_some() {
        return pause_update(p, input, touch, fx);
    }
    p.elapsed += dt as f64;
    p.shake.0 = (p.shake.0 - dt).max(0.0);
    for b in &mut p.blasts {
        b.t += dt;
    }
    p.blasts.retain(|b| b.t < 0.7);
    if let Some(n) = &mut p.note {
        n.t += dt;
        if n.t > 2.2 {
            p.note = None;
        }
    }
    if let Some((from, t)) = p.fade {
        let t = t + dt / FADE_SECS;
        p.fade = if t >= 1.0 { None } else { Some((from, t)) };
    }
    if p.hold > 0.0 {
        p.hold = (p.hold - dt).max(0.0);
    }

    if let Some(f) = &mut p.flight {
        f.t += dt as f64;
        if f.landed() {
            let before = p.view.clone();
            p.land(fx);
            // A stage that just cleared: show it as it ended for the hold.
            if p.hold > 0.0 {
                p.view = Some(ended_view(&p, before));
            }
        }
    } else if p.hold <= 0.0 && p.view.is_some() {
        p.view = None;
        // The next stage is up, and stages open in the present: the
        // ambience follows if the cleared one ended in 1347.
        if p.ending.is_none() {
            fx.push(Effect::Ambience(Some(p.era())));
        }
    }

    if let Some(left) = p.ending {
        let left = left - dt;
        if left <= 0.0 {
            let victory = *p.run.state() == State::Victory;
            return Mode::Over { play: p, victory };
        }
        p.ending = Some(left);
        return Mode::Play(p);
    }

    if matches!(p.run.state(), State::Drafting(_)) && p.flight.is_none() && p.hold <= 0.0 {
        fx.push(Effect::Play(Cue::Draft));
        return Mode::Draft {
            play: p,
            sel: 0,
            armed: false,
        };
    }

    let idle = p.flight.is_none() && p.view.is_none();
    if idle && !p.dragging() {
        p.aim(dt, input.held);
    }
    let mut pressed = input.pressed.clone();
    if let Some(t) = touch {
        if t.pressed && hit::PAUSE.contains(t.x, t.y) {
            pressed.push(Button::Back);
        } else if t.pressed && hit::shift(&shift_label(&p)).is_some_and(|r| r.contains(t.x, t.y)) {
            pressed.push(Button::Shift);
        } else if t.pressed && idle && hit::FIELD.contains(t.x, t.y) {
            p.drag = Some(Drag {
                aim: (p.angle_ticks, p.power_ticks),
                from: (t.x, t.y),
                moved: false,
            });
        }
        if let Some(mut d) = p.drag {
            let (a, pw) = d.aim;
            d.moved |= (t.x - d.from.0).hypot(t.y - d.from.1) > DRAG_DEAD_ZONE;
            p.drag = Some(d);
            if !idle {
                p.drag = None;
            } else if t.down {
                // Over the bands the drag would cancel: show the old aim.
                if d.moved && hit::FIELD.contains(t.x, t.y) {
                    p.aim_at(t.x, t.y);
                } else {
                    (p.angle_ticks, p.power_ticks) = (a, pw);
                }
            } else if t.released {
                p.drag = None;
                // A tap, or a release over the bands (or off the frame),
                // cancels.
                if d.moved && hit::FIELD.contains(t.x, t.y) {
                    p.aim_at(t.x, t.y);
                    pressed.push(Button::Fire);
                } else {
                    (p.angle_ticks, p.power_ticks) = (a, pw);
                }
            }
        }
    } else if let Some(d) = p.drag.take() {
        (p.angle_ticks, p.power_ticks) = d.aim;
    }
    for b in &pressed {
        match b {
            Button::Fire if idle => p.fire(fx),
            Button::Shift => p.shift(fx),
            Button::Back => {
                fx.push(Effect::Play(Cue::Ui));
                if let Some(d) = p.drag.take() {
                    (p.angle_ticks, p.power_ticks) = d.aim;
                }
                p.paused = Some(0);
                fx.push(Effect::Ambience(None));
                return Mode::Play(p);
            }
            _ => {}
        }
    }
    Mode::Play(p)
}

/// The stage as the clearing shot left it. The run has moved on to the next
/// stage, so this is the pre-shot view with the same shot fired into it:
/// deterministic, so identical to what the run resolved.
fn ended_view(p: &Play, before: Option<Stage>) -> Stage {
    let mut s = before.unwrap_or_else(|| p.run.stage().clone());
    let Some(RunInput::Fire {
        angle_ticks,
        power_ticks,
    }) = p
        .run
        .log()
        .iter()
        .rev()
        .find(|i| matches!(i, RunInput::Fire { .. }))
        .copied()
    else {
        return s;
    };
    let lo = s.active().loadout(p.run.base_loadout());
    let half = p.run.holds(shift_sim::PhaseMod::HalfPhased);
    s.fire(
        shot(angle_ticks, power_ticks).quantized(&lo),
        p.run.base_loadout(),
        half,
        false,
    );
    s
}

fn draft_update(
    mut p: Box<Play>,
    mut sel: usize,
    mut armed: bool,
    input: &Input,
    fx: &mut Vec<Effect>,
) -> Mode {
    let n = match p.run.state() {
        State::Drafting(o) => o.len(),
        _ => return Mode::Play(p),
    };
    for b in &input.pressed {
        let pick = match b {
            Button::Left | Button::Up => {
                sel = (sel + n - 1) % n;
                armed = false;
                fx.push(Effect::Play(Cue::Ui));
                None
            }
            Button::Right | Button::Down => {
                sel = (sel + 1) % n;
                armed = false;
                fx.push(Effect::Play(Cue::Ui));
                None
            }
            Button::One => Some(Some(0)),
            Button::Two => Some(Some(1)),
            Button::Three => Some(Some(2)),
            Button::Fire => Some(Some(sel)),
            Button::Back => Some(None),
            _ => None,
        };
        if let Some(choice) = pick {
            let input = match choice {
                Some(i) if i < n => RunInput::Pick(i as u8),
                Some(_) => continue,
                None => RunInput::Skip,
            };
            if let Some(events) = p.run.apply(input) {
                fx.push(Effect::Play(Cue::Draft));
                if let Some(Event::Drafted(Some(pick))) = events.first() {
                    p.note(format!("{} TAKEN", pick.name()));
                }
                p.last_path = [Vec::new(), Vec::new()];
                // The next stage begins after a draft, in the present.
                fx.push(Effect::Ambience(Some(p.era())));
                return Mode::Play(p);
            }
        }
    }
    Mode::Draft {
        play: p,
        sel,
        armed,
    }
}

/// The SHIFT button's label for the bottom band; empty when there is none.
pub fn shift_label(p: &Play) -> String {
    let run = &p.run;
    let to = if p.era() == Plane::Present {
        "1347"
    } else {
        "PRESENT"
    };
    if p.flight.is_some() || p.view.is_some() {
        "SHIFT ...".to_string()
    } else if *run.state() != State::Firing {
        String::new()
    } else if !run.can_shift() {
        "SHIFT - NO SHELLS".to_string()
    } else if run.shift_cost() > 0 {
        format!("[SHIFT] {to} -1")
    } else {
        format!("[SHIFT] {to}")
    }
}

/// Stage number shown in the HUD, 1-based.
pub fn stage_label(p: &Play) -> String {
    format!("{}/{}", (p.run.stage_index() + 1).min(STAGES), STAGES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(b: Button) -> Input {
        Input {
            held: Held::default(),
            pressed: vec![b],
            pointer: None,
        }
    }

    fn tick(g: &mut Game, secs: f32) {
        let mut t = 0.0;
        while t < secs {
            g.update(1.0 / 60.0, &Input::default());
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn title_starts_a_run_and_a_shot_lands() {
        let mut g = Game::new();
        g.fixed_seed = Some(7);
        let fx = g.update(0.016, &press(Button::Fire));
        assert!(fx.contains(&Effect::Music(false)));
        assert!(fx.contains(&Effect::Ambience(Some(Plane::Present))));
        assert!(matches!(g.mode, Mode::Play(_)));
        g.update(0.016, &press(Button::Fire));
        assert!(g.play().unwrap().flight.is_some());
        tick(&mut g, 6.0);
        let p = g.play().unwrap();
        assert!(p.flight.is_none());
        assert_eq!(p.run.total_shots(), 1);
    }

    #[test]
    fn shift_crossfades_and_is_locked_in_flight() {
        let mut g = Game::new();
        g.start(9);
        g.update(0.016, &press(Button::Shift));
        let p = g.play().unwrap();
        assert_eq!(p.era(), Plane::Siege);
        assert!(p.fade.is_some());
        tick(&mut g, 0.5);
        g.update(0.016, &press(Button::Fire));
        g.update(0.016, &press(Button::Shift));
        assert_eq!(
            g.play().unwrap().run.stage().active(),
            Plane::Siege,
            "no SHIFT mid-flight"
        );
    }

    #[test]
    fn aiming_moves_and_clamps() {
        let mut g = Game::new();
        g.start(3);
        let up = Input {
            held: Held {
                up: true,
                ..Held::default()
            },
            pressed: vec![],
            pointer: None,
        };
        for _ in 0..2000 {
            g.update(0.016, &up);
        }
        assert_eq!(g.play().unwrap().angle_ticks, MAX_ANGLE_TICKS);
    }

    #[test]
    fn idle_frames_are_not_redrawn() {
        let mut g = Game::new();
        g.start(3);
        g.drawn();
        g.update(0.016, &Input::default());
        assert!(!g.needs_redraw());
        g.update(0.016, &press(Button::Up));
        assert!(g.needs_redraw());
    }

    fn ptr(x: f32, y: f32, down: bool) -> Input {
        Input {
            pointer: Some(Pointer { x, y, down }),
            ..Input::default()
        }
    }

    #[test]
    fn drag_aims_and_release_fires() {
        let mut g = Game::new();
        g.start(5);
        g.update(0.016, &ptr(205.0, 62.0, true));
        g.update(0.016, &ptr(200.0, 60.0, true));
        let p = g.play().unwrap();
        assert!(p.dragging(), "out of the dead zone, aiming");
        let (a0, p0) = (p.angle_ticks, p.power_ticks);
        g.update(0.016, &ptr(120.0, 40.0, true));
        let p = g.play().unwrap();
        assert!(p.angle_ticks > a0, "higher and nearer is steeper");
        assert_ne!(p.power_ticks, p0);
        g.update(0.016, &ptr(120.0, 40.0, false));
        let p = g.play().unwrap();
        assert!(p.drag.is_none());
        assert!(p.flight.is_some(), "release over the field fires");
    }

    #[test]
    fn release_over_a_band_cancels() {
        let mut g = Game::new();
        g.start(5);
        let before = {
            let p = g.play().unwrap();
            (p.angle_ticks, p.power_ticks)
        };
        g.update(0.016, &ptr(200.0, 60.0, true));
        g.update(0.016, &ptr(200.0, 176.0, true));
        g.update(0.016, &ptr(200.0, 176.0, false));
        let p = g.play().unwrap();
        assert!(p.flight.is_none());
        assert_eq!((p.angle_ticks, p.power_ticks), before, "aim restored");
        assert_eq!(p.run.total_shots(), 0);
    }

    #[test]
    fn tapping_shift_and_pause() {
        let mut g = Game::new();
        g.start(5);
        let r = hit::shift(&shift_label(g.play().unwrap())).unwrap();
        let (sx, sy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + 5.0);
        g.update(0.016, &ptr(sx, sy, true));
        g.update(0.016, &ptr(sx, sy, false));
        assert_eq!(g.play().unwrap().era(), Plane::Siege);
        assert!(
            g.play().unwrap().drag.is_none(),
            "a button tap is not a drag"
        );

        let (px, py) = (hit::PAUSE.x as f32 + 5.0, 4.0);
        g.update(0.016, &ptr(px, py, true));
        assert_eq!(g.play().unwrap().paused, Some(0));
        // Paused: time stands still.
        let t = g.play().unwrap().elapsed;
        g.update(0.016, &ptr(px, py, false));
        tick(&mut g, 1.0);
        assert_eq!(g.play().unwrap().elapsed, t);
        // Abandon.
        let r = hit::pause_item(1);
        g.update(0.016, &ptr(r.x as f32 + 4.0, r.y as f32 + 4.0, true));
        assert!(matches!(g.mode, Mode::Title { .. }));
    }

    #[test]
    fn esc_pauses_and_resumes() {
        let mut g = Game::new();
        g.start(5);
        g.update(0.016, &press(Button::Back));
        assert!(g.play().unwrap().paused.is_some());
        g.update(0.016, &press(Button::Back));
        assert!(g.play().unwrap().paused.is_none());
        assert!(matches!(g.mode, Mode::Play(_)));
    }

    #[test]
    fn draft_cards_take_two_taps() {
        let mut g = Game::new();
        g.start(0x5A17_11E5);
        // Play the bot to the first draft.
        for _ in 0..200 {
            if matches!(g.mode, Mode::Draft { .. }) {
                break;
            }
            let Some(p) = g.play() else { break };
            if let Some(next) = shift_sim::bot::next_input(&p.run) {
                if let Mode::Play(p) = &mut g.mode {
                    match next {
                        RunInput::Shift => p.shift(&mut Vec::new()),
                        RunInput::Fire {
                            angle_ticks,
                            power_ticks,
                        } => {
                            p.angle_ticks = angle_ticks;
                            p.power_ticks = power_ticks;
                            p.fire(&mut Vec::new());
                        }
                        _ => {}
                    }
                }
            }
            tick(&mut g, 8.0);
        }
        let Mode::Draft { play, .. } = &g.mode else {
            panic!("never reached a draft");
        };
        let n = match play.run.state() {
            State::Drafting(o) => o.len(),
            _ => 0,
        };
        // Card 0 is selected by default, but a first tap still only selects.
        let r0 = hit::card(0, n);
        let (x0, y0) = (r0.x as f32 + 10.0, r0.y as f32 + 10.0);
        g.update(0.016, &ptr(x0, y0, true));
        g.update(0.016, &ptr(x0, y0, false));
        assert!(
            matches!(g.mode, Mode::Draft { sel: 0, .. }),
            "first tap on the default card only selects it"
        );
        let r = hit::card(n - 1, n);
        let (x, y) = (r.x as f32 + 10.0, r.y as f32 + 10.0);
        g.update(0.016, &ptr(x, y, true));
        g.update(0.016, &ptr(x, y, false));
        assert!(
            matches!(g.mode, Mode::Draft { sel, .. } if sel == n - 1),
            "first tap selects"
        );
        g.update(0.016, &ptr(x, y, true));
        assert!(matches!(g.mode, Mode::Play(_)), "second tap takes it");
    }

    #[test]
    fn a_finished_run_sets_the_best_once() {
        let mut g = Game::new();
        g.autoplay = true;
        g.fixed_seed = Some(0x5A17_11E5);
        let mut saved = Vec::new();
        for _ in 0..60 * 60 * 20 {
            for e in g.update(1.0 / 30.0, &Input::default()) {
                if let Effect::SaveBest(s) = e {
                    saved.push(s);
                }
            }
            if matches!(g.mode, Mode::Over { .. }) {
                break;
            }
        }
        assert!(
            matches!(g.mode, Mode::Over { .. }),
            "the bot finishes a run"
        );
        assert_eq!(saved.len(), 1);
        assert!(g.new_best);
        assert_eq!(g.best, saved[0]);
        // Lingering on the end screen saves nothing more.
        g.autoplay = false;
        assert!(g.update(0.1, &Input::default()).is_empty());
    }

    #[test]
    fn a_tap_on_the_field_fires_nothing() {
        let mut g = Game::new();
        g.start(5);
        let before = {
            let p = g.play().unwrap();
            (p.angle_ticks, p.power_ticks)
        };
        g.update(0.016, &ptr(200.0, 60.0, true));
        g.update(0.016, &ptr(202.0, 61.0, true));
        g.update(0.016, &ptr(202.0, 61.0, false));
        let p = g.play().unwrap();
        assert!(p.flight.is_none(), "a tap is not a shot");
        assert_eq!((p.angle_ticks, p.power_ticks), before);
    }

    #[test]
    fn blank_band_left_of_the_shift_button_is_not_shift() {
        let mut g = Game::new();
        g.start(5);
        let r = hit::shift(&shift_label(g.play().unwrap())).unwrap();
        let (x, y) = (r.x as f32 - 4.0, r.y as f32 + 5.0);
        g.update(0.016, &ptr(x, y, true));
        g.update(0.016, &ptr(x, y, false));
        assert_eq!(g.play().unwrap().era(), Plane::Present);
    }

    #[test]
    fn pausing_stops_the_ambience_and_resuming_restarts_it() {
        let mut g = Game::new();
        g.start(5);
        let fx = g.update(0.016, &press(Button::Back));
        assert!(fx.contains(&Effect::Ambience(None)));
        assert!(!g.busy(), "a paused frame does not redraw itself");
        let fx = g.update(0.016, &press(Button::Back));
        assert!(fx.contains(&Effect::Ambience(Some(Plane::Present))));
    }

    #[test]
    fn a_new_stage_brings_back_the_present_ambience() {
        let mut g = Game::new();
        g.autoplay = true;
        g.fixed_seed = Some(0x5A17_11E5);
        let mut in_siege = false;
        let mut back_to_present_on_new_stage = false;
        let mut stage = 0;
        for _ in 0..60 * 60 * 10 {
            let fx = g.update(1.0 / 30.0, &Input::default());
            for e in &fx {
                match e {
                    Effect::Ambience(Some(Plane::Siege)) => in_siege = true,
                    Effect::Ambience(Some(Plane::Present)) => in_siege = false,
                    _ => {}
                }
            }
            if let Some(p) = g.play() {
                if p.run.stage_index() != stage
                    && p.view.is_none()
                    && matches!(g.mode, Mode::Play(_))
                {
                    stage = p.run.stage_index();
                    assert!(!in_siege, "stage {stage} opened with 1347's ambience");
                    back_to_present_on_new_stage = true;
                }
            }
            if matches!(g.mode, Mode::Over { .. }) {
                break;
            }
        }
        assert!(back_to_present_on_new_stage);
    }
}
