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
(never edit an existing entry) and add a line here with the digest; a test
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
  band after the first wind and triples two bands after that, so no two new
  dials turn on the same screen (DESIGN §2.5). The golden corpus gains a
  ninth run (seed `0xb`) because none of the first eight drafted on a supply
  screen under the new mix. Existing epoch-5 runs are discarded on update
  because generated screens and outcomes change. Epoch digest:
  `0x0e222e08a33eaf8a`.
