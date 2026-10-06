# Salt Line — Design Spec

Working title. Single-player artillery roguelike on an endless procedural horizon.
Deterministic Rust simulation core, Godot 4 presentation layer via gdext.

---

## 1. Run structure

A run is an unbroken left-to-right traversal. There is no level select, no
between-screen menu, no death. The camera pans forward when a screen clears.

| Unit | Meaning |
|---|---|
| **Screen** | One emplacement + one target set. Roughly one viewport wide plus overscan. |
| **Band** | 4 screens. A modifier draft fires at the end of each band. |
| **Run** | Continues until the shell reserve hits zero. |

**Starting reserve:** 40 shells.

**Refund on clear** (per screen, based on shots taken to clear it):

| Shots used | Refund |
|---|---|
| 1 | +3 |
| 2 | +2 |
| 3 | +1 |
| 4+ | 0 |

Net drain is deliberate. A perfect player still trends slowly downward without
economy modifiers, so the draft matters. Tune the refund table first when
balancing — it is the single strongest lever on run length.

**Measured (batch harness, October 2026).** These numbers come from
`salt-line-sim`'s `batch` binary, not from players. Its hit chances are
assumptions (perfect 100%, steady 80%, rough 55%), so compare them with each
other, not with real players. All use 100 seeds and flat aim. Reproduce with
`just balance 100 <screens> <skill> <base|evolving>` and
`just balance-sweep 100 40 base 65 80 5`. CI holds three related 40-screen
configurations inside a band (the "shell economy stays in its band" step).

- **A perfect player builds a surplus first, then drains.** Without drafts,
  the reserve rises about +2 a screen in bands 0–1, levels off near 67 by band
  6, and starts falling at band 9 (about −0.6 a screen, nearer −1 by band 21).
  The downward trend this section describes is real, but it starts around
  screen 36, not from the first screen. Over 100 screens, 4 of 100 perfect
  runs are dry by around screen 97. Whether the early surplus is intended is
  an open question for the owner; the measurement doesn't settle it.
- **About 75% accuracy carries almost every run through 40 screens without
  drafts.** With a flat hit chance, 53 of 100 runs run dry at 65%, 19 at 70%,
  1 at 75% and none from 80%.
- **Drafting carries long runs.** Over 100 screens a steady player runs dry in
  every run without drafts (around screen 59), against 8 of 100 with them.
  For a rough player it is 100 against 61. Salvage's refund share comes from
  probing where near-solution misses land (about 53% of misses on one-target
  screens, 78% on three-target ones); Chalk Marks' effect is assumed. See
  `batch.rs`.

**Score:** screens cleared. Tiebreak on shells remaining, then total shots.
Never show a score during the run. Show only the shell count and the screen
index, in the corner, small.

---

## 2. Terrain generator

### 2.1 Representation

A **heightfield**: `Vec<f32>` of surface height, one sample per screen pixel
column. With the internal resolution fixed at 180px tall (§5.1), one world unit
equals one pixel and `TERRAIN_STEP = 1.0`. Do not sub-sample below 1.0 — at this
resolution a crater is only a handful of pixels wide and finer sampling buys
nothing you can see.

Heights are stored as `f32` and only quantized to integers at render time. The
simulation needs sub-pixel precision for collision even though the display has
none; rounding in the buffer makes shallow trajectories collide inconsistently.

Three parallel buffers per screen:

```
base:     Vec<f32>   // generated, immutable
scar:     Vec<f32>   // accumulated subtraction, mutable
material: Vec<u8>    // layer id at surface, recomputed from base - scar
```

Rendered surface is `base[i] - scar[i]`. Keeping them separate means a screen
can be regenerated from its seed alone and replayed by re-applying the shot log
to a fresh scar buffer. That property is what makes seed-sharing exact.

### 2.2 Seeding

```
run_seed:    u64              // user-visible, shareable
screen_seed: u64 = splitmix64(run_seed ^ (screen_index * 0x9E3779B97F4A7C15))
```

Each screen generates independently from its own seed. No sequential state
carries forward, so you can generate screen 400 without generating 1–399 — which
you will want for debugging, for the level inspector, and for validating the
difficulty curve in a batch run.

Use PCG32 or SplitMix64 explicitly. Do not use `rand::thread_rng` anywhere in
the sim crate, and do not use `HashMap` iteration order in any code path that
feeds generation.

### 2.3 Generation pipeline

1. **Base ridge.** 4 octaves of value noise, lacunarity 2.0, gain 0.5.
   Amplitude scaled by band difficulty (§2.5). Value noise, not Perlin —
   cheaper and the slightly blockier character suits the flat-color art.

2. **Domain warp.** Offset the sample coordinate by a second low-frequency noise
   field, strength ~15 units. This is what stops the terrain reading as
   symmetric hills. Cheap, high impact — do not skip it.

3. **Shape template.** Select one from a weighted table by band. The template is
   a coarse height multiplier curve applied over the base:

   | Template | Character | Available from |
   |---|---|---|
   | `Flat` | Gentle roll, target in the open | Band 0 |
   | `Ridge` | One tall obstruction mid-screen | Band 0 |
   | `Chasm` | Gap with the target on the far lip | Band 1 |
   | `Mesa` | Flat-topped plateau, target on top | Band 2 |
   | `Spires` | Two or three narrow columns | Band 3 |
   | `Overhang` | Target tucked under a lip (see §2.4) | Band 5 |
   | `Basin` | Target below the firing line | Band 6 |

   For the opening sequence, screen 3 is `Ridge` with a radio mast, and screen
   6 is `Chasm` with a silo (screen numbers here are player-facing, starting at
   1).
   The ridge must block the straight sightline to the first target. All other
   screens use the band weights, and every featured screen still passes the
   usual solvability gate before it is shown.

4. **Material layering.** Assign layers by depth below the local surface:
   `Sand` (0–8 units), `Rock` (8–30), `Bedrock` (30+). Structures are placed as
   `Steel` columns. Material controls crater radius multiplier (§2.6) and
   determines what the tunneling modifier can pass through.

5. **Target placement.** 1–3 targets, count rising with band. Placed at
   template-defined anchor points, snapped to the surface. Minimum horizontal
   separation of 40 units so a single blast cannot trivially clear two.

6. **Solvability validation.** See §2.4. Reject and regenerate with
   `screen_seed + 1` on failure. Cap at 8 attempts, then fall back to a `Flat`
   template — a boring screen is better than a hang. The fallbacks are
   validated as well; see the ladder in §2.4.

### 2.4 Solvability

Every screen must be clearable in one shot with the player's *current* loadout.
Not "probably" — provably, before the screen is shown.

