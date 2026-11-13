# Game Off 2026 entry: design lock

Theme: **SHIFT**. Deadline: Tue 2026-12-01 13:37 PT. Target: submit Mon 2026-11-30.
Status: design locked 2026-11-13. Working title: **Counterweight** (final name can change before the itch page
goes up).

## Same engine, its own game

Game Off rule 1: templates, starter projects, existing libraries and reusable
code are welcome, "but the vast majority of your game's development should
happen during the submission period". The FAQ adds: "Please don't submit an old
game with only minor changes." Bolting a phase system onto released Salt Line
(v0.2.0, Steam, shareware edition) would be an old game with additions.

So the entry is a **new, smaller game built in November** on the same engine:
`salt-line-sim` used as a **library** (terrain heightfields, ballistics, damage,
the solver and the RNG). Its rules, run loop, presentation and identity are new.

**Identity.** Counterweight is its own game, not a Salt Line edition: its own
name, title art, icon and application ID (`com.cascadiacollections.counterweight`).
`TRADEMARK.md` reserves the Salt Line name and icon, and the jam entry
stays clear of them. The credits say it is built on the Salt Line engine.

- Only the new game and the library it needs go into the public repo. The full
  Salt Line game (the Godot crate, editor, versus, challenges, store builds)
  stays out of it.
- `salt-line-sim` is used as is. If the new game needs something it doesn't
  expose, add a small public accessor in its own PR. Never fork or change its
  outcomes: `SIM_EPOCH`, the golden runs and Salt Line's replays must stay
  unchanged.
- Salt Line on `main` is untouched. Nothing about the jam ships in Salt Line
  unless it's merged back on purpose after the jam (see POSTJAM.md).

## Layout in the repo

| Path | What |
| --- | --- |
| `crates/shift-sim` | New. The era rules: two-era stages, echoes, the phase modifiers, Expedition run and score. Depends only on `salt-line-sim`. Deterministic, headless, fully tested. |
| `crates/shift-godot` | New. gdext presentation: rendering, input, HUD, audio, title, victory and defeat. |
| `gameoff/godot/` | New. The Godot project for the entry (its own `project.godot`, `.gdextension` and export presets: Web plus desktop fallbacks). |
| `gameoff/` | Design, post-jam notes, itch page copy. |
| `AI_DISCLOSURE.md` | Maintained from day one. Pasted verbatim into the itch page and the public README. |

The game never reads `salt-line-godot`. The workspace gains two members, and
nothing in the existing crates depends on them.

## The game: "The line shifts"

Artillery against a fixed run of fortified screens, across two **eras** of the
same coast. **SHIFT** (key: **Shift**, gamepad: shoulder button, shown in the
HUD) swaps eras, and the battlefield cross-fades.

| | **Present** | **Siege** (1347) |
| --- | --- | --- |
| Look | Pale, day | Deep, dusk, torchlit |
| Your gun | Howitzer | **Trebuchet**: counterweight throw, heavy stone |
| Flight | Shell speed, full wind | 0.85× speed (shorter reach, higher lobs), 0.35× wind |
| Impact | Shell crater and blast | 1.5× crater, 1.3× blast |
| Targets | Derricks, masts, water towers, silos, pylons | The same footprints dressed as watchtowers, banner poles, bell towers, keeps, siege towers |

The crossover is the theme: one coastline, six hundred years apart. Wreck a
keep in 1347 and its ruin still stands in the present as a weakened bunker.
Fell a radio mast today and a ghostly watchtower rises on the medieval line.

### 1. Two eras (core)

- Each screen is a **pair** of `salt_line_sim::run::Screen`s generated from one
  run seed: `era_seed = rng::substream(run_seed, "PRES" | "SIEG")`. Terrain,
  structures and wind come from that era's seed. Damage (craters, structure
  state) lives in that era's `Screen` and never touches the other.
- The trebuchet is the drafted loadout with the Siege multipliers applied on
  top, per shot. Each era is generated and solvability-gated **with its own
  gun**, so the Siege screens are always reachable by stone.
- The pairing is deterministic from the run seed, so one seed always gives the
  same expedition.

### 2. Echoes

- When a structure is destroyed in one phase, it leaves an **echo** in the
  other: a ghost emplacement at the same x position, seated on that era's
  terrain, with **45% of the original's health**. It still blocks and still
  counts as hostile.
- A screen is cleared only when **every structure and every echo is dead in
  both eras**.
- The skill is choosing which era to leave your wreckage in. Killing a
  structure in its "home" era is full price, and the echo it leaves is cheap.
- Inactive-era structures and echoes draw translucent over the active era,
  so you can plan.

### 3. Modifiers (drafted every 4 screens)

The pool is the new game's own, kept to 6 entries: four stock gun modifiers
(Ballast, Sabot, Airburst, Vernier; they apply to the howitzer and the
trebuchet alike) plus these two era modifiers.

