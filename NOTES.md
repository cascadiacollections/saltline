# Build notes

Where this build had to decide something the spec did not, or where a spec
number produced a bad result. The brief asks for these to be reported rather
than quietly retuned, so this is that report.

DESIGN.md is still authoritative. Nothing here changes a mechanic; the numbers
below are either fills for gaps in the spec or fixes for cases where following
the spec literally broke one of its own stated goals.

---

## 1. Changed a spec number, with a reason

### `STEEL_BLAST_FACTOR = 1.15`

DESIGN §2.7 says a direct blast reduces integrity by "blast overlap × material
factor" and puts collapse at `integrity ≤ 0`, but never gives the factor. At
anything below 1.0 a clean direct hit leaves a structure `Leaning`, which means
no screen is ever clearable in one shot — and DESIGN §1 pays +3 for a one-shot
clear while §2.4 requires every screen to have one. 1.15 makes a direct hit
fell a structure and leaves a ~30%-overlap near miss at `Leaning`.

**If this is wrong**, the alternative reading is that one-shot clears are meant
to require a modifier, and the refund table's top tier is aspirational. That
would be a much harsher economy than §1 describes.

### `POWER_MAX` 240 → 300

The whole ballistics scale (`GRAVITY = 140`, `POWER_MIN = 30`, `POWER_MAX`,
`SHELL_RADIUS`, `BASE_CRATER_RADIUS = 7`, `BASE_BLAST_RADIUS = 9`) is unstated
in DESIGN, so it was chosen here to fit §2.5's distance table: base loadout
covers the 290-unit cap with headroom for wind and elevation, and flight times
land between 0.6s and 2.5s. 240 left too little headroom once Ballast narrowed
the range — see item 3.

### Target distance is clamped to what the loadout can reach

DESIGN §2.5 ramps target distance to a 290-unit cap. Two stacks of Ballast cut
the power range by 36%, which puts the band-13-and-later distances past the
shell's maximum range at *any* angle. The generator cannot fix that by
reshaping terrain, so it regenerated eight times, fell back to `Flat`, and
still produced unsolvable screens. The batch harness caught 32 of them in
5,000.

Placement is now `min(110 + band*6, 290, max_flat_range(loadout) * 0.75)`. This
is inside §2.4's requirement — "clearable in one shot with the player's
*current* loadout" — but it does mean a heavy Ballast build sees closer
targets. That is a legible consequence of the pick, and the alternative is
screens it cannot solve.

### `f64` heightfield buffers, not `f32`

DESIGN §2.1 says `Vec<f32>`. The brief's determinism constraint says `f64`
throughout the sim and is marked non-negotiable, so it wins. Quantization to
integers still happens only at render time.

### `MUZZLE_HEIGHT = 6`

Measured off the art rather than picked: `gen_sprites.py::tank` puts the barrel
pivot at sprite row 3 of a 12-row chassis whose lowest drawn row is 11, and the
reference scene seats that chassis two rows into the ground.

---

## 2. Gaps the spec left, and how they were filled

- **Solvability with more than one target.** DESIGN §2.4 says "≥ 3 candidates
  survive" but §2.3 puts up to three structures on a screen, and one shell
  cannot fell three. The gate is therefore per target: every standing structure
  needs three or more shots that would fell it. A screen where one target has
  twenty answers and another has two is rejected.
- **Candidates are verified against the damage model, not a re-derivation.**
  A candidate survives only if simulating it leaves the target reading
  `Collapsed`. Solvability cannot drift away from what the game actually does.
- **Aim points.** The 40-sample sweep is aimed at the box centre, as specified.
  Two more sweeps aim at the footprint base and just under the top, because
  undermining is a real kill path and `Overhang` has no direct answer at all.
  The histogram reports the centre-aim count only, so it stays a readable
  number instead of saturating.
- **`Overhang` in a heightfield.** One height per column cannot represent an
  overhang. It is built as a thin wall immediately in front of a narrow pocket,
  which does the same job: nothing on a direct arc gets in, and it takes
  ricochet, airburst or tunneling to solve. Gated on the loadout, per §2.4.
- **Reserve cap.** DESIGN §4 says Forced March "drops" the cap to 15 without
  stating a starting cap. The default is left uncapped, because a 40-shell
  ceiling would eat most of a Depot on the very first draft.