Projectile with gravity `g` and constant horizontal wind acceleration `w`, from
origin to target `(dx, dy)`:

```
x(t) = v·cos(θ)·t + ½·w·t²
y(t) = v·sin(θ)·t − ½·g·t²
```

For a fixed flight time `t`, this is linear in the velocity components — solve
directly for `vx, vy`, then check `v = hypot(vx, vy)` falls inside the power
range and `θ = atan2(vy, vx)` inside the angle range. Sweep `t` over 40 samples
across the plausible flight-time range; keep every `t` that yields a valid
`(θ, v)`.

For each candidate, raycast the trajectory against the heightfield at the sim
timestep. A candidate survives if it reaches the target without prior collision.

**Accept the screen if ≥ 3 candidates survive.** One is too tight — it produces
screens with a single pixel-perfect answer, which reads as unfair. Three gives a
window. Log the surviving-candidate count per screen; it is your best proxy for
difficulty and worth graphing over a 200-screen batch.

`Overhang` screens intentionally fail the direct check and are only offered once
the player holds a modifier that unlocks them (ricochet, airburst, or
tunneling). Gate template availability on the loadout, not just the band.

**Fallbacks are validated too.** When the drawn templates run out (§2.3 step 6)
generation steps down a ladder, and every rung goes through the same gate:

1. Eight drawn templates, each on the next seed.
2. Eight `Flat` screens on a separate run of seeds.
3. No template: the base ridge flattened to a gentle roll, one target.
4. Level ground, no wind before or after the apex, one Pylon at half the
   loadout's flat-ground range (or the band's distance, if nearer).

Rung 4 is solvable by construction for any loadout the game can hold: nothing
stands in the way, nothing blows the shell off line, and half range leaves a
low and a high answer across a wide band of power, whatever Featherweight,
Ballast or Sabot have done to the range and however coarse the angle quantum.
It is still validated, to prove the construction rather than trust it. If it
ever failed, a debug or test build would panic rather than show the screen, and
the batch harness would fail. A supply screen (Scavenger Rites) has no targets,
so there is nothing to prove; it is exempt by nature, not by oversight.

### 2.5 Difficulty curve

Drive everything from `band = screen_index / 4`, with the primary axes ramping
on different schedules so the player never feels one dial turning:

| Axis | Schedule | Cap |
|---|---|---|
| Target distance | `110 + band·6` units | 290 |
| Wind magnitude | `band·0.35`, resampled per screen | 7.0 |
| Terrain amplitude | `1.0 + band·0.05` × base | 2.2 |
| Target count | Weighted mix: first doubles at band 2; first triples at band 4; triples reach 25% at band 6 and 50% at band 9 | 3 |
| Elevation delta | Widens `±(8 + band·3)` | ±70 |

These are scaled to the 320×180 internal buffer, where the emplacement sits at
x≈30 and the far edge is x≈310. The distance cap keeps the target on screen at
base zoom without Long Lens. If you raise the internal resolution later, scale
this whole table by the same factor rather than retuning by feel.

Wind is **resampled per screen** and displayed as a windsock that flies the way
the wind blows, beside its strength as a number. The sock is the same size at any
strength and hangs limp when there is no wind to show. Reading a number and
converting it to a correction is the core skill; an arrow sized by strength
would do that work for the player.

Past band 20, stop ramping magnitude and start ramping template weight toward
`Spires` / `Overhang` / `Basin`. Infinite numeric scaling turns into noise;
structural difficulty stays interesting.

### 2.6 Craters and persistence

On impact at `x`, radius `r`, apply a smooth subtraction to the scar buffer:

```
scar[i] += r · (1 − (d/r)²)²      for |d| = |x − pos[i]| < r
```

The squared falloff gives a rim that reads as a crater rather than a notch.

**Radius by material:** Sand ×1.4, Rock ×1.0, Bedrock ×0.55, Steel ×0.3 plus
structural damage state.

Scars persist for the life of the screen. They do not persist across screens —
you never return. The consequence is local and immediate: a short shot into the
hill in front of you can lower it enough to open a flatter line, or crater the
approach so the only remaining line is steeper. Both outcomes are legitimate.
Do not "fix" players digging themselves out of trouble; that emergent recovery
is the most interesting thing the system does.

Bedrock slows excavation (crater radius ×0.55) but does not stop it: a column can be dug
through bedrock down to `WORLD_FLOOR`, and no further. Never allow terrain to be
excavated below the world floor, or the heightfield inverts and collision breaks.
Digging through bedrock is slow on purpose; it is the expensive end of the Demo build
and of the deep caches in §8.

### 2.7 Structural collapse

Targets are vertical structures anchored to the heightfield across a **footprint**
of 5–11 pixel columns. Each structure tracks:

```
integrity:  f32       // 1.0 → 0.0
support:    f32       // fraction of footprint columns still at original height
state:      Intact | Leaning | Collapsed | Cleared
```

Two independent kill paths, and having both is the point:

- **Direct.** Blast within the structure's bounding box reduces `integrity` by
  blast overlap × material factor.
- **Undermining.** Craters inside the footprint reduce `support`. Recompute once
  per shot, after every detonation that shot caused (a Cluster shell's
  submunitions all land before states are resolved), as the mean remaining height
  across footprint columns, normalised to the original.

**State thresholds:**

| Condition | State |
|---|---|
| `integrity > 0.35` and `support > 0.6` | Intact |
| `integrity ≤ 0.35` or `support ≤ 0.6` | Leaning |
| `integrity ≤ 0` or `support ≤ 0.3` | Collapsed |
| Direct shell hit on an already collapsed wreck | Cleared |

Leaning is a real state, not a cosmetic one: a leaning structure tilts toward the
weaker side of its footprint, which shifts its bounding box by several pixels and
changes the shot you need. It also grants no refund — the screen is not cleared
until every target is down (Collapsed or Cleared).

Collapse animates as a topple in the lean direction, then the debris adds a small
positive mound to the scar buffer where it lands. Terrain gets slightly higher
where a structure fell. That is a nice consequence to leave in.

Collapse is permanent. Debris or later craters that raise or refill the footprint
never stand a collapsed structure back up.
An already collapsed wreck still catches a shell. A direct follow-up hit clears
the wreck and its collision box, opening the line to targets behind it. The
debris mound is still ordinary terrain and can be cratered.

Undermining is what gives the DEMO build a win condition of its own. A player
running Auger and Cluster never needs to hit a target directly — they excavate
the ground out from under it. Do not balance that away.

---

## 3. Ballistics core

- Fixed timestep, `dt = 1/120`, semi-implicit Euler. No variable timestep in the
  sim, ever — render interpolates, sim does not.
