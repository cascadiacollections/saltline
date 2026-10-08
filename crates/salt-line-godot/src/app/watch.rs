//! Watching a finished run or match again, and the REPLAYS list it is
//! chosen from (DESIGN §9.4). The file format and the check that a line
//! still replays to its own result are `crate::history`'s.
//!
//! # Where it starts and where it goes back to
//!
//! ```text
//!  title ──REPLAYS (W, d-pad up, the corner plate)──► list ──a row──┐
//!  a run's result (OUT OF SHELLS) ──WATCH──────────────────────────┤
//!  a won match's plate ──WATCH─────────────────────────────────────┤
//!                                                                   ▼
//!        watch: the shots again, 1× 2× 4×, pause, next shot, next round
//!                                                                   │
//!            EXIT (Back, Esc, B, the back gesture) ──► where it started
//! ```
//!
//! # Recorded where the kept file is cleared
//!
//! A run is added to the history in `finish_run`, where its run file is
//! cleared for the score; a match in `drop_live_match`, where its match
//! file is cleared, and only a match that was won (REMATCH or TITLE on its
//! plate). A match quit before it was won is not recorded: it has no
//! result to show, and quitting is how a family says "not that one". Nor
//! is a `--salt-screen` jump, whose log cannot rebuild it, the demo's, or a
//! finished run a launch found and scored (the app died on its last shot,
//! which is rare, and its line would have to be judged twice).
//!
//! # Playing it back through the game
//!
//! A watch drives the ordinary run and versus code with the logged shots,
//! the way the attract demo drives it with the solver's (`app/demo.rs`):
//! the aim swings to each logged shot ([`WATCH_AIM`]), `fire` or
//! `versus_fire` takes it, and the shell flies, lands and topples exactly
//! as it did, plates and fanfare included. Drafts are taken by the logged
//! pick and a match's rounds dealt where the log deals them. Every shot the
//! game takes is compared with its log entry as it is fired, and the
//! result with the one recorded at the end; a difference stops the watch
//! with a note ([`Game::watch_diverged`]). Before any of that the record is
//! rebuilt off screen (`Record::check`), so a line that would diverge is
//! never shown at all.
//!
//! The speed multiplies the frame's time for everything the watch moves:
//! the flights, the holds, the aim's swing. NEXT SHOT and NEXT ROUND run
//! the same steps fast and silent within one frame ([`SKIP_STEP`]) until
//! the next shot is fired, or the next round's first plate (a run's next
//! screen) is up, so a skip lands on a frame the game really showed.
//!
//! # Nothing the player keeps changes
//!
//! The demo's two rules (DESIGN §12.3), for the same reasons:
//!
//! 1. **Set aside whole.** The player's `Play` (a finished run's result
//!    screen, or the run kept for CONTINUE), the versus state (a won
//!    match's plate) and the buffer are moved into the watch's stash on
//!    entry and moved back on exit, with the mode it started from.
//! 2. **No write is asked for.** Every place that would ask for a disk
//!    effect checks [`Game::spectating`] (the demo or a watch), so no save,
//!    score, lesson, settings, run or match file, history or crash report
//!    is written while a watch runs; `Game::tick`'s backstop drops one that
//!    slips through, and debug builds and the tests fail if it ever does.
//!
//! # Sound, screen and frame rate
//!
//! **Not muted**, unlike the demo. The demo plays to an empty room and
//! starts by itself; a watch is chosen by someone sitting with the phone,
//! often a child, and the ding of a hit and the fanfare are half of seeing
//! it again. It plays at the player's own volume, mute included, and a
//! skip is silent, and mute works during it (kept in memory, saved once the
//! watch is over). The screen is held on while a watch plays, as for a
//! demo chosen from the menu (`DEMO_AWAKE_LIMIT` without input), and let go
//! while it is paused, as a paused run is. It runs at the full frame rate
//! while it plays (at 4× a 30 fps flight would stutter) and at
//! `frame::IDLE_FPS` paused or at its end (`Game::idle`); being shown, not
//! played, it never asks for desktop GameMode (`Game::active_play`).

use super::versus::Stage;
use super::*;
use crate::history::{Record, Watchable};
use crate::input::Wb;
use crate::match_log::Entry as MatchEntry;
use salt_line_sim::replay::DRAFT_MARK;