- **Draft picks are in the shot log.** DESIGN §3 says seed plus shot log
  reconstructs a run and nothing else is persisted, but a draft is a player
  decision. Picks ride in the same `(screen_index, angle_ticks, power_ticks)`
  triple with `angle_ticks = -1` marking "took offer N".
- **`Phase::Cleared`.** `fire` no longer pays the refund and advances in one
  step; the run sits on the cleared screen until `resolve_clear`. The
  presentation layer needs the collapse to animate on the screen that was
  cleared. Headless callers call it straight away.
- **Auger's budget** is the remaining thickness of the layer it entered (8
  units in sand, 22 in rock). Bedrock stops it, per §4, at the shell's own
  depth below the original surface, so a shell entering Rock near bedrock
  stops there with budget left.
- **Airburst's fuse** is horizontal distance from the muzzle, which is the
  number Rangefinder displays.
- **Storm Front** resamples the wind at the trajectory apex — one resample per
  shot, at a point the player can see.
- **Cluster submunitions** leave the detonation at 45% of the parent's impact
  speed, fanned across ±15°, carrying 40% damage. Only the damage share and the
  spread are specified.
- **Top-heaviness.** Support loss is scaled per silhouette (water tower 1.6,
  silo 0.8) so that undermining a bulb on legs fells it sooner than a solid
  cylinder. DESIGN §5.4 wants that difference legible; it was not a number.

---

## 3. Resolved second-stack behavior

- **Vernier.** The deterministic integer angle grid is now 0.05° per tick.
  Base input remains 0.5°, stack one grants 0.1°, and stack two grants 0.05°.
- **Salvage.** A qualifying crater remains within 30 horizontal units of a
  standing target. It refunds one shell per stack, so stack two refunds two.

---

## 4. Tuning observations from the batch harness

`cargo run --release -p salt-line-sim --bin batch` over 5,000 screens across 50
seeds, with the loadout evolving one seeded pick per band:

- Zero unsolvable screens, zero fallbacks to `Flat`, and generation almost
  never needs a second attempt (mean attempts 1.01–1.05 per template).
- Direct-aim candidate counts run 15–25 on average and fall with structural
  difficulty exactly as expected: `Flat` 25, `Chasm` 23, `Mesa` 22, `Ridge` 21,
  `Basin` 18, `Spires` 17, `Overhang` 12.
- Some late-band screens have **zero** direct-aim candidates and pass on the
  base/top aims alone. Those are the `Overhang` and `Basin` screens, which is
  the intent, but it means the direct-aim histogram is not the whole gate.
- **Band 0 has no wind at all.** `wind = band * 0.35` is exactly 0 for the
  first four screens, so the opening band teaches the arc with nothing to read.
  That may be deliberate. If not, a small floor (±0.4) would put a number on
  the HUD worth looking at from screen 1.
- Template and target-count weights now overlap for build testing: two-target
  screens begin in band 3, three-target screens in band 9, and later bands
  retain a small share of simpler counts. (Superseded by epoch 6: doubles now
  begin in band 2 and triples in band 4; see §6.) Eligible
  `Overhang` has signature-template weight comparable to `Basin` instead of
  half its weight, while its capability and solvability gates remain intact.
- The batch report now includes a deterministic solver-assisted economy
  baseline (spend/refunds/bonuses/reserve), clear tiers, modifier exposure,
  death screen/run length, and indirect-shot demand. A reduced 5-seed ×
  30-screen smoke run after this tuning validated 142 combat screens and 201
  targets with zero failures or fallbacks; 8 supply screens were skipped by
  the gate, and the ideal policy averaged 1.42 shots per clear.

---

## 5. Art gaps

Programmer art, per the brief, but two gaps affect play rather than polish:

- **All five silhouettes now have all three damage states.** ~~Only the water
  tower has them.~~ The tower is still the only one drawn by hand per state.
  The derrick, mast, silo and pylon derive `Leaning` by shearing the intact
  drawing about its own footing — base row fixed, top row displaced, footing
  bruised to the damage tone — and `Collapsed` from a wreck sized to their own
  silhouette rather than the tower's. Deriving rather than redrawing is the
  decision here: it keeps the damage states from drifting away from the
  geometry, at the cost of four leaning poses that are a shear and not a
  drawing. Replace them when real art lands.