- Wind is an **acceleration**, not a velocity offset. Drift grows with the square
  of flight time, so long shots are disproportionately wind-sensitive. This is
  the single best decision in the whole model; it makes distance meaningful
  beyond just needing more power.
- `f64` throughout the sim. Precompute `sin`/`cos` from the quantized angle
  (§4, `Vernier`) rather than calling trig on a float that varies by platform.
- Projectile is a point with a collision radius. Sample the heightfield at the
  projectile's x each step; collide when `y < surface(x)`.
- Off-screen vertically is fine — shells arc above the viewport. Off-screen
  horizontally past 1.5 screen widths is a miss; end the shot.

**Shot log** for replay: `(screen_index, angle_ticks, power_ticks)` per shot.
That plus `run_seed` reconstructs a run exactly. Nothing else needs storing.

---

## 4. Modifiers

Drafted 3-at-a-time at the end of each band. Take one, the other two are gone.
Offers are drawn from the pool by `screen_seed`, filtered to exclude anything
already held at max stacks.

Tags mark the build each supports: **DRIFT**, **DEMO**, **PRECISION**,
**ECONOMY**, **RISK**.

### Shell modifiers

| Name | Effect | Tag | Stacks |
|---|---|---|---|
| **Ballast** | Shell mass ×1.6. Wind acceleration ×0.5, but power range −20%. | DRIFT | 2 |
| **Sabot** | Muzzle velocity +25%, crater radius ×0.5. | PRECISION | 2 |
| **Airburst** | Set a fuse in units of distance; shell detonates there. Blast radius ×1.8, no terrain contact needed. | PRECISION | 1 |
| **Skipstone** | Shells ricochet once off terrain at ≤20° incidence, retaining 70% velocity. Structures are hit, not bounced off. | DEMO | 2 |
| **Auger** | Shell tunnels through one material layer before detonating. Bedrock stops it. | DEMO | 1 |
| **Cluster** | On detonation, splits into 3 submunitions at ±15°. Each does 40% damage. | DEMO | 2 |
| **Thermal Lance** | Steel counts as Rock for crater radius. Structures collapse rather than chip. | DEMO | 1 |
| **Featherweight** | Wind acceleration ×2.2, power range +40%. | RISK | 1 |

### Instrument modifiers

| Name | Effect | Tag | Stacks |
|---|---|---|---|
| **Vernier** | Angle quantization from 0.5° to 0.1°, then 0.05°. | PRECISION | 2 |
| **Anemometer** | Wind value shown to one decimal instead of rounded. | PRECISION | 1 |
| **Rangefinder** | Exact horizontal distance to nearest target displayed. | PRECISION | 1 |
| **Chalk Marks** | Previous shot's landing point stays marked on the terrain. | PRECISION | 2 |
| **Dead Reckoning** | Wind hidden until after the first shot of each screen. In exchange, +2 shells per screen cleared. | RISK | 1 |
| **Plumb Line** | A vertical guide at the apex of the last trajectory. | PRECISION | 1 |

### Economy modifiers

| Name | Effect | Tag | Stacks |
|---|---|---|---|
| **Salvage** | Misses that still crater within 30 units of a target refund 1. | ECONOMY | 2 |
| **Quartermaster** | +1 to every refund tier, including the 4+ tier. | ECONOMY | 2 |
| **Depot** | +8 shells immediately. No ongoing effect. | ECONOMY | 3 |
| **Standing Orders** | Clearing a screen in one shot grants +1 additional shell per consecutive one-shot streak, resetting on any miss. | ECONOMY | 1 |
| **Scavenger Rites** | Every 5th screen is a supply screen: no targets, +6 shells, counts toward screen total. | ECONOMY | 1 |
| **Forced March** | Refunds ×2. Reserve cap drops to 15. | RISK | 1 |

### Structural modifiers

| Name | Effect | Tag | Stacks |
|---|---|---|---|
| **Enfilade** | Emplacement can be repositioned ±25 units before the first shot of a screen. | PRECISION | 2 |
| **Sapper's Eye** | Material layers are visible as color banding in the terrain. | DEMO | 1 |
| **Long Lens** | Camera zooms out one step; the whole screen including target is always framed. | PRECISION | 1 |
| **Storm Front** | Wind resamples mid-flight once per shot. Reserve +12 immediately. | RISK | 1 |

### Intended build shapes

- **Drift** — Ballast + Anemometer + Vernier. Neutralize wind, win on precision.
  Weak against `Spires`, which punish the flatter arcs Ballast encourages.
- **Demo** — Auger + Cluster + Thermal Lance. Stop aiming at targets, reshape the
  terrain until the target is exposed. Shell-hungry; needs an economy pick.
- **Precision** — Sabot + Rangefinder + Chalk Marks. One-shot everything, live on
  refunds. Fragile: one bad wind read cascades.
- **Risk** — Featherweight + Dead Reckoning + Forced March. Enormous refund
  throughput on a 15-shell cap. Ends abruptly.

The RISK tag is deliberately the most fun and the shortest-lived. That is the
correct trade for a run-based game.

---

## 5. Entities and art direction

Low resolution done well is a set of commitments held without exception. Held
inconsistently it just reads as a small budget. The rules below are the ones that
actually separate the two.

### 5.1 Resolution

**Internal buffer: 180px tall, width 320–400 depending on device aspect.**

Fix the height, let the width float. Vertical scaling stays a clean integer on
every device, and since the world is endless horizontally, extra width on a
tall phone is free real estate rather than a letterbox. Generate terrain to fill
whatever width the device reports.

In Godot: stretch mode `viewport`, aspect `expand`, `scaling_3d_mode` irrelevant,
texture filter `nearest` project-wide. Set the default texture filter in Project
Settings rather than per-material — one missed material is the whole illusion.

**Non-negotiables:**

- Camera position quantized to whole pixels every frame. A camera at x=41.3 makes
  the entire terrain shimmer as it pans. This is the single most common way
  low-res games look cheap.
- No rotation on any sprite except the barrel (§5.3), which uses pre-drawn frames
  rather than runtime rotation.
- No scaling of any sprite, ever. Long Lens changes the internal buffer size, not
  the sprite scale.
- No anti-aliasing, no soft shadows, no gradients except through dithering.
- UI renders in the same buffer at the same resolution. A crisp modern font over
  chunky pixel art is the tell that gives away a fake retro look.

### 5.2 Palette

**Sixteen colors, fixed, no exceptions.** Allocate them deliberately:

| Count | Role |
|---|---|
| 4 | Terrain ramp — sand, rock, bedrock, and a shared shadow tone |
| 3 | Sky tones for menus and effects; the playfield uses one flat deep tone |
| 3 | Player chassis — body, highlight, dark edge |
| 3 | Structures — body, highlight, damage tone |
| 2 | UI — foreground text, background plate |
| 1 | Alert accent, used only for wind and the shell counter under 10 |

Local versus relaxes the alert accent's reservation, for that mode only: the
accent is the left player's colour there (§11.4).

Pull the ramp from desert reference photography rather than from another game's
palette — that keeps you clean on trade dress and produces better colors anyway.
Salt flats have a genuinely strange color range: bleached white-pink ground
against a deep sky, with very little mid-tone. Lean into that. It gives you high
contrast, which is exactly what a game about reading terrain silhouettes needs.

Dither only for menu sky gradients and material transition bands. The playfield
sky stays flat so targets, shells and the terrain silhouette remain legible.
Dithering everything is the other common low-res failure.

### 5.3 Player entity

**Tracked chassis, 26×11px.** Two body tones plus a one-pixel darker bottom edge.
No crew, no hatch, no insignia, no visible wheels — at this size wheels turn to
noise. Read it as a silhouette first: a low trapezoid with a slight rear taper.

**Barrel: 16px long, 3px thick, pre-rendered in 5° increments** from 0° to 90°.
Nineteen frames. Runtime rotation at this resolution produces stair-stepping that
changes shape frame to frame; pre-drawn frames stay stable. The barrel displays
the angle *quantized to 5°* while the numeric field shows the true value — the
sprite is coarse feedback, the number is the instrument. That gap is intentional
and it is why Vernier feels meaningful.

**Build silhouettes.** Swap the chassis or barrel sprite when a modifier lands:

| Modifier | Visual change |
|---|---|
| Ballast | Chassis widens to 30px, sits 1px lower |
| Sabot | Barrel extends to 22px, thins to 2px |
| Cluster | Barrel shortens to 9px, thickens to 5px |
| Auger | Barrel gains a 2px muzzle bulge |
| Forced March | Chassis loses its highlight tone, reads darker |

Five sprite variants covering the highest-impact modifiers is enough for MVP. The
player should be able to identify their build from the silhouette alone.

### 5.4 Structures

Vertical, 20–48px tall, 5–11px wide at the base. Silhouette variety matters far
more than detail:

| Type | Silhouette | Footprint |
|---|---|---|
| Derrick | Narrow lattice, tapering | 7px |
| Radio mast | Thin vertical, guy wires as single-pixel diagonals | 5px |
| Water tower | Bulb on legs — top-heavy, topples dramatically | 9px |
| Silo | Solid cylinder, domed cap | 9px |
| Pylon | Wide-based A-frame with two crossarms | 11px |

Three damage states, hand-drawn, not procedural:

- **Intact** — full sprite
- **Leaning** — redrawn at roughly 15° tilt toward the undermined side. A separate
  sprite, not a rotation.
- **Collapsed** — a low rubble sprite plus the debris mound in the scar buffer

The water tower is the one to build first. Top-heavy structures make undermining
legible, and the moment a player realizes they can crater the legs instead of
hitting the tank is the moment the DEMO build teaches itself.

### 5.5 Terrain rendering

Draw the heightfield as **vertical spans of solid color, one pixel column each**.
For each column: sky above the surface height, then the material color for each
depth band below it, then bedrock to the bottom. No sprites, no tilemap, no mesh.
It is a per-column loop into an image buffer and it is fast enough at 320 wide to
redraw every frame.

Material bands render as horizontal color changes within the column, so Sapper's
Eye is free — it just stops flattening the bands to a single tone.

Crater rims get one pixel of the shadow tone on the inner edge. That single line
is what makes craters read as depth rather than as bites taken out of the ground.

### 5.6 Type

Use a bitmap pixel font at native resolution — 5×7 for the shell counter and wind
value, 3×5 for the screen index. (Local versus scales the 3×5 face up for a few
callouts; see §11.4.) Never scale a TTF down into the buffer. Draw the
modifier draft cards with the same font at the same size; resist the urge to make
draft UI larger and smoother than the game.

---

## 6. Architecture

```
salt-line-sim/          # Rust, no_std-friendly, zero Godot dependency
  ├─ rng.rs             # SplitMix64 + PCG32
  ├─ terrain.rs         # generation, heightfield, scar buffer, materials
  ├─ ballistics.rs      # fixed-step integration, collision
  ├─ solver.rs          # solvability validation
  ├─ modifiers.rs       # pool, draft, effect application
  ├─ run.rs             # state machine, economy, shot log
  └─ replay.rs          # seed + shot log → verified reconstruction

salt-line-godot/        # gdext binding
  └─ GDExtension nodes wrapping run.rs; rendering, input, audio, UI
```

The sim crate must build and run headless. Its test suite is a batch harness:
generate 5,000 screens across seeds, assert every one is solvable, and histogram
the surviving-candidate counts by band. That harness is how you tune §2.5
without playing the game a thousand times.

Determinism test: replay 200 recorded runs on every CI platform and assert the
final state hashes match.

---

## 7. Independence checklist

Confirm before any public build:

- [ ] No character combatants — targets are structures, not figures
- [ ] Single player at its core. Local versus (§11), an alternating
      hot-seat duel, is in scope for now as a secondary mode; the run stays
      the main game, and versus has to pass every other item on this list
      (no figures, no skyline, its own terrain and art).
- [ ] No primate, fruit, or skyline motif anywhere in art or naming
- [ ] Title has no lineage to either reference game
- [ ] Zero code, data, or asset lineage from any prior artillery title
- [ ] Palette and UI developed from reference photography, not from another game
- [ ] Employer personal-project review submitted and cleared before publishing

---

## 8. Caches and seed rulesets

**Status: proposed.** Nothing here is built yet. The numbers are starting
points for the batch harness to tune (§6), not final values.

A **cache** is a supply crate buried in the terrain. Players dig it out with
ordinary shots: craters already persist (§2.6), so digging is a mechanic the
player has from the first screen. It is a trade in the game's only currency.
Shells spent digging now buy shells or a modifier later.

Caches are off by default. They are switched on per run by a **ruleset** that
travels with the seed, so a shared seed still means one exact run.

### 8.1 Rulesets

A run is identified by `(run_seed, rules)`. `rules` is a `u32` bitset, where
bit 0 is `CACHE` and the other bits are reserved for future variants.

**Text form:** `<seed>` or `<seed>+<RULE>[+<RULE>…]`, for example
`20260907+CACHE`. `parse_seed` hashes any text into a seed (§2.2 sharing), so
the rules must be split off first:

1. Split the text on `+`.
2. If every part after the first is a known rule token (case-insensitive),
   those parts are the ruleset, and the first part is parsed as a seed exactly
   as today.
3. Otherwise the whole text is hashed as today. A seed someone already shared
   keeps its meaning unless it happens to end in a rule token.

The formatted seed is `<seed>` with `+<RULE>` for each bit set, in bit order,
so every run has exactly one canonical text.

**Rules are part of the run's identity everywhere a seed is:**

| Place | Change |
|---|---|
| `RunState` | holds `rules`; generation and economy read it |
| `Replay` | `(run_seed, width, rules, log)`; reconstruction starts from the same rules |
| `state_hash` | includes `rules` |
| Save board | a `run` line gains a sixth field, `rules`. Bump `VERSION`; a five-field line reads as `rules = 0`, so old boards load unchanged |
| "Your best here" | keyed by `(seed, rules)` |
| Daily run | always `rules = 0` at first (see §8.6) |

### 8.2 Placement

Each screen rolls for a cache independently, from its own stream:
`cache_seed = splitmix64(screen_seed ^ CACHE_SALT)`. Because the stream is
separate, **terrain, targets and wind are identical with or without `CACHE`**.
The same seed plays the same screen and only gains buried crates. Existing
screen digests do not change, and a test pins that.

- **Chance:** 30% per screen, about 1.2 per band.
- **Column:** in the approach, `x ∈ [muzzle + 30, nearest target − 45]`. The 45
  keeps dig craters out of Salvage's 30-unit refund zone plus a crater radius,
  so digging never doubles as a Salvage refund. If the gap is narrower than 10,
  that screen has no cache.
- **Footprint:** 3 columns. A cache never sits under a structure.
- **Depth:** below the *original* surface, by tier:

| Tier | Share | Depth | Layer | Centred hits to reach* | Contents |
|---|---|---|---|---|---|
| Shallow | 50% | 10–14 | Rock | 2 | +3 to +5 shells |
| Deep | 35% | 18–26 | Rock | 3–4 | +5 to +8 shells |
| Bedrock | 15% | 32–38 | Bedrock | 5–6 | A modifier offer: pick 1 of 2 from the draft pool, or +10 shells if the pool has nothing to offer |

\* From the crater rule (§2.6) with `BASE_CRATER_RADIUS = 7`, a centred hit digs
9.8 in sand, 7.0 in rock and 3.85 in bedrock. Expect about 1.5× the centred
count in practice, since off-centre hits dig less.

Contents are fixed by `cache_seed` but hidden until the cache is opened, so
each cache is a gamble for the player and exactly the same for everyone who
shares the seed.

Supply screens (Scavenger Rites) have no targets, so they place a cache
anywhere between the muzzle and the far edge, at the same odds.

### 8.3 Claiming

A cache is claimed on the shot whose crater lowers any of its columns to its
depth, that is when `scar[i] ≥ depth` there. The check runs in the sim after
craters are applied, alongside collapse. Claiming is derived entirely from
the shots, so **caches add nothing to the shot log**, and replays reconstruct
them for free.

Shells are granted immediately and respect the reserve cap (Forced March). A
modifier offer opens a small draft *after* the screen resolves, never
mid-screen, so the draft stays the only place the loadout changes.

### 8.4 The real cost, and who caches are for

Digging shots are shots. They count toward the screen's shot total and so
toward its refund tier (§1), and a dig resets a Standing Orders streak. For a
player who would have cleared in one shot, a shallow cache really costs about
3 dig shells plus the drop from a +3 refund to 0. That player should usually
leave it. For a player already on their third shot, the lost refund is only 1,
and a miss that digs is less wasted.

That is the intended shape, and it gives each build its own economy:

- **Precision** lives on refunds and mostly ignores caches.
- **Demo** is described as shell-hungry (§4), and caches feed it. Auger tunnels
  a layer before detonating and Cluster digs wide, so Demo should reach deep
  caches for fewer shells. The harness measures how many fewer (§8.7). Caches
  are Demo's economy pick.
- **Sapper's Eye** also shows a cache's outline in the material banding. That
  turns a guess into a costed decision.

### 8.5 Presentation

- **Tell:** a single `UI_FG` pixel on the surface above the cache, glinting for
  one frame every two seconds. It uses no new colour: the alert accent stays
  reserved (§5.2). Sapper's Eye adds the crate's 3-column outline in its band.
- **Open:** the crate appears in the crater, drawn in the structure colours
  (§5.2), for a beat. Then its contents fly to the shell plate as `+N`, or the
  offer opens after the screen. No words.
- **Seed entry:** `+CACHE` can be typed, and touch gets a toggle on the keypad.
  The title screen and the run-over screen show the canonical seed text,
  rules included.

### 8.6 Open questions

- **Daily runs.** Everyone shares the daily, so cache positions become community
  knowledge. That's fun lore for custom seeds, but it flattens a leaderboard.
  Start with dailies at `rules = 0`. Revisit with a weekly `+CACHE` daily if the
  lore turns out to be a feature.
- **Consumable power-ups** as a fourth tier (one-shot Auger, one-shot Airburst).
  They would need a HUD slot and a rule for when they fire. Defer until the
  shells-and-offers version has been tuned.
- **More rules.** The bitset leaves room for `+CALM` (no wind), `+HARDPAN`
  (thin sand), `+IRONMAN` (no refunds). Each must keep terrain generation
  identical, or declare that it doesn't.

### 8.7 Tuning targets

Batch harness bots over the same seeds, compared by median run length (screens
cleared):

| Bot | Target vs never-dig |
|---|---|
| Never dig | baseline |
| Dig only shallow caches, only after a miss | +0% to +15% |
| Demo loadout (Auger + Cluster), dig everything reachable | +15% to +30% |
| Always dig every cache | **worse** than baseline, so digging is never automatic |

The levers, in order: chance per screen, tier shares, payouts, depths.

---

## 9. Run history and replay versioning

**Status: proposed.** Nothing here is built yet.

Every finished run is kept on the device as its seed, rules and shot log.
From that the game can offer **play this seed again**, **watch that run**,
and **share a run**. A run is a few kilobytes of text, so there is no
database: the storage model is the same as the save file's, plain text the
player owns (README, "Nothing for sale").

### 9.1 Storage

- **File:** `user://runs.txt`, next to `salt_line.txt`. One line per finished
  run, appended when the run ends. Runs abandoned mid-way are not recorded; see
  §9.6 for resuming.
- **Durability:** append and flush. A torn last line, from a crash mid-write, is
  skipped by the parser, the same rule the save file uses. Compaction rewrites
  the file through a temp file and a rename, as `save::store` already does.
