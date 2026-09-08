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
  units in sand, 22 in rock). Bedrock stops it, per §4.
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

## 3. Two draft picks that currently do nothing on their second stack

Both are listed in DESIGN §4 with `Stacks: 2`, and in both cases the second
stack has no effect as specified. Left alone rather than invented on, but they
are dead picks and worth a decision:

- **Vernier.** Stack one takes angle quantization from 0.5° to 0.1°, which is
  the tick grid's floor. A second stack has nothing left to tighten. Either
  make it one stack, or make the grid 0.05° and let the second stack reach it.
- **Salvage.** "Misses that still crater within 30 units of a target refund 1."
  Stack one grants that; stack two grants the same 30 units and the same 1
  shell. Refunding 2 on the second stack is the obvious reading, but it is not
  what the table says.

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
- Template share across a 100-screen run: `Ridge` 23%, `Flat` 17%, `Chasm` 16%,
  `Mesa` 13%, `Spires` 11%, `Basin` 7%, `Overhang` 4%. `Overhang` is rare
  because it needs the unlock *and* band 5; if it should read as a signature
  screen, its weight wants raising.

---

## 5. Art gaps

Programmer art, per the brief, but two gaps affect play rather than polish:

- **Only the water tower has all three damage states.** Every other silhouette
  falls back to its intact sprite while `Leaning` — so the state change is
  visible only in the shifted bounding box, not in the drawing — and to a
  procedural rubble mound when it collapses. `Leaning` is a real state that
  changes the shot you need; it should look like one.
- **No barrel strip for the Ballast or Forced March chassis.** The generator
  now emits base, sabot, cluster and auger strips; the two chassis variants
  reuse the base barrel, which is correct but means those builds read only from
  the hull.