/// The speeds, in turn: SPEED steps through them and wraps.
pub const SPEEDS: [u32; 3] = [1, 2, 4];
/// Seconds from an aim appearing to its shot, at 1×: the barrel swings to
/// it over [`WATCH_SWING`], then holds, so the aim is read. The demo's.
pub const WATCH_AIM: f64 = demo::DEMO_AIM;
/// Seconds the barrel and readouts take to swing to the next aim.
pub const WATCH_SWING: f64 = demo::DEMO_SWING;
/// Seconds the draft cards stay up before the logged pick is taken.
pub const WATCH_DRAFT: f64 = demo::DEMO_DRAFT;
/// Seconds a pass-the-phone plate stays up: long enough to see whose turn.
pub const WATCH_HANDOFF: f64 = 1.0;
/// The same with a cue on the plate (LAST SHOT, TIED, GUNS ROLLED), as
/// long as the AI's cued plate, so it can be read out.
pub const WATCH_HANDOFF_CUED: f64 = super::versus::AI_HANDOFF_CUED;
/// Seconds a round's result plate stays up before the next round.
pub const WATCH_RESULT: f64 = 2.0;
/// Seconds of play one step of a skip moves on. Small enough that a skip
/// stops on the frame the next shot is fired, not a whole flight later.
pub const SKIP_STEP: f64 = 0.1;
/// Steps a skip takes at most: far more than a round's 12 turns need.
pub const SKIP_LIMIT: usize = 6000;
/// Rows a page of the REPLAYS list shows: plated rows on a touchscreen,
/// text lines with keys or a controller.
pub const LIST_ROWS_TOUCH: usize = 6;
pub const LIST_ROWS_KEYS: usize = 10;

/// What a watch frame shows that input or a step can change, for
/// `FrameState`: paused, the speed, ended, the next entry.
pub(crate) type Fingerprint = (bool, usize, bool, usize);

/// The finished runs and matches, as the history file has them, and where
/// the REPLAYS list stands.
#[derive(Default)]
pub(crate) struct HistoryState {
    /// The file as last read or written, newest last.
    pub(crate) file: crate::history::History,
    /// The list's chosen row, counted newest first.
    pub(crate) focus: usize,
    /// The run on the result screen, if it was recorded: its WATCH.
    pub(crate) last_run: Option<Record>,
}

/// The player's state, moved out whole while a watch runs.
struct Stash {
    play: Play,
    versus_live: Option<VersusPlay>,
    help: [bool; 2],
    rolling_off: bool,
    ground: Option<GroundPick>,
    buffer_w: i32,
    kept_width: bool,
    mode: Mode,
}

/// What the driver last waited on: a change starts its hold afresh.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cue {
    Draft,
    Handoff,
    Result,
}

/// The next logged aim, swinging into place before it fires.
#[derive(Clone, Copy, Debug)]
struct Pending {
    from: (i32, i32),
    to: (i32, i32),
    /// Seconds into the swing.
    t: f64,
}

/// Where a skip stops.
#[derive(Clone, Copy, Debug)]
enum Skip {
    /// Once this many shots have been fired.
    Shot(usize),
    /// Once this round (a run's screen) is on screen, nothing moving.
    Round(u32),
}

/// A watch in progress. See the module docs.
pub(crate) struct Watch {
    /// What is being played back, already checked.
    pub(crate) show: Watchable,
    /// Index into [`SPEEDS`].
    pub(crate) speed: usize,
    pub(crate) paused: bool,
    /// The last entry has been played and the result matched.
    pub(crate) ended: bool,
    /// The next log entry to play.
    pub(crate) cursor: usize,
    /// Shots fired so far.
    pub(crate) fired: usize,
    wait: f64,
    cue: Option<Cue>,
    pending: Option<Pending>,
    stash: Stash,
}

impl Watch {
    /// The speed as a multiple of real time.
    pub(crate) fn rate(&self) -> u32 {
        SPEEDS[self.speed % SPEEDS.len()]
    }

    /// Whether this is a match rather than a run.
    pub(crate) fn duel(&self) -> bool {
        matches!(self.show, Watchable::Duel(..))
    }