- **No barrel strip for the Ballast or Forced March chassis.** The generator
  now emits base, sabot, cluster and auger strips; the two chassis variants
  reuse the base barrel, which is correct but means those builds read only from
  the hull.

---

## 6. Sim epochs

`SIM_EPOCH` (`salt-line-sim/src/lib.rs`) versions the sim's outcomes, as
DESIGN §9.3 proposes. `tests/golden_runs.rs` hashes the pinned screen digests
together with the golden-run corpus (`tests/golden_runs.txt`) and asserts the
result is the current epoch's entry in `EPOCHS`. When it fails: bump
`SIM_EPOCH`, regenerate the corpus, append `(epoch, digest)` to `EPOCHS`
(never edit an entry that has reached `main`; an epoch still only on an
unmerged branch may have its digest replaced in place) and add a line here with the digest; a test
checks that every digest in `EPOCHS` appears in this section.

Saved logs use the text form of `replay::encode_log`, which starts with a
format version (`v2`). The parser rejects any other version, so an epoch-1
log, which has no fuse or offset to give, is refused rather than replayed
with guessed values.

- **Epoch 1:** everything before epoch 2. Shot logs held only screen, angle
  and power, so a run that used Airburst or Enfilade did not replay: the
  fuse and the emplacement offset were never recorded.
- **Epoch 2:** each logged shot also records the fuse (tenths of a unit) and
  the emplacement offset (whole units), and both are in `state_hash`. The
  live game now holds the fuse on a 0.1-unit grid (a new screen's default
  fuse is rounded to it) and the offset on whole units, so a replay applies
  exactly the values that flew. Screen generation is unchanged — the pinned
  screen digests did not move — but every run's final hash did, and the
  determinism bin's digest changed from `0xe58dfc4036186a6a` to
  `0x6d19f4bc6cd194fa`. Epoch digest: `0xce60de848487e4a5`.
- **Epoch 3:** three rule fixes. A Scavenger Rites supply screen just
  before a band start (15, 35, …) was walked through without the band-end
  draft check, so the drafts at 16 and 36 never opened; they now open, with
  the run drafting on the supply screen. A collapsed structure is no longer
  re-resolved, so debris refilling its footprint cannot stand it back up
  (DESIGN §2.7). Auger's bedrock check reads the layer at the shell's depth,
  not the column's surface, so bedrock stops a tunnelling shell wherever it
  reaches it (DESIGN §4); the check is made only while the shell is
  underground, so one that tunnels out over a deep crater flies on. The
  terrain-free generation fallback is now gated on the solver like every
  other rung, with a level, windless single-target rung below it that is
  solvable by construction (DESIGN §2.4). A screen that reaches the fallback
  and fails the gate now comes off the level rung instead of being shown
  unproven; no such screen has been found for any loadout a draft can build,
  so no pinned screen, golden run or digest moved for it. Otherwise screen
  generation is unchanged — the pinned screen digests did not move — but a
  log that crosses a band start after a supply screen, tunnels with Auger,
  or piles debris over a wreck replays differently. The determinism bin's digest changed from
  `0x6d19f4bc6cd194fa` to `0x8e29ed6154deaf03`. As with any bump, a saved
  in-progress run from epoch 2 is discarded on update: the resume file
  refuses an older epoch rather than replay it under new rules. Epoch
  digest: `0x64713d327a31a792`.
- **Epoch 4:** screen 3 features a radio mast behind a ridge that blocks a
  straight sightline, and screen 6 features a silo across a chasm. Other early
  screens still draw from their band's template and structure weights.
  The normal solvability gate and fallback ladder still apply. This changes
  generated screens and the outcomes of saved shot logs, so an in-progress
  epoch-3 run is discarded on update. Epoch digest: `0x1104ab329b376f2c`.
- **Epoch 5:** a direct shell hit on an already collapsed wreck clears its
  drawing and collision box. It still counts as a downed target, and the
  debris mound remains destructible terrain. A shot log that strikes rubble
  can now replay differently, so epoch-4 in-progress runs are discarded.
  The epoch digest includes an explicit rubble-clearing probe because the
  scripted golden runs do not reliably take that second shot. Epoch digest:
  `0x9c4ef5d00758cebe`.
