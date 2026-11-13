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

/// One frame's input. `held` is what is down now; `pressed` what went down
/// since the last frame.
#[derive(Clone, Default, Debug)]
pub struct Input {
    pub held: Held,
    pub pressed: Vec<Button>,
}

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
    /// Title music on or off.
    Music(bool),
    /// Quit the application (title screen, Esc).
    Quit,
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
    /// Esc pressed once: a second press within the window leaves the run.
    pub quit_armed: f32,
    /// After the run ends, seconds until the end screen.
    pub ending: Option<f32>,
}

#[derive(Clone)]
pub enum Mode {
    Title { t: f32 },
    Play(Box<Play>),
    Draft { play: Box<Play>, sel: usize },
    Over { play: Box<Play>, victory: bool },
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
            Mode::Play(p) => {
                p.flight.is_some()
                    || p.fade.is_some()
                    || !p.blasts.is_empty()
                    || p.note.is_some()
                    || p.hold > 0.0
                    || p.ending.is_some()
                    || p.quit_armed > 0.0
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
        let mut fx = Vec::new();
        let h = input.held;
        if !input.pressed.is_empty() || h.up || h.down || h.left || h.right {
            self.dirty = true;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Title { t: 0.0 });
        self.mode = match mode {
            Mode::Title { t } => self.title(t + dt, input, &mut fx),
            Mode::Play(p) => play_update(p, dt, input, &mut fx),
            Mode::Draft { play, sel } => draft_update(play, sel, input, &mut fx),
            Mode::Over { play, victory } => {
                if input.pressed.contains(&Button::Fire) || input.pressed.contains(&Button::Back) {
                    fx.push(Effect::Play(Cue::Ui));
                    fx.push(Effect::Music(true));
                    Mode::Title { t: 0.0 }
                } else {
                    Mode::Over { play, victory }
                }
            }
        };
        if !fx.is_empty() {
            self.dirty = true;
        }
        fx
    }

    fn title(&mut self, t: f32, input: &Input, fx: &mut Vec<Effect>) -> Mode {
        if input.pressed.contains(&Button::Fire) {
            fx.push(Effect::Play(Cue::Ui));
            fx.push(Effect::Music(false));
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
            quit_armed: 0.0,
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
                    fx.push(Effect::Play(Cue::Victory));
                    self.note("THE BASTION FALLS");
                    self.ending = Some(2.0);
                }
                Event::Defeat => {
                    fx.push(Effect::Play(Cue::Defeat));
                    self.note("OUT OF SHELLS");
                    self.ending = Some(2.0);
                }
                Event::Shifted { .. } | Event::Drafted(_) => {}
            }
        }
    }
}

fn play_update(mut p: Box<Play>, dt: f32, input: &Input, fx: &mut Vec<Effect>) -> Mode {
    p.elapsed += dt as f64;
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
    p.quit_armed = (p.quit_armed - dt).max(0.0);
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
        return Mode::Draft { play: p, sel: 0 };
    }

    let idle = p.flight.is_none() && p.view.is_none();
    if idle {
        p.aim(dt, input.held);
    }
    for b in &input.pressed {
        match b {
            Button::Fire if idle => p.fire(fx),
            Button::Shift => p.shift(fx),
            Button::Back => {
                if p.quit_armed > 0.0 {
                    fx.push(Effect::Play(Cue::Ui));
                    fx.push(Effect::Music(true));
                    return Mode::Title { t: 0.0 };
                }
                p.quit_armed = 2.0;
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

fn draft_update(mut p: Box<Play>, mut sel: usize, input: &Input, fx: &mut Vec<Effect>) -> Mode {
    let n = match p.run.state() {
        State::Drafting(o) => o.len(),
        _ => return Mode::Play(p),
    };
    for b in &input.pressed {
        let pick = match b {
            Button::Left | Button::Up => {
                sel = (sel + n - 1) % n;
                fx.push(Effect::Play(Cue::Ui));
                None
            }
            Button::Right | Button::Down => {
                sel = (sel + 1) % n;
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
                return Mode::Play(p);
            }
        }
    }
    Mode::Draft { play: p, sel }
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
}