    /// Wait on `cue`: on a new cue the wait restarts at `hold`. True once
    /// it has run out.
    fn ready(&mut self, cue: Cue, hold: f64) -> bool {
        if self.cue != Some(cue) {
            self.cue = Some(cue);
            self.wait = hold;
        }
        self.wait <= 0.0
    }
}

impl Game {
    /// A replay is being watched.
    pub(crate) fn watching(&self) -> bool {
        self.watch.is_some()
    }

    /// The game on screen is being shown, not played: the attract demo or
    /// a watch. Nothing the player keeps may change, so every disk write
    /// checks this (see the module docs, and DESIGN §12.3).
    pub(crate) fn spectating(&self) -> bool {
        self.in_demo() || self.watching()
    }

    // ------------------------------------------------------------ history

    /// At launch: the history file's text, if there is one (`game.rs` picks
    /// it from the file and its temporary copy, as the slot file's).
    pub fn set_history(&mut self, text: Option<&str>) {
        self.history.file = text.map(crate::history::History::parse).unwrap_or_default();
    }

    /// Add a finished run or match to the history, and ask for the file to
    /// be written, unless it is a newer build's.
    fn record_finished(&mut self, record: Record) {
        if self.spectating() {
            return;
        }
        if let Some(text) = self.history.file.push(record) {
            self.fx.push(Effect::History(text));
        }
    }

    /// `finish_run`, for a run kept on disk: the run on screen has ended.
    /// It becomes the result screen's WATCH too.
    pub(super) fn record_run(&mut self) {
        let Some(run) = self.play.run.as_ref() else {
            return;
        };
        let record = Record::run(self.today, run, self.play.daily);
        self.history.last_run = Some(record.clone());
        self.record_finished(record);
    }

    /// The record of the won match `vs`, kept with aim help `help`. `None`
    /// while it is still to be won.
    pub(super) fn match_record(&self, vs: &VersusPlay, help: [bool; 2]) -> Option<Record> {
        let saved = super::versus::saved_line(vs, help)?;
        let ground = match &vs.layout {
            None => crate::history::Ground::Generated,
            Some(l) => crate::history::Ground::Map(l.name.as_str().to_string()),
        };
        Record::duel(self.today, &saved, &vs.duel, ground)
    }

    /// `drop_live_match`, for a kept match being cleared: recorded if won.
    pub(super) fn record_match(&mut self, vs: &VersusPlay) {
        if let Some(record) = self.match_record(vs, self.versus.help) {
            self.record_finished(record);
        }
    }

    // --------------------------------------------------------------- list

    /// The REPLAYS list, on its newest row.
    pub(crate) fn open_replays(&mut self) {
        self.history.focus = 0;
        self.mode = Mode::Replays;
    }

    /// Rows a page of the list holds on the device in use.
    pub(crate) fn replay_rows(&self) -> usize {
        if self.input.touch_active {
            LIST_ROWS_TOUCH
        } else {
            LIST_ROWS_KEYS
        }
    }

    /// The list: a row (or Confirm on the chosen one) watches it, Up and
    /// Down choose, Left and Right (and the page buttons) turn a page, Back
    /// goes to the title.
    pub(super) fn handle_replays_input(&mut self, delta: f64) {
        let n = self.history.file.records.len();
        let rows = self.replay_rows();
        let page = self.history.focus / rows;
        if self.action_pressed(Action::Back) {
            self.play(Sound::Ui);
            self.mode = Mode::Title;
            return;
        }
        let tapped = (0..rows as u8).find(|&i| self.input.touch.pressed(Action::Wb(Wb::Row(i))));
        if let Some(i) = tapped {
            let index = page * rows + i as usize;
            if index < n {
                self.history.focus = index;
                self.watch_listed(index);
            }
            return;
        }
        if self.action_pressed(Action::Confirm) || self.pressed(Key::KpEnter) {
            if n > 0 {
                self.watch_listed(self.history.focus);
            }
            return;
        }
        if n == 0 {
            return;
        }
        let down = self.action_repeated(2, Action::Down, delta) as i64
            - self.action_repeated(3, Action::Up, delta) as i64;
        let pages = self.action_repeated(1, Action::Right, delta) as i64
            - self.action_repeated(0, Action::Left, delta) as i64
            + self.input.touch.pressed(Action::Wb(Wb::PageNext)) as i64
            - self.input.touch.pressed(Action::Wb(Wb::PagePrev)) as i64;
        if down != 0 || pages != 0 {
            let to = self.history.focus as i64 + down + pages * rows as i64;
            let to = to.clamp(0, n as i64 - 1) as usize;
            // A click only for a move: not at either end of the list.
            if to != self.history.focus {
                self.history.focus = to;
                self.play(Sound::Ui);
            }
        }
    }