- **Epoch 6:** the target-count mix introduces doubles in band 2 (screen 9)
  and triples in band 4 (screen 17), then ramps them through band 9, instead
  of guaranteeing single-target screens for the first twelve. Doubles come a
  band after the first wind and triples two bands after that, so no two
  difficulty-table dials turn on the same screen (DESIGN §2.5; band 2 also
  unlocks the Mesa template, which that table does not count as a dial). The golden corpus gains a
  ninth run (seed `0xb`) because none of the first eight drafted on a supply
  screen under the new mix. Existing epoch-5 runs are discarded on update
  because generated screens and outcomes change. Epoch digest:
  `0x0e222e08a33eaf8a`.

---

## 7. Local versus prototype (DESIGN §11)

Choices the owner's brief left open, made the simple way:

- **No self-hits.** A shell that lands on its own gun scores nothing (the
  screens call it OOPS). The shooter's own gun is not a collision box; the
  other gun is.
- **Openers alternate by round**, left first, rather than the loser opening.
  (Equal shots, below, has since removed most of what opening is worth.)
- **The gate checks three winds** (calm and both extremes), not every turn's
  draw. Craters change the ground during a round anyway, so no per-turn gate
  could hold; this proves the screen fair as dealt.
- **Gun box** is the 26-pixel chassis around the barrel pivot, three rows
  above it and nine below (two of those in the ground), measured off
  `tank_base.png` as it is seated.
- **Title entry.** A LOCAL VERSUS row on a touchscreen only; with keys it is
  the bindable `versus` action (`2`, pad LB). Adding a keyed row would have
  changed the pinned title snapshot, which the brief asked to keep.
- **The score and wind wait for the landing.** The rules score a shot when it
  is fired; the HUD shows the score and the wind as they were until the
  shell comes down, like SCREEN CLEAR in a run.
- **Nothing is saved.** No resume, no board, no onboarding lessons learned
  from versus input.
- **Match seed** mixes the wall clock at launch (read by the adapter; the core
  reads no clock), the daily seed and a per-session match count, so no two
  launches deal the same first match.
- **One touch, one screen.** Every versus screen change spends the fingers
  down and ignores new touches for 0.3 s, after a Pixel playtest where one tap
  on START also readied the turn and fired. Single-player screens are not
  changed.
- **Mirrored controls.** Left and Right move either barrel's tip the same way
  across the screen, so for the right gun they run the other way in angle.
- **Rematch goes through the pre-match screen.** REMATCH opens the aim-help
  choice again rather than dealing the next match at once, so a player can
  change their mind about help between matches. The choices are session
  state, so the last match's are already set; nothing new is stored.
- **Kid readability (DESIGN §11.4).** Amber left, sky blue right: two of
  the sixteen slots, chosen for hue distance and red-green colour blindness.
  The blue *is* the duel sky's horizon band (`SKY_LO`, the Day phase's
  bottom band), with no contrast at all there, which is why everything in a
  side colour sits on a plate or a one-pixel `UI_BG` backing (`raster::Ink`):
  the gun tags, the gun name, the callout, the shell head, the flashes, the
  landing mark, the hit outline and edge flash. A render test checks that no
  amber pixel touches the field; both sides draw through the same code.
  Versus moves its dim text to `ROCK`, its wind digits to plain text and its
  windsock to brick and sand, so the colours mean only a side. This relaxes
  §5.2's reservation of amber for wind and the low-shell counter, in versus
  only. Digit scores are gone in favour of pips; the round number stays as
  text bottom right.
- **Big type is the small face scaled.** HIT/MISS at 4x, names at 2x
  (`Canvas::text_small_scaled`), rather than a new large face: one glyph set
  to keep legible, and integer blocks keep it on the pixel grid. Versus only;
  §5.6 still governs everything else.
- **Cues beyond colour.** A HIT callout sits over the scorer's half and,
  like the handoff and result plates, has an arrowhead on that side.
- **OOPS.** A shell that misses the other gun but reaches its own is called
  OOPS, plain, with no pip and no ding. The godot-side `TurnReport` gains
  `self_hit`; the sim is unchanged.
- **The ding waits 80 ms** after the landing (`DING_DELAY`), so it is not
  masked by the impact and collapse sounds on the same frame. Collapse stays.
- **The HIT/MISS status line became the callout.** The callout is derived
  from the match state (`VersusPlay::callout`), so it lasts exactly the
  landing hold and goes when the phone is passed; the status line is left
  for other notes and drawn in plain text in versus.