- **Retention:** keep the most recent 500 runs, plus every run on the board.
  Compact once the file passes the cap by 10%.
- **Why not SQLite:** it's a C dependency on every target (Android, iOS,
  Windows arm64 cross-builds), it isn't hand-editable, and nothing here needs a
  query engine. The largest realistic history is a few megabytes.

### 9.2 Line format

```
run <epoch> <seed> <rules> <width> <day> <screens> <shells> <shots> <daily> <log>
```

| Field | Meaning |
|---|---|
| `epoch` | `SIM_EPOCH` of the build that recorded the run (§9.3) |
| `seed`, `rules` | the run's identity (§8.1) |
| `width` | buffer width the run was generated at. A replay must use it, whatever the current device |
| `day` | days since the Unix epoch, for sorting and display |
| `screens`, `shells`, `shots`, `daily` | the result, as the board records it |
| `log` | the shot log |

The **log** is space-separated tokens:

- `@N` sets the screen index.
- `A:P` is a shot at angle ticks `A` and power ticks `P`.
- `DN` is a pick of draft offer `N`.

For example: `@0 900:600 860:640 @1 700:520 @2 … @4 D1 …`. This keeps the
line readable, keeps each entry's screen for replay validation, and costs
about 8 bytes a shot.

### 9.3 Replay versioning: `SIM_EPOCH`

A shot log only reproduces its run on a sim that resolves shots
identically. Any change that alters generation or ballistics, which the
pinned screen digests and the determinism gate already detect (§6), makes
old logs replay to a different run. An old log silently diverging would be
worse than no replay at all.

- `salt-line-sim` exports `SIM_EPOCH: u32`, bumped on any change that alters a
  replay's outcome.
- **Enforced, not remembered.** A test combines the pinned screen digests and a
  small golden-run corpus (§9.5) into one hash, and asserts it equals
  `EPOCH_DIGEST`, which is recorded next to `SIM_EPOCH`. If the sim's outcomes
  change, the test fails with "bump SIM_EPOCH and update EPOCH_DIGEST". The
  bump goes in NOTES.md with the reason.
- **On load:**
  - Epoch matches: the run can be watched and replays exactly.
  - Epoch differs: the run keeps its seed, rules and result, so *play again*
    still works, but *watch* is unavailable and the entry is marked as from an
    earlier version. It is never replayed and never shown diverging.

### 9.4 Presentation

This follows the no-words direction (§5.6) and the touch menus.

- **PAST RUNS** from the title screen: a list of seed, screens and day, newest
  first. Rows from an older epoch are dimmed.
- **Row actions:**
  - PLAY AGAIN starts a new run on the same seed, rules and width.
  - WATCH plays the run back using the in-game shot animation, with a speed
    toggle. It's only offered when the epoch matches.
  - COPY copies the run line.
- **Sharing a run:** pasting a copied run line into seed entry imports it. The
  parser validates it. If the epoch differs, it imports as a seed only.

### 9.5 Testing

- The runs parser gets the save parser's treatment: a property round trip,
  a format snapshot, and a fuzz target (`fuzz/`).
- **Golden runs:** `crates/salt-line-sim/tests/golden_runs.txt` holds a few
  recorded runs across bands and loadouts, each with its final `state_hash`.
  CI asserts they still reproduce. It's the regression corpus for §9.3, and it
  is regenerated in the same commit that bumps `SIM_EPOCH`.

### 9.6 Open questions

- **Resume.** Mobile operating systems kill backgrounded apps. An in-progress
  run could be written the same way, as the log so far, and resumed by
  replaying it on launch. The sim reconstructs a 60-shot run in about a quarter
  of a millisecond, so this is cheap. It needs a rule for a shot that was in
  flight when the app died. The likely answer: the shot is committed once it's
  in the log, and resuming replays its outcome.
  **Built** (`crates/salt-line-godot/src/resume.rs`): `user://run.txt` holds
  one line, `resume <epoch> <seed> <width> <daily> <log>`, rewritten
  atomically whenever the log grows and deleted when the run ends (before its
  score is written, and only if that is confirmed). A finished run whose score
  could not be written keeps its line at the top of the file, ahead of any
  later run, until a score or a launch gets it on the board. A build older
  than that reads such a file as damaged and discards it, the live run and
  the waiting scores with it: a downgrade after a failed score costs them. The title
  screen offers CONTINUE when the epoch matches and the log reconstructs; a
  shot in flight at the kill resumes resolved, and a log whose last shot ended
  the run is scored instead. Another epoch, or a log that does not replay, is
  discarded with a note. Quitting to title keeps the run; starting a new one
  replaces it, after a second press. The resumed run keeps its recorded
  width, letterboxed if the device's differs.
- **Replays across epochs.** Old sims could be kept around to replay old runs
  (a versioned sim, or a pinned build). That's likely not worth it: the seed
  survives, and the seed is what players share.
- **Export.** A share format for whole runs (`saltline-run:` plus the line) is
  simple, but it waits until there is somewhere to share them.

---

## 10. Sandstorms

**Status: proposed.** Nothing here is built yet. The numbers are starting
points for the batch harness to tune (§6).

A sandstorm is wind you can see. It fits the salt-flat biome and the core skill
of reading wind (§2.5), and it works against the second skill, reading the ground
you have already shot at (§2.6). It comes in two parts that can ship separately:
**haze**, which is presentation only, and **drift**, which is simulation.
Wind shear (§10.7) is deliberately deferred.

### 10.1 When a storm happens

A storm is a per-screen condition, rolled from its own stream in the same way as
caches (§8.2): `storm_seed = splitmix64(screen_seed ^ STORM_SALT)`. Terrain,
targets and wind are identical with or without a storm, so a storm adds to a
screen and never changes what was generated.

- **Chance:** 0% in bands 0–1, then 15%, rising by 5% per band to a cap of 35%.
- **Intensity:** a value from 0.5 to 1.0 from the same stream. It is **not** taken
  from the wind's magnitude, because haze density would then give away how
  strong the wind is (§10.3).
- **Switch:** it starts behind the ruleset bit `SANDSTORM` (§8.1, bit 1), which
  is opt-in like caches. Once tuned, it becomes part of the default game, and
  the bit then turns it off (`+CALM`).

### 10.2 Drift

After each shot has resolved, with craters, collapse and claims all done, a
storm lays sand over the screen:

```
deposit[i] = intensity · (base_rate · n(i, shot) + fill_rate · max(0, local_mean[i] − surface[i]))
scar[i]    = max(0, scar[i] − deposit[i])
```

- `n(i, shot)` is value noise from `storm_seed` and the screen's shot count,
  biased to fall downwind of rises (the lee side), in the direction of the
  wind's sign.