    /// WATCH the list's row `index`, counted newest first, or say why not.
    fn watch_listed(&mut self, index: usize) {
        let Some(record) = self.history.file.newest_first().nth(index).cloned() else {
            return;
        };
        self.watch_record(&record);
    }

    /// WATCH on a run's result screen: the run just finished.
    pub(super) fn watch_last_run(&mut self) {
        if let Some(record) = self.history.last_run.clone() {
            self.watch_record(&record);
        }
    }

    /// WATCH on a won match's plate: the match on screen.
    pub(super) fn watch_live_match(&mut self) {
        let record = self
            .versus
            .live
            .as_ref()
            .and_then(|vs| self.match_record(vs, self.versus.help));
        if let Some(record) = record {
            self.watch_record(&record);
        }
    }

    /// Check `record` and watch it, or say why it cannot be watched. The
    /// watch goes back to the screen it was started from: the list, a
    /// run's result, a won match's plate (the mode is in its stash).
    fn watch_record(&mut self, record: &Record) {
        match record.check() {
            Ok(show) => self.start_watch(show),
            Err(why) => self.note(why.note().into()),
        }
    }

    // -------------------------------------------------------------- watch

    /// Start watching `show`: settings flushed while writes are still
    /// allowed, the player's state set aside, and the run or match put on
    /// screen from its start.
    fn start_watch(&mut self, show: Watchable) {
        if self.spectating() {
            return;
        }
        self.play(Sound::Ui);
        self.flush_settings();
        self.status_time = 0.0;
        let stash = Stash {
            play: std::mem::take(&mut self.play),
            versus_live: self.versus.live.take(),
            help: self.versus.help,
            rolling_off: self.versus.rolling_off,
            ground: self.versus.ground.take(),
            buffer_w: self.display.buffer_w,
            kept_width: self.display.kept_width,
            mode: self.mode,
        };
        self.watch = Some(Watch {
            show,
            speed: 0,
            paused: false,
            ended: false,
            cursor: 0,
            fired: 0,
            wait: 0.0,
            cue: None,
            pending: None,
            stash,
        });
        self.begin_watch_show();
        self.settle_touch();
    }

    /// Put the watched run or match on screen as it began, and the watch at
    /// its first entry: at the start, and for AGAIN at the end.
    fn begin_watch_show(&mut self) {
        let Some(w) = self.watch.as_mut() else {
            return;
        };
        w.cursor = 0;
        w.fired = 0;
        w.ended = false;
        w.paused = false;
        w.wait = 0.0;
        w.cue = None;
        w.pending = None;
        let show = w.show.clone();
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.play = Play::default();
        self.versus.live = None;
        match show {
            Watchable::Run(replay, _, daily) => {
                // A run keeps the width it was played at, letterboxed on
                // another device (DESIGN §9.2).
                self.set_run_width(Some(replay.width));
                let run = RunState::new(replay.run_seed, replay.width);
                self.play.last_seed = run.run_seed;
                self.play.daily = daily;
                self.play.run = Some(run);
                self.mode = Mode::Playing;
                self.new_preview();
            }
            Watchable::Duel(saved, ..) => {
                let Some(start) = saved.start() else {
                    // Checked before the watch began; never expected.
                    self.watch_diverged();
                    return;
                };
                let mut vs = VersusPlay::new(start);
                vs.layout = saved.layout().ok().flatten();
                vs.ai = saved.ai;
                // Aim help as the match had it: the landing marks and arcs
                // the players saw.
                self.versus.help = saved.help;
                self.versus.live = Some(vs);
                self.set_run_width(Some(saved.width));
                self.mode = Mode::Versus;
            }
        }
        self.apply_buffer_size();
        self.fx.push(Effect::Redraw);
    }