- **Sounds.** `versus_hit` (A5 then E6) and `versus_win` (C-E-G-C up) come
  from `tools/gen_audio.py` like the rest, standard library only. The hit
  ding plays with the landing; the fanfare plays once, with the result plate.

### The opener's edge and the repeated shot (October 2026)

Owner playtest: once a player had found the range they could fire the same
shot again, and whoever shot first had the edge. The brief's first idea, wind
every turn and alternating openers, was already in the rules, so the levers
were measured instead with a new duel harness (`just duel`,
`crates/salt-line-godot/examples/duel.rs`): two bots that re-aim from their
last landing, 400 matches per row, width 320. `kid` is a young player with
aim help (shaky hand, slow learning, little wind reading, the odd
overcorrection or change of angle), `steady` a grown-up without it; both are
guesses, so compare rows rather than reading them as forecasts.

| rules, kid v kid | shots per hit | turns per round | repeat hits | opener wins round | past 3 hits | turns per match (mean, p90) |
|---|---|---|---|---|---|---|
| as shipped in #60 | 1.77 | 7.7 | 96% | 59.0% | 0% | 19.2, 26 |
| equal shots only | 1.66 | 9.1 | 96% | 46.7% | 22% | 23.1, 34 |
| **equal shots + rolling guns (16, ladder −1..+2)** | **2.28** | **11.5** | **40%** | **46.5%** | **16%** | **28.8, 42** |
| same, step 20 (hangs 3 px off screen) | 2.58 | 12.8 | 22% | 48.2% | 15% | 32.0, 47 |
| same, step 16, forward only (ladder 0..+3) | 2.16 | 11.3 | 37% | 45.6% | 20% | 28.5, 40 |

| rules, steady v steady | shots per hit | turns per round | repeat hits | opener wins round | past 3 hits | turns per match (mean, p90) |
|---|---|---|---|---|---|---|
| as shipped in #60 | 1.44 | 6.6 | 96% | 67.7% | 0% | 16.7, 22 |
| equal shots only | 1.28 | 11.5 | 93% | 48.5% | 37% | 28.0, 48 |
| **equal shots + rolling guns** | **1.90** | **12.8** | **51%** | **51.3%** | **33%** | **31.8, 52** |

Kid against steady, the kid's share of matches (the mean over the two ways
round, since the side that opens round 1 differs):

| rules | kid as left | kid as right | kid's share |
|---|---|---|---|
| as shipped in #60 | 30.8% | 25.0% | 27.9% |
| equal shots only | 14.8% | 22.2% | 18.5% |
| **equal shots + rolling guns** | **24.2%** | **24.8%** | **24.5%** |

"Repeat hits" is how often the shot that just hit, fired again unchanged on
that side's next turn, would hit again. "Past 3 hits" is the share of rounds
that a tie carried beyond three.

- **Wind was not the lever.** A 45° shot across a duel flies about two
  seconds, so ±2.5 drifts it about ±5 units against a hit window 44 units wide
  (the 26-unit box plus 9 of reach each side). In the first round of
  measurements (an earlier, stronger kid bot), wind 2–7 with a floor of 2 only
  took repeat hits from 96% to 85%, and spoiling repeats with wind alone needs
  winds well past single-player's cap, which would swamp every shot.
  `WIND_MIN` exists as a lever; it and `WIND_MAX` keep their old values.
- **A shorter reach makes every shot harder, not repeats rarer** (reach 4:
  repeat hits 94%). `HIT_REACH` stays at the blast radius.
- **Rolling the guns** is what spoils a repeat. Both guns move together, back
  or forward, so the duel stays mirrored and one value (`Screen::muzzle_x`)
  still places both guns. At a step of 16 a repeated hit lands about as often
  as a fresh aim does (kid 40% against 1 / 2.28 = 44%; steady 51% against
  1 / 1.90 = 53%), so repeating the last hit stops being worth anything. A step of 20
  would make repeating worse than aiming afresh, but one step back would put
  the pivot at x = 10 and the chassis 3 pixels off the screen; 16 is the
  longest step that can go back once and stay on screen, so both ways are
  reachable from home.
- **Even odds, both ways.** A forward-only ladder (guns only ever start by
  closing in) measured about the same (37% repeats, 45.6% opener rounds), so
  there is no case for it over a random direction.
- **They roll at the end of a pair**, not straight after the hit. Rolling at
  once left the opener winning 55–61% of rounds even with equal shots: the
  side answering a winning hit had to aim at guns that had just moved.