- **Half-Phased Plating.** Your shots land in both eras: the other era at 55%
  (smaller craters, 55% of the structural damage). Every SHIFT costs a shell.
  Dangerous to both worlds, safe in neither. (The handoff's "enemy fire passes
  through you" clause is dropped because nothing fires back in Salt Line. The
  shell-per-SHIFT cost is the price instead.)
- **Echo Decay.** Each SHIFT strips 0.10 integrity from every live echo in
  the era you shift into.

### Run shape: Expedition (the only mode for the jam)

- **12 screens** with escalating structure count and wind (reusing
  `salt-line-sim` band curves). Drafts come after screens 4 and 8.
- Screen 12 is the **bastion**: one heavy structure anchored in both phases
  (no echo rules). It must be killed once in each phase.
- Shells are the run resource (start 14, refunds on clean clears, as in the sim
  economy). **Defeat**: shells run out with the screen uncleared.
- **Score** = screens cleared × 1000 + shells remaining × 100 + time bonus
  (max(0, 900 − seconds)).
- Endless mode is out of scope for the jam. The design allows it later.

### Balance guardrails

- SHIFT costs nothing. It locks only while a shell is in flight.
- Echoes start at 40–50% health (45% to begin with).
- Start with 30 shells. Clearing a stage refunds as a screen cleared in
  ⌈shots/2⌉ shots. First bot numbers (200 seeds, perfect solver aim): 96% wins,
  7.2 shells left on average, and the losses all run dry at the bastion.
  Humans miss, so expect to loosen this in the balance pass.
- A competent player clears an Expedition in 8–12 minutes. A golden-seed test
  and a batch harness of random shots check it can be finished at all, and a
  playtest checks the time.

## Presentation

- **Art.** The Present reuses existing engine art (terrain, howitzer,
  structures) with its own grade. Siege is a grade swap (deep,
  dusk) of the same art. The few medieval pieces it needs (trebuchet, stone,
  medieval skins for the five structure footprints) are **made during the jam
  with the existing procedural pipeline** (`tools/gen_sprites.py`), at the same
  pixel scale and palette. This relaxes the handoff's "no new art" rule, at
  the owner's request (trebuchet crossover). No image-model art.
- **Audio**: reuse existing WAVs: `title_loop`, `ui` (menu move and select),
  `draft` (modifier pick), `collapse`, `terrain_impact` and `structure_impact`
  (explosions), `versus_win` (victory), `low_shell` (defeat sting). The handoff's
  names (`explosion`, `game_over`, `victory`, `ui_move`, `ui_select`,
  `pickup_modifier`) don't exist under those names. These are the mapped
  equivalents. A short SHIFT whoosh is synthesized in `tools/gen_audio.py`,
  the same way the existing SFX are.
- **HUD, always visible**: active era and gun, SHIFT (ready or locked), shells,
  screen n/12, echoes remaining (both phases).
- Title screen: one EXPEDITION entry, plus the build stamp (`gameoff-vX.Y.Z` +
  short SHA, from CI env at build time).

## Determinism and tests (`crates/shift-sim`)

- Same seed gives identical eras, terrain, structures and echo placement.
- Per-era isolation: damage in the Present never alters Siege terrain or original
  structures. The only cross-era effect is new echoes.
- Replay: the run is seed + an input log (shots and SHIFTs). It has its own
  versioned format (`SHIFT_LOG_VERSION`) and a round-trip test.
  `salt-line-sim`'s codec and epoch are untouched.
- Golden-seed Expedition: scripted inputs, taken from the solver, reach and kill
  the bastion.
- Fuzzing: `fuzz/` is untouched unless shift-sim's log decoder is exposed to
  untrusted input. If it is, add one target.

## Schedule

| Date | Milestone |
| --- | --- |
| Nov 13 | Design lock, branch cut, AI disclosure, web-export spike, `shift-sim` skeleton |
| Nov 14–18 | Two-phase stages, SHIFT, palette swap, determinism tests. Godot shell boots in browser |
| Nov 19–22 | Echoes, modifiers + draft, bastion, victory/defeat, HUD |
| Nov 23–25 | Balance, golden-seed test, `gameoff-web.yml` end-to-end, itch draft channel playable |
| Nov 26–28 | Playtest + fixes only. **Freeze Nov 28 EOD** |
| Nov 29 | Public repo published after secrets scan; itch page + disclosure |
| Nov 30 | Butler push to public channel, **submit**, confirm entry on the jam page |

## Open decisions (owner)

- Final title (Counterweight is the working title).
- Public repo name (`cascadiacollections/counterweight` proposed, replacing the
  handoff's `saltline-gameoff`) and license (MIT, matching the sim).
- Where the workspace asset folders (`salt-line/assets`, `/audio`,
  `/marketing`) live. They aren't on this Mac under `~/Development` or `~`.