    /// End the watch and put everything back as it was, on the screen it
    /// started from, with touches settled so the tap that ended it presses
    /// nothing there.
    pub(crate) fn stop_watch(&mut self) {
        let Some(w) = self.watch.take() else {
            return;
        };
        let s = w.stash;
        self.play = s.play;
        self.versus.live = s.versus_live;
        self.versus.help = s.help;
        self.versus.rolling_off = s.rolling_off;
        self.versus.ground = s.ground;
        self.display.buffer_w = s.buffer_w;
        self.display.kept_width = s.kept_width;
        self.mode = s.mode;
        self.input.touch.cancel_drag();
        self.input.drag_from = None;
        self.settle_touch();
        self.apply_buffer_size();
        self.fx.push(Effect::Redraw);
    }

    /// What the game did is not what the log says: stop, and say so. Never
    /// expected once `Record::check` has passed; kept so a determinism bug
    /// shows as a note rather than as a different game.
    fn watch_diverged(&mut self) {
        self.stop_watch();
        self.note(crate::history::Refused::Mismatch.note().into());
    }

    /// The watch's controls (the bar on a touchscreen; keys and a
    /// controller as the bar's hint names them): Back leaves; Confirm or
    /// Fire pauses and plays (AGAIN once it has ended); Up steps the speed;
    /// Right skips to the next shot and Down to the next round. Mute and
    /// F1 are asked first, as in every mode (`handle_global_input`).
    pub(super) fn handle_watch_input(&mut self) {
        self.handle_global_input();
        let tapped = |g: &Game, b: Wb| g.input.touch.pressed(Action::Wb(b));
        if tapped(self, Wb::Exit) || self.action_pressed(Action::Back) {
            self.play(Sound::Ui);
            self.stop_watch();
            return;
        }
        let ended = self.watch.as_ref().is_some_and(|w| w.ended);
        if tapped(self, Wb::Pause)
            || self.action_pressed(Action::Confirm)
            || self.action_pressed(Action::Fire)
            || self.pressed(Key::KpEnter)
        {
            self.play(Sound::Ui);
            if ended {
                self.begin_watch_show();
            } else if let Some(w) = self.watch.as_mut() {
                w.paused = !w.paused;
            }
        } else if tapped(self, Wb::Speed) || self.action_pressed(Action::Up) {
            self.play(Sound::Ui);
            if let Some(w) = self.watch.as_mut() {
                w.speed = (w.speed + 1) % SPEEDS.len();
            }
        } else if tapped(self, Wb::NextShot) || self.action_pressed(Action::Right) {
            if !ended {
                self.watch_skip(false);
            }
        } else if (tapped(self, Wb::NextRound) || self.action_pressed(Action::Down)) && !ended {
            self.watch_skip(true);
        }
    }

    /// One frame of a watch: unless paused, `delta` at the chosen speed.
    pub(super) fn watch_frame(&mut self, delta: f64) {
        let Some(w) = self.watch.as_ref() else {
            return;
        };
        if w.paused {
            return;
        }
        let dt = delta * w.rate() as f64;
        self.watch_advance(dt);
    }

    /// Move the watch on by `dt` seconds of play: the driver, then the run's
    /// or the match's own step, as `tick` steps a game in play.
    fn watch_advance(&mut self, dt: f64) {
        if let Some(w) = self.watch.as_mut() {
            w.wait -= dt;
            if let Some(p) = w.pending.as_mut() {
                p.t += dt;
            }
        }
        self.watch_drive();
        // A watch that stopped as it was driven (a diverging shot) has put
        // the player's game back: that is not to be stepped.
        if self.watch.is_none() {
            return;
        }
        self.step(dt);
        self.step_versus(dt);
    }

    /// NEXT SHOT or NEXT ROUND: the watch's own steps, fast and silent,
    /// until the next shot is fired, or the next round (a run's next
    /// screen) is up and still. A paused watch stays paused there, so
    /// NEXT SHOT steps through a game shot by shot.
    fn watch_skip(&mut self, round: bool) {
        let Some(w) = self.watch.as_ref() else {
            return;
        };
        let target = if round {
            Skip::Round(self.watch_round() + 1)
        } else {
            Skip::Shot(w.fired + 1)
        };
        let from = self.fx.len();
        for _ in 0..SKIP_LIMIT {
            let Some(w) = self.watch.as_ref() else {
                break;
            };
            if w.ended || self.skip_reached(target) {
                break;
            }
            self.watch_advance(SKIP_STEP);
        }
        // Silent: the sounds of what was skipped are dropped.
        let kept: Vec<Effect> = self
            .fx
            .drain(from..)
            .filter(|e| !matches!(e, Effect::Play(_)))
            .collect();
        self.fx.extend(kept);
        self.fx.push(Effect::Redraw);
    }