- The `fill_rate` term makes sand collect in low places first, so craters fill
  before flat ground.
- **Drift only refills what was dug.** Because it reduces the scar and never
  goes below zero, the surface never rises above the generated one. The
  solvability proof (§2.4), made on the generated surface, therefore still holds
  on every later shot. The only exception is debris mounds (§2.7), which already
  sit above the surface.
- Deposited sand is Sand by depth (§2.1), so a refilled bedrock pit digs like
  sand again. That is the point.
- It runs between shots, never in flight, so ballistics and the solver need no
  change. It is drawn as a short settle over the next frames.

**Starting rates:** `base_rate` 0.25 and `fill_rate` 0.35 units per shot, at
full intensity. A centred Rock hit digs 7.0 units (§8.2), so drift slows
excavation without stopping it.

**Effects on other systems:**

- **Undermining (§2.7):** sand refills footprint columns, so Demo builds have to
  dig faster than the storm fills. Collapse is permanent, so a structure that
  has already fallen stays down.
- **Caches (§8.3):** partly dug caches get buried again, which punishes slow
  digging. A claimed cache stays claimed.
- **Replay and epoch:** drift is simulation. It is part of the screen state and
  `state_hash`, it needs a `SIM_EPOCH` bump (§9.3), and golden runs must cover a
  storm screen.

### 10.3 Haze

Haze is presentation only, so it needs no epoch or replay change.

- Blowing sand is drawn as a dithered overlay. It gets denser with distance from
  the emplacement, scaled by intensity, and at full intensity it hides terrain
  detail and craters beyond about 60% of the way to the target.
- **Target silhouettes stay readable.** Structures are drawn over the haze, the
  terrain under it. The shot trace, chalk marks, the plumb line and the
  Rangefinder readout are drawn above it: the precision instruments (§4) are
  exactly the counters to haze.
- **It must not reveal the wind.**
  - Particles move in the direction of the wind's sign, which the windsock
    already shows, at a fixed speed that says nothing about strength.
  - Under Dead Reckoning, before the first shot, particles swirl with no
    direction, because the sock is hidden then too.
- **The dither exception.** §5.2 limits dithering to the sky and material bands.
  Haze is a deliberate third exception, and uses only existing sand and sky
  tones. No new colours.
- **Battery.** The overlay updates at about 10 Hz rather than every frame, so
  the redraw-on-change loop (`frame::Redraw`) stays mostly idle on a storm
  screen.

### 10.4 Presentation

- **Screen tell:** a storm screen opens with the haze rolling in over the first
  second, and a small `≋` storm glyph beside the windsock. No words, in keeping
  with the HUD direction.
- **The settle:** after each shot, drift is visible as sand sliding into craters
  over roughly 0.4 s, within the existing clear-hold timing.

### 10.5 Salt storms

Salt flats are white. From band 10, a storm can be a **salt storm**: the same
system drawn in the palette's pale tones (`SAND`, `UI_FG`), with denser haze
and somewhat faster drift. It gives the look of snow without leaving the biome.
It is the same code with a different colour table and rates, chosen by the
storm stream.

### 10.6 Tuning targets

Batch harness over the same seeds, comparing storm screens with the same
screens calm:

| Measure | Target |
|---|---|
| Mean shots per screen | +0.2 to +0.4 in storms, no more |
| Surviving solver candidates | unchanged (drift can't raise terrain) |
| Demo build (Auger + Cluster) clear rate by undermining | at least 80% of calm |
| Runs where a storm screen ends the run | within 10% of the average screen |

Levers, in order: chance, intensity range, `fill_rate`, `base_rate`, and the haze
distance.

### 10.7 Deferred: wind shear

Stronger wind at altitude would add a real lob-versus-flat decision. It also
attacks readability, which §2.5 protects: it would need a second, readable wind
display, perhaps a high windsock, and a new closed form. The solver's
re-simulation would keep screens provably solvable, but candidate generation
would get less efficient. Revisit it after haze and drift have been played.

### 10.8 Open questions

- **Opt-in or default:** should storms stay a ruleset, or become the default once
  tuned (with `+CALM` to turn them off)? Recommended: default, once the
  targets in §10.6 hold.
- **Storms and the daily run:** same question as caches (§8.6). Probably yes,
  since storms are seeded and fair to everyone, unlike cache knowledge.
- **A storm-themed modifier:** for example a RISK card that doubles storm chance
  for shells, or an instrument that sees through haze. Only once the base
  system is in.

---

## 11. Local versus (prototype)

**Status: prototype.** Built for hot-seat play on one phone. Not part of the
roguelike: nothing here touches a run, the economy, the board or the shot log.

**Scope (owner, October 2026):** local versus is in scope for now as a
secondary mode, and §7's single-player item says so. The run stays the main
game. Versus is held to the rest of §7 like everything else: structures and
terrain only, no figures or skyline, its own art.

### 11.1 Rules

- Two guns on one duel screen, the left at `EMPLACEMENT_X`, the right at its
  mirror. Players alternate shots, and a pass-the-phone plate covers the field
  between turns so neither sees the other aim.
- **Hit:** any detonation whose blast reaches the other gun's box. This is the
  same overlap a blast needs to damage a structure (§2.7): any overlap at all.
  A direct hit on the gun counts. A shot that lands on its own gun scores
  nothing for anyone.
- First to **3 hits** takes a round; **best of 3** rounds takes the match. The
  round's loser does not matter for who opens: the sides take turns opening,
  left first.
- **Equal shots** (`versus::EQUAL_SHOTS`): the side that did not open gets an
  answer only when a hit would tie. If the opener makes its third hit one
  ahead, the other side gets one last shot; if the opener is two or more
  ahead (3-0, 3-1), no answer could tie and the round ends on the opener's
  shot. A tie past three plays on in pairs until a pair ends with one side
  ahead.
- **The guns roll after a hit** (`versus::GUN_SHIFT`, 16 units each),
  unless ROLLING GUNS is turned off on the pre-match screen (on by default,
  kept for a rematch). At the end of any pair of shots that had a hit in it,
  both guns move one step together, back toward their edges or forward
  toward each other, with even odds drawn from the match seed. They stand on
  a ladder of spots a step apart, one step behind `EMPLACEMENT_X` to two in
  front (`GUN_ROLL_BACK`, `GUN_ROLL_FORWARD`); one step back is as far as the
  chassis stays on screen. A gun at the end of the ladder rolls the other
  way, and a spot is only taken if the screen still passes the duel gate
  (§11.3) from there; otherwise the guns stay put. The range changes by 32
  units, so the shot that just hit is no better than a fresh aim. Both move,
  so the duel stays mirrored. Each gun lands on fresh ground (the craters
  under its chassis are filled) so both are seated alike, and each side's
  landing mark and last arc move with its gun. They roll at the end of the
  pair, not straight after the hit, so the side answering a hit aims at the
  guns it was scored on. A new round puts them back. NOTES §7 has the
  harness measurements behind these numbers.
- **Wind** is drawn every turn, uniform in `±2.5`, from the match seed, the
  round and the turn number (`versus::turn_wind`). The HUD shows it to one
  decimal, since every duel wind is small.
- **Craters** persist for the round (§2.6), with one exception: when the
  guns roll, the craters under both guns' chassis at their new spots are
  filled back to the ground as generated, so neither gun starts its next shot
  in a hole the other dug. Craters anywhere else stay. Every round is new
  ground.
- **Unlimited shells**, the base loadout for both guns, no drafts, no refunds.
- **Aim help**, per player, off by default, set on the pre-match screen: that
  player's last landing mark and arc this round, and a Short arc (the drag
  preview's) of the current aim in calm air.

### 11.2 One sim, two directions

The sim only fires rightward (`Shot` angles run from flat-right to straight
up). The right player's shot is resolved in a **mirrored frame**: the
heightfield reversed column for column, the wind's sign flipped, the right gun
at `EMPLACEMENT_X`. Craters dug there are mapped back by reversing the arrays
again, which is exact. The renderer always draws the shared frame and mirrors
the right gun's sprites; only the app and the renderer know whose turn it is.

This keeps the integrator, the solver and every determinism guard (§9.3)
exactly as single-player runs them; no `SIM_EPOCH` bump. The sim's only change
is a public `run::crater_detonation`, the crater half of a detonation moved out
of `Screen::detonate` unchanged, so versus digs by the same rule.

### 11.3 The duel screen

Generated per round from the match seed (§2.2's split, by round):

1. Base ridge and domain warp (§2.3 steps 1–2), at band 4's amplitude.
2. A template laid out from one gun to the other, drawn from `Flat`, `Ridge`,
   `Chasm`, `Mesa`, `Spires` and `Basin` (`Overhang` is unsolvable without a
   modifier). No elevation delta: neither side starts higher.
3. **Near-symmetric:** every column is pulled 45% of the way toward its mirror
   image, so a tenth of any asymmetry is left. Fair, without being the same
   picture twice.
4. **Gate (§2.4):** each gun needs `MIN_SOLUTIONS` distinct shots that hit the
   other, in calm air and at both wind extremes. Candidates come from the
   solver's closed form aimed at the gun box and are kept only if the
   integrator says they hit. The ladder is the single-player one: eight drawn
   templates, eight `Flat`, then level ground, which is solvable by
   construction at every width.

A batch of 300 seeds passed on the first rung every time, with every gun
saturating the gate cap (8 shots) at every wind. That is easy on purpose for a
first prototype: amplitude and template weights are the levers if duels need
more terrain to fight over.

### 11.4 Reading it at a glance

The mode is played by a parent and a young child on one phone, so who is who
and how it is going should read without reading words.

- **A colour each:** left is the alert amber, right the pale sky blue
  (`SKY_LO`). Far apart in hue, and still apart for red-green colour
  blindness. Everything that names a side or comes from its shot uses its
  colour: the tag over each gun (big over the gun whose turn it is), the
  score pips, the handoff and result plates' borders and lettering, the HIT
  callout, the hit flash, the shell's head, the muzzle and impact flashes,
  and aim help's landing mark and preview.
- **This relaxes §5.2 for versus.** The palette is still sixteen colours, but
  §5.2 (and §8.5) reserve the alert accent for wind and the low-shell
  counter; in versus it is the left player's colour instead. So that the two
  colours mean only a side, versus draws its secondary text in `ROCK` instead
  of `SKY_LO`, its wind digits in plain text and its windsock in brick and
  sand.
- **Plates everywhere:** `SKY_LO` *is* the duel sky's horizon band, so a blue
  mark over it disappears. Everything in a side colour sits on a plate or a
  one-pixel dark backing, for both sides alike.
- **Pips, not digits:** a big dot per hit a round takes, filled as hits land,
  over a small dot per round the match takes. When a tie carries a round past
  three, both rows grow to the higher side's hit count, so every hit has a
  dot and a lead shows as one filled dot against an empty one (at 4–3 the
  rows are four wide). The score plate widens to match, up to five dots a
  row; past that each side's hits are a number in its colour, so the plate
  stays clear of the pause button and inside the handoff and result plates.
- **Overtime and roll cues:** the handoff plate adds a line under the name of
  the player taking the phone. LAST SHOT - HIT TO TIE (in their colour) when
  the opener has made the winning hit one ahead and they must answer; HIT TO
  WIN (in their colour) when a hit by them would win the round outright;
  TIED - KEEP GOING when a pair has ended level past three; and GUNS ROLLED
  CLOSER or GUNS ROLLED APART after the guns roll. The HIT callout on a hit that leaves the
  round open reads RIGHT GUN TO ANSWER (or LEFT) or TIED - KEEP GOING instead
  of naming a scorer, so the opener's third hit does not look like the win.
  The result plate and its fanfare come only with an actual round win. The
  rolled guns appear at their new spots when the field comes back; they are
  not animated there.
- **Big words:** HIT, MISS and OOPS, whose turn it is and who won are drawn
  in the 3x5 face at two to four times size, on a plate. This is a deliberate
  exception to §5.6 for this mode only: whole blocks on the buffer's grid, so
  it is still pixel type. The instruments stay at native size.
- **Not only colour:** a HIT is called out over the scorer's half of the
  screen, and the HIT, handoff and result plates carry an arrowhead on the
  side of the gun they are about. A shell that lands only on its own gun is
  called OOPS, not MISS: no point, no ding.
- **Sound:** a two-note ding just after a hit lands (80 ms, so it is not
  lost under the impact), and a short rising fanfare when a round or the
  match is won, procedural like every other effect.
- **Rematch** returns to the pre-match aim-help choice, still set as it was.

### 11.5 Open questions

- Should a round's loser open the next round, rather than alternating?
  (Equal shots has removed most of the edge either way.)
- Should the guns ease to their new spots when the field comes back after a
  roll? Today the handoff plate says which way they rolled and they are
  simply there.
- Is a round of about 12 turns right for a young player? Rolling guns
  lengthens rounds by about a fifth over equal shots alone.
- Should a gun take damage from its own shells?
- A visible keyed title row (the keyed title snapshot was kept unchanged).
- Resume after the app is killed: a match is a seed plus shots, so it could be
  logged like a run (§9.6), but nothing is saved today.