- **Equal shots** removes the opener's edge within a round: the side that did
  not open gets its shot whenever that shot could still tie. If its answer
  ties things past three, play goes on in pairs until one pair ends with
  someone ahead. An opener two or more clear at the mark (3-0, 3-1) wins on
  its own shot rather than handing the other side a shot that cannot matter.
  On its own equal shots hurts a weaker player against a stronger one (the
  stronger rarely misses the answering shot); with rolling guns the kid's
  share comes most of the way back (24.5% against 27.9% under the #60 rules).
- **Fresh ground under rolled guns.** A gun could roll into a stretch the
  other side had cratered: the two pivots were up to 16 units apart after a
  roll, against under 5 on the generated ground (what is left of its
  asymmetry). Rolling now fills the craters under both chassis, so both sit
  on the generated ground; after a roll the pivots differ by 1.4 on average
  and 4.6 at most.
- **The re-gate is cheap.** A shot that rolls the guns takes about 0.12 ms on
  the desktop in a release build (0.34 ms at most), against microseconds for
  one that does not; the duel gate saturates its cap of 8 shots almost at
  once.
- **Owner toggle.** ROLLING GUNS on the pre-match screen (on by default, kept
  for a rematch) plays a match with `GUN_SHIFT` at zero, so the rule can be
  tried both ways. Equal shots stays on.
- What is left: at the match level, the side that opens round 1 won 44–47%
  of same-bot matches under the new rules (±2.5% at 400 matches).
- **The guns are found moved after the handoff.** During the HIT or MISS
  hold the guns are drawn where the shot found them; the move happens behind
  the pass-the-phone plate. Each side's landing mark and last arc move with
  its gun. The handoff plate then says GUNS ROLLED CLOSER or APART
  (DESIGN §11.4). The guns are not eased to their new spots: the screen
  shown is a borrowed `Screen`, and easing would need an owned copy rebuilt
  every frame for the field, the gun tags and aim help alike, for a move the
  plate already announces.
- **Overtime cues** (DESIGN §11.4) come from one rules query,
  `Match::stakes`: LAST SHOT when the side to move must answer a winning hit
  and a hit would tie, TIED when a pair ended level past three, HIT TO WIN
  whenever a hit by the side to move would end the round in their favour
  (in overtime, and at the ordinary deciding moments such as 2-2 after the
  opener missed). HIT TO WIN asks `round_winner` itself, so the cue and the
  rule cannot drift apart, and the HIT callout and the handoff plate read
  the same query. Pips past three grow both rows to the higher count rather
  than adding a +1 marker, up to five; past that the score is two numbers
  in the sides' colours, so the plate stays clear of the pause button and
  inside the 200-pixel handoff and result plates.

## 8. Attract demo (DESIGN §12)

The owner settled two things: the idle demo is on by default with an opt-out,
and the loop alternates a run and a duel. Everything else was decided here.

- **Set aside, not copied.** The demo reuses the run and versus code rather
  than a second renderer, so it moves the player's whole `Play` (and the
  versus state and buffer width) into its own stash and moves it back. A copy
  would have needed `Play` to be `Clone` and left two runs in memory that could
  drift; a move means the player's run cannot be reached at all while the demo
  runs. The CONTINUE offer (`run_file`) is never moved: nothing in the demo
  reads it.
- **Guards at the source, a backstop at the end.** `persist_run`,
  `finish_run`, `learn`, `write_settings`/`tick_settings` and
  `retire_last_crash` each return early in the demo. `persist_run` is the one
  that matters most: after a quit to the title `run_persist` is still set for
  the suspended run, so without the guard the demo's run would have been
  written over `run.txt` on its first shot. (Removing that guard fails four of
  the demo tests.) `Game::tick` then strips any disk effect a demo frame still
  asked for and `debug_assert!`s there was none.
- **Pending settings are flushed on entry**, while writes are still allowed,
  so "no write during the demo" holds without losing a change made the frame
  before.
- **The demo's run is jumped, not played to.** Each curated run starts on a
  chosen screen, as `--salt-screen` does, because the early bands are plain
  ground and one structure. Screens were picked from headless screenshots of a
  dozen seeds at several screens: `DUNE` 7, `TOWER` 12, `1984` 8, `MESA` 16
  (zero-based).