    /// The round being watched: a match's, or a run's screen.
    fn watch_round(&self) -> u32 {
        match self.mode {
            Mode::Versus => self.versus.live.as_ref().map_or(0, |vs| vs.duel.round),
            _ => self.play.run.as_ref().map_or(0, |r| r.screen_index),
        }
    }

    fn skip_reached(&self, target: Skip) -> bool {
        match target {
            Skip::Shot(n) => self.watch.as_ref().is_some_and(|w| w.fired >= n),
            Skip::Round(r) => {
                let still = match self.mode {
                    Mode::Versus => self
                        .versus
                        .live
                        .as_ref()
                        .is_some_and(|vs| vs.stage == Stage::Handoff),
                    _ => self.play.anim.is_none() && self.play.clear_hold <= 0.0,
                };
                still && self.watch_round() >= r
            }
        }
    }

    /// Swing the aim toward the pending shot, eased in and out, as the
    /// demo's does. The aim, once it is exactly on the shot and
    /// [`WATCH_AIM`] is up.
    fn swing_watch_aim(&mut self) -> Option<(i32, i32)> {
        let w = self.watch.as_mut()?;
        let p = w.pending?;
        if p.t < WATCH_AIM {
            let t = (p.t / WATCH_SWING).clamp(0.0, 1.0);
            let e = t * t * (3.0 - 2.0 * t);
            let lerp = |a: i32, b: i32| a + ((b - a) as f64 * e).round() as i32;
            (self.play.angle_ticks, self.play.power_ticks) =
                (lerp(p.from.0, p.to.0), lerp(p.from.1, p.to.1));
            return None;
        }
        w.pending = None;
        (self.play.angle_ticks, self.play.power_ticks) = p.to;
        Some(p.to)
    }

    /// Start the aim swinging to `to`.
    fn aim_watch(&mut self, to: (i32, i32)) {
        let from = (self.play.angle_ticks, self.play.power_ticks);
        if let Some(w) = self.watch.as_mut() {
            w.pending = Some(Pending { from, to, t: 0.0 });
        }
    }

    /// One step of the watch's play, in place of the player's input.
    fn watch_drive(&mut self) {
        let Some(w) = self.watch.as_ref() else {
            return;
        };
        if w.ended {
            return;
        }
        if w.duel() {
            self.watch_duel_drive();
        } else {
            self.watch_run_drive();
        }
    }

    fn watch_run_drive(&mut self) {
        let Some(run) = self.play.run.as_ref() else {
            return;
        };
        if self.play.anim.is_some() || self.play.clear_hold > 0.0 {
            return;
        }
        let phase = run.phase;
        let Some(w) = self.watch.as_mut() else {
            return;
        };
        let Watchable::Run(replay, ..) = &w.show else {
            return;
        };
        let next = replay.log.get(w.cursor).copied();
        match phase {
            Phase::Over(_) => self.watch_end(),
            Phase::Cleared => self.play.clear_hold = CLEAR_HOLD,
            Phase::Drafting => {
                let Some(e) = next.filter(|e| e.angle_ticks == DRAFT_MARK) else {
                    return self.watch_diverged();
                };
                if !w.ready(Cue::Draft, WATCH_DRAFT) {
                    return;
                }
                w.cue = None;
                w.cursor += 1;
                let taken = self
                    .play
                    .run
                    .as_mut()
                    .and_then(|r| r.take_offer(e.power_ticks as usize));
                if taken.is_none() {
                    return self.watch_diverged();
                }
                self.new_preview();
                self.play(Sound::Draft);
                self.apply_buffer_size();
            }
            Phase::Firing => {
                let Some(e) = next.filter(|e| e.angle_ticks != DRAFT_MARK) else {
                    return self.watch_diverged();
                };
                if w.pending.is_none() {
                    // The emplacement and the fuse as the shot was fired
                    // with them, the way `Replay::reconstruct` sets them.
                    let Some(run) = self.play.run.as_mut() else {
                        return;
                    };
                    let moved = e.emplacement_offset == run.offset_units()
                        || (run.reposition(e.emplacement_offset as f64)
                            && run.offset_units() == e.emplacement_offset);
                    run.set_fuse_tenths(e.fuse_tenths);
                    if !moved || run.fuse_tenths() != e.fuse_tenths {
                        return self.watch_diverged();
                    }
                    self.aim_watch((e.angle_ticks, e.power_ticks));
                } else if self.swing_watch_aim().is_some() {
                    self.fire();
                    let same = self.play.run.as_ref().and_then(|r| r.log.last()) == Some(&e);
                    if !same {
                        return self.watch_diverged();
                    }
                    if let Some(w) = self.watch.as_mut() {
                        w.cursor += 1;
                        w.fired += 1;
                    }
                }
            }
        }
    }

    fn watch_duel_drive(&mut self) {
        let Some(vs) = self.versus.live.as_ref() else {
            return;
        };
        let (stage, phase, easing, cued) =
            (vs.stage, vs.duel.phase, vs.easing(), vs.handoff_cued());
        let Some(w) = self.watch.as_mut() else {
            return;
        };
        let Watchable::Duel(saved, ..) = &w.show else {
            return;
        };
        let next = saved.log.get(w.cursor).copied();
        match stage {
            Stage::Handoff => {
                let hold = if cued {
                    WATCH_HANDOFF_CUED
                } else {
                    WATCH_HANDOFF
                };
                if w.ready(Cue::Handoff, hold) {
                    w.cue = None;
                    self.begin_versus_turn();
                }
            }
            Stage::Aiming => {
                // The guns finish rolling before the aim moves, as the
                // AI's does.
                if easing {
                    return;
                }
                let Some(MatchEntry::Shot(a, p)) = next else {
                    return self.watch_diverged();
                };
                if w.pending.is_none() {
                    self.aim_watch((a, p));
                } else if self.swing_watch_aim().is_some() {
                    self.versus_fire();
                    let same = self.versus.live.as_ref().and_then(|vs| vs.log.last())
                        == Some(&MatchEntry::Shot(a, p));
                    if !same {
                        return self.watch_diverged();
                    }
                    if let Some(w) = self.watch.as_mut() {
                        w.cursor += 1;
                        w.fired += 1;
                    }
                }
            }
            Stage::Flight => {}
            Stage::Result => match phase {
                crate::versus::Phase::MatchOver(_) => self.watch_end(),
                crate::versus::Phase::RoundOver(_) => {
                    if next != Some(MatchEntry::NextRound) {
                        return self.watch_diverged();
                    }
                    if !w.ready(Cue::Result, WATCH_RESULT) {
                        return;
                    }
                    w.cue = None;
                    w.cursor += 1;
                    self.after_versus_result();
                }
                crate::versus::Phase::Aiming => {}
            },
        }
    }

    /// The last entry has played: the result must be the recorded one, and
    /// every entry taken. The end shows until EXIT, or AGAIN.
    fn watch_end(&mut self) {
        let Some(w) = self.watch.as_ref() else {
            return;
        };
        let matched = match &w.show {
            Watchable::Run(replay, score, _) => {
                w.cursor == replay.log.len()
                    && self
                        .play
                        .run
                        .as_ref()
                        .is_some_and(|r| r.is_over() && r.score() == *score)
            }
            Watchable::Duel(saved, winner, rounds) => {
                w.cursor == saved.log.len()
                    && self.versus.live.as_ref().is_some_and(|vs| {
                        vs.duel.phase == crate::versus::Phase::MatchOver(*winner)
                            && vs.duel.rounds == *rounds
                    })
            }
        };
        if !matched {
            return self.watch_diverged();
        }
        if let Some(w) = self.watch.as_mut() {
            w.ended = true;
        }
        self.fx.push(Effect::Redraw);
    }

    /// What a watch frame shows that input or a step can change, for
    /// `FrameState`: paused, the speed, ended, the entry reached.
    pub(super) fn watch_fingerprint(&self) -> Option<Fingerprint> {
        self.watch
            .as_ref()
            .map(|w| (w.paused, w.speed, w.ended, w.cursor))
    }
}