- **Near misses had to search angle as well as power.** A first cut moved only
  the power, up to ±240 ticks, and on fast flat shots (power 800–900) every
  step still brought the target down, so the demo fell back to its short lob
  on most first shots. Steeper and flatter barrels (±170 to ±420 angle ticks)
  after the power steps found a real near miss for every miss in the four
  curated run legs; the
  run-leg test now fails if more than a quarter of the shots are the lob.
- **Pacing and balance.** 0.8 s per aim (the barrel eases to it over
  0.55 s, then holds), 1.6 s of draft cards, 0.8 s of heading plate, 1.8 s
  of result plate, 1.2 s on a run's last screen. The first cut (3 screens a
  run, 2 rounds or 90 s a duel) gave duels 75–80% of each cycle, so review
  rebalanced it to 4 screens a run and 1 round or 40 s a duel. Over the
  first eight cycles a run leg now takes 17–27 s and a duel 20–42 s (a 3-0
  round is the short end), a little over half of each cycle.
- **Taps in a duel.** Every versus screen change settles touches for 0.3 s
  ("one touch, one screen"), and the demo's duel changes plate several
  times a turn, so about one tap in five on a demo duel was ignored. In the
  demo only its own start settles touches now; a test taps on the frame after
  each of a duel's first plate changes and expects the title.
- **Unwatched demos end.** An idle-started demo ends when the window or app
  loses focus (`focus_lost`, `app_paused`), does not start while unfocused
  (the adapter's FOCUS_IN calls `focus_gained`), and gives the title back
  after 10 minutes, after which the title waits for input before trying
  again. Analog triggers, which no action reads, now reach the core as
  `Event::Trigger` on the pull past halfway, so they count as input too.
- **Rotation across demos.** The next demo in a session starts on the leg
  after the last one shown, so a second look is not the same run again.
- **Exit is any new input, counted from events.** Held keys are not input to
  stop on (the key that chose DEMO is often still down the next frame), but
  any key event is, read by the game or not. A finger going down during the
  touch settle after DEMO was tapped is that tap reported again, so it does
  not stop the demo it just started.
- **Screen sleep.** Godot's `keep_screen_on` defaults to on, which held the
  screen on everywhere, title, menus and pause included: a phone left on the
  title never slept. The demo first worked around that by releasing the hold
  for an idle demo and restoring the setting after. Since then the project
  sets `keep_screen_on` off, and the core holds the screen on only for live
  play (`Playing`, the draft included), a versus match not paused (its
  plates and results too: both players are looking at the phone) and a
  menu-started demo; everything else, an idle demo included, lets the
  screen sleep on the OS timeout (DESIGN §12.4 has the table).
  `Game::screen_awake_wanted` derives the state from the mode, the pause
  and the demo, and `take_effects`, which every effect leaves through,
  sends `ScreenAwake(bool)` only when it changes, so no transition pushes it
  and the focus and app-pause notifications (which return their effects at
  once) let go too. Out of focus or app-paused counts as unwatched: a draft
  or a versus plate, which focus loss does not pause, lets go until focus
  comes back. The old three-way `Awake` (keep, let sleep, as project) and
  the adapter's launch-time read of the project setting are gone.
  `app/awake_tests.rs` checks every mode, pause and resume, both demos,
  focus, app pause and the error screen, with a stand-in adapter that fails
  on any repeated or out-of-step effect.
- **30 fps.** The demo uses the menus' `IDLE_FPS` cap rather than play's
  uncapped rate. It also stays out of `is_active_play`, so it never asks for
  Linux GameMode.
- **Title layout.** The keyed title gains a `D  DEMO` row (the pinned title
  snapshot changes by that one row, which the brief allowed). The touch title
  instead gets a DEMO plate in the top-left corner, mirroring MUS ON/OFF: a
  further plated row would have pushed DEVELOPMENT PREVIEW off the bottom of a
  phone's title once a run is kept for CONTINUE and the board has a best.
- **Duel plates in the demo** keep the side heading and score but drop TAKE
  THE PHONE - NO PEEKING and READY, and do not dim the board.
- **Banner text** follows the input in use: `TAP TO PLAY` on a touchscreen,
  `PRESS ANY KEY` otherwise.
- **Snapshots.** Changed: the title (the new row) and the save format (the
  pinned save now sets `idle_demo 0`). New: the demo's first run frame and
  its first duel handoff, banner and all.
