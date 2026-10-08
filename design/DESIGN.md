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
  ├─ replay.rs          # seed + shot log → verified reconstruction
  └─ layout.rs          # a screen as data: forks, share codes, checks (§13)

salt-line-godot/        # gdext binding
  └─ GDExtension nodes wrapping run.rs; rendering, input, audio, UI
```

The sim crate must build and run headless. Its test suite is a batch harness:
generate 5,000 screens across seeds, assert every one is solvable, and histogram
the surviving-candidate counts by band. That harness is how you tune §2.5
without playing the game a thousand times.

Determinism test: replay 200 recorded runs on every CI platform and assert the
final state hashes match. The platforms are Linux and Windows (x86_64 and
arm64), macOS (arm64), Android (x86_64 emulator, bionic) and iOS (arm64
simulator). The shipped Android ABI, arm64-v8a, is checked on a phone with
`just android-digest`, because hosted runners cannot run an arm64 emulator.

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
  width, letterboxed if the device's differs. A local versus match is kept
  and resumed the same way, in a file of its own (§11.6).
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

**Status: prototype.** Built for hot-seat play on one phone, or one player
against a local AI (§11.7). Not part of the
roguelike: nothing here touches a run, the economy, the board or the shot log.
A match in play is kept in a file of its own, to resume after the app is
killed (§11.6).

**Scope (owner, October 2026):** local versus is in scope for now as a
secondary mode, and §7's single-player item says so. The run stays the main
game. Versus is held to the rest of §7 like everything else: structures and
terrain only, no figures or skyline, its own art.

### 11.1 Rules

- Two guns on one duel screen, the left at `EMPLACEMENT_X`, the right at its
  mirror. Players alternate shots, and a pass-the-phone plate covers the field
  between turns so neither sees the other aim. Against the AI (§11.7) the
  plate stays, as the turn's marker, but the player's readies itself, and
  the AI's aim is shown swinging to its shot: there is nothing to hide from
  a computer that already knows the answer.
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
- **Aim help**, per player, set on the pre-match screen: that player's last
  landing mark and arc this round, and a Short arc (the drag preview's) of
  the current aim in calm air. Off by default between two players. The
  player's own (the left gun's, YOUR AIM HELP against the AI) is
  remembered for the session per opponent: ON against the AI and OFF
  against a human until set. The row sets the one for the opponent chosen,
  and switching RIGHT OPPONENT shows the other's, as last set. Resuming a
  match brings back the aim help it was played with, and that becomes the
  player's help against that match's opponent, with nothing held hidden.
  The right player's is theirs alone and never follows the opponent. The
  AI has none: its row picks its level instead, and its preview would draw
  the shot it solved. The attract demo shows both sides' help, as it
  always has, and leaves the player's, for both opponents, as it found it.

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

A custom duel (§13.9) skips all of this: every round is dealt the same
layout's ground, which passed the same gate when it was loaded.

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
  The result plate and its fanfare come only with an actual round win.
  Rolled guns stay drawn where they stood while the handoff plate covers the
  field, then roll to their new spots over 0.45 s once the next player is
  ready (`app::versus::ROLL_EASE`). Only the drawn gun and its tag move: the
  ground, its filled footprints included, is already the new one, the next
  shot fires from the new spot whenever it is fired, and aim help appears
  once the guns are home.
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
- **Pause on the result plate.** The pause key works there as between
  turns, and on a touchscreen the plate has the II button top centre, above
  the plate at 180 rows. RESUME comes back to the same plate, with nothing
  dealt: NEXT ROUND still has to be pressed, and the tap that resumed is
  spent ("One touch, one screen", NOTES.md §7). Every result plate has it,
  a round's or the match's, between two players or against the AI, so
  pause is in the same place on every screen of a match.
- **Quitting asks twice.** TITLE on the pause plate quits the match, and
  so does TITLE on a round's result plate between two players; while the
  match is still to be won either needs a second press: the first turns
  the row into TAP AGAIN TO QUIT (with keys, PRESS AGAIN TO QUIT after the
  key's name), and only another press within 3 s quits. On the pause plate
  RESUME, the pause key and MAKE MAP OF THIS ROUND disarm it; on the result
  plate NEXT ROUND, MAKE MAP OF THIS ROUND and the pause key or II (the
  pause plate opens disarmed, its own TITLE asking twice) do; focus going
  and the 3 s disarm it on either. Arming changes the screen, so the tap
  that armed it is spent and the second has to be a new one: one tap, a
  held finger or a bounce cannot quit. Paused over a round's result plate,
  TITLE sits where NEXT ROUND's target was, under a tap meant for NEXT
  ROUND after an accidental II; now that tap only arms it. A won match's
  plate, paused or not, quits on the first press: nothing is left to lose.

### 11.5 Open questions

- Should a round's loser open the next round, rather than alternating?
  (Equal shots has removed most of the edge either way.)
- Is a round of about 12 turns right for a young player? Rolling guns
  lengthens rounds by about a fifth over equal shots alone.
- Should a gun take damage from its own shells?
- A visible keyed title row for a new match (the keyed title snapshot was
  kept unchanged; a keyed RESUME MATCH row shows only over a suspended
  match, §11.6).

### 11.6 Resume after the app is killed

**Built** (`crates/salt-line-godot/src/match_log.rs`). A match is a seed,
its pre-match choices and its shots, so the match in play is kept the way a
run is (§9.6): `user://match.txt`, one line, rewritten atomically after each
shot and each new round, moved aside while a launch rebuilds it, by the run
file's own rules.

```
match <format> <sim epoch> <match epoch> <seed> <width> <rolling> <help L> <help R> <n> <log>
match 1 6 1 1234567 360 1 0 1 4 900:600 870:640 / 850:610

match <format> <sim epoch> <match epoch> <seed> <width> <rolling> <help L> <help R> <level> <ground or -> <n> <log>
match 3 6 1 1234567 360 1 1 0 easy - 4 900:600 870:640 / 850:610
```

- **The log** is `A:P` per shot (the aim as dialled) and `/` when the
  players move on from a round's result. The line must end in its newline,
  so a line cut anywhere is damaged rather than a shorter or different
  match; `n` counts the entries, so one lost from the middle (a hand edit)
  is caught too.
- **Versioned three ways.** `format` is the line's layout; `sim epoch` is
  `SIM_EPOCH` (§9.3); `match epoch` (`match_log::MATCH_EPOCH`) covers the
  duel rules, which live outside the sim and so outside its digest: a golden
  match pinned by its final state hash fails until it is bumped. Any of the
  three differing, or a log that does not replay, discards the file at launch
  with a note on the title (SAVED MATCH FROM AN OLDER VERSION, ... COULD NOT
  BE RESTORED). A file that brings the rebuild down is set aside and costs
  one launch, as the run file's does.
- **Resuming** rebuilds the match by taking its entries again through the
  same steps live play takes, so it is the match as played, byte for byte:
  the rules' state, each side's kept aim and last arc, and the roll the plate
  announces. It lands on the pass-the-phone plate for whoever is next, or on
  the result plate if the last shot won a round or the match. A shot in the
  air at the kill is committed, as in a run. Aim help and rolling guns come
  back as they were chosen, and the match keeps its width, letterboxed if
  the device's differs.
- **Cleared** when the players leave a won match's result plate (REMATCH or
  TITLE), and when they quit it (the pause plate, or TITLE on a round's
  result). A match killed as it was won comes back on the plate that says
  who won. A rematch is a new match.
- **The title.** On a touchscreen the LOCAL VERSUS row reads RESUME MATCH
  (amber, like CONTINUE) while a match is suspended: the same row, not an
  extra one, since the phone title's worst case (a saved run, a crash report,
  a launch note) already fills the screen. With keys, `2  RESUME MATCH`
  shows under SEED ENTRY only then. A new match over a suspended one is had
  by resuming it and quitting: RESUME MATCH, pause, QUIT, then LOCAL
  VERSUS. That is a deliberate four taps, the same as giving up a match in
  play; a NEW MATCH row would cost the phone title a row, and a
  second-press confirm on the versus row would make RESUME MATCH a trap for
  whoever taps it twice. (Open question: whether a long-suspended match
  should expire.)
- **A run and a match can both be suspended.** Each offer stays in its
  mode's place, CONTINUE on confirm at the top (the run is the main game) and
  RESUME MATCH on the versus row, so neither moves the other and both read at
  a glance. They are separate files and separate state: taking either leaves
  the other untouched.
- **The attract demo** never writes or clears the match file, and never
  touches the offer (§12.3).
- **A match on a layout** (the GROUND row, §13.9) is kept like any other,
  its line in format 2 carrying the layout's code (§13.4), and resumes on
  it. One started by `--salt-layout` is not kept.
- **A match against the AI** (§11.7) is format 3: format 2's fields with
  the AI's level (`easy`, `medium`, `hard`) after the aim help and `-` for
  the ground on generated screens. It resumes against the AI at that level,
  and its shots replay like anyone's, since the AI's are logged as fired. A
  match between two players is still written as format 1 or 2, so a build
  from before the AI resumes it as before; that build drops a format 3 line
  as a newer version, with the usual note.
- **Forking the round** (MAKE MAP OF THIS ROUND, §13.11) takes an
  unfinished match off the screen without ending it: its line stays and
  RESUME MATCH offers it, as after a kill.

### 11.7 Local AI opponent

**Built** (`crates/salt-line-godot/src/opponent.rs`, paced in
`app/versus.rs`). **Owner decisions (October 2026):** RIGHT OPPONENT: AI on
the pre-match screen puts the computer on the right gun; three levels,
EASY by default; it eases off while it leads.

**Who it is for.** A child who wants a duel when nobody is free to take the
phone. So it is pitched to lose more than it wins at EASY, it says YOU and
AI rather than LEFT GUN and RIGHT GUN, and nothing in an AI match asks
anyone to pass the phone or to read much.

- **It knows the wind and the range exactly.** Each turn it asks the solver
  for a shot that hits, flown by the integrator in this turn's wind
  (`versus::shots_at`), the way the demo's duel does (§12.2). What makes it
  beatable is that it misses on purpose.
- **Levels.** RIGHT AIM HELP becomes `AI LEVEL: EASY / MEDIUM / HARD`
  (Right, or tap the row), and LEFT AIM HELP reads YOUR AIM HELP; the right
  player's own aim-help choice is kept for a human match. The odds of firing the hitting shot on a turn are:

  | Level | Hit | Ahead |
  |---|---|---|
  | EASY | 2 in 10 | 1 in 10 |
  | MEDIUM | 4 in 10 | 2 in 10 |
  | HARD | 6 in 10 (the demo's 3 in 5) | 4 in 10 |

- **Easing off.** While the AI leads on hits in the round being played, it
  plays one level down, and EASY drops to 1 in 10 (`opponent::ahead`).
  Rounds won do not count. A lead it opens is slow to grow, and a child who
  falls behind is not shut out. It never eases when level or behind on hits,
  even a round up, so a last shot to tie, or a tied pair, is taken at full
  odds.
- **Deterministic.** Which turns hit is a hash of the match seed, the round,
  the turn and the level (`opponent::turn_hash`): no random stream and no
  frame timing, so a match plays the same way whatever the device.
- **A miss is a near miss**, as in the demo: the hitting aim moved in power,
  then angle, tried on a copy of the match until one leaves both guns
  unhit. If none will do, or the solver finds nothing, a few high short
  lobs (`opponent::FALLBACKS`) are tried on a copy too, in order of
  preference: the first that misses both guns; failing that, the first
  that hits only the player's gun (a point given away rather than its own
  gun shelled); failing every one, the plain lob. So the AI avoids shelling
  itself wherever any fallback allows. The demo does not take this path:
  it falls back to the plain lob, untried, as it always has
  (`opponent::Fallback::Lob`), so its duels are the same as before by
  construction.
- **Pacing.** The AI's plate holds 1.2 s (`AI_HANDOFF`), 2 s when it has a
  cue to read out (LAST SHOT, HIT TO WIN, TIED, GUNS ROLLED;
  `AI_HANDOFF_CUED`); its barrel swings to the shot over 0.7 s and fires at
  1 s. The player's plate stays, so the turns read as turns and the guns
  still roll once it lifts, but readies itself after 1.5 s (`AUTO_READY`),
  or 2 s with one of the same cues (`AUTO_READY_CUED`, the AI's cued hold),
  so a LAST SHOT is read out before the aim appears; READY skips the
  wait. Everything that moves by itself stops under the
  pause plate and while the window is out of focus, and in an AI match
  the handoff plates pause on focus loss as aiming does. The AI's own
  stage changes do not settle touches (§11.4's "one touch, one screen"),
  so a tap on II just as it moves on is taken; the player's auto-ready
  does, so no finger held over the plate fires a shot.
- **Words.** One helper names a side everywhere (`render::versus::say`):
  YOU and AI on the handoff plate, the aiming tag, the HIT callout (YOU
  SCORE, AI TO ANSWER) and the result plate (YOU WIN, AI WINS). The AI's
  plate reads AI IS AIMING, the player's YOUR TURN.
- **Between rounds** the result plate leaves TITLE off in an AI match: a
  child playing alone mashes, and Y (or R), which turns the AI on and off on
  the pre-match screen, was TITLE there. NEXT ROUND and MAKE MAP OF THIS
  ROUND stay; leaving mid-match is the pause plate's TITLE, pressed twice
  (§11.4), as between turns. Between two players the round's plate keeps
  its TITLE, which asks twice as well. On a touchscreen the plate's own II
  reaches it, so quitting never means starting the next round first. The
  pause key (Esc, a controller's B) works on the plate too, but the plate
  does not list it, so a controller player may not find it. A won match's
  plate keeps TITLE, with nothing left to lose.
- **Aim help on by default** for the player against the AI (§11.1): the
  child this is for gets the landing mark and the preview without finding
  the row first. Turned off, it stays off against the AI for the session,
  and the human matches' choice is kept apart. It is what the harness's
  kid bot below has always assumed.
- **Kept** like any match (§11.6), in format 3, and for a rematch.
- **How it plays against the harness's bots** (`just duel 400 kid --right
  ai-easy`, 400 matches each, the bot on the left opening):

  | | EASY | MEDIUM | HARD |
  |---|---|---|---|
  | kid wins | 99% | 74% | 21% |
  | steady wins | 99% | 84% | 33% |
  | AI's hit rate (v kid) | 19% | 35% | 55% |

  The bots are guesses (`examples/duel.rs`): read the columns against each
  other. EASY is meant to be beaten. HARD is the demo's odds and beats the
  steady adult bot about two matches in three, so it is a stiff opponent
  for a grown-up rather than an even one. The kid bot plays near-adult:
  with aim help (the default against the AI, §11.1) it hits about 42-47%
  of its shots against the steady bot's 50-55%, so how EASY and MEDIUM
  feel to a real child is not settled by these numbers. Their calibration
  is provisional until the owner's playtests with the child they are for.

---

## 12. Attract demo

**Status: built.** The game plays itself on the title's behalf, the way an
arcade cabinet does between players: a run, then a duel, then a run, until
someone touches it. It shows what the game is before anyone has to read a
menu, and it is the one screen a phone left on a table is likely to be on.

**Owner decisions (October 2026):** the idle demo is on by default, with a
Controls toggle to turn it off; the loop alternates a single-player run and a
versus duel.

### 12.1 Starting and stopping

- **From the title only.** After `IDLE_DEMO_AFTER` (30 s) on the title with
  no input at all (no key, no pad button, trigger or stick, no finger down)
  while the window has focus, or at once from the title's DEMO: `D` (pad
  d-pad down) and a keyed DEMO row, or the DEMO plate in the top-left corner
  of a touchscreen title, which mirrors the music toggle so the menu rows and
  the board stay where they were. Never from Controls, seed entry, a pause,
  versus setup or a result. Starting forgets a half-confirmed new run
  (`discard_armed`), as leaving the title does.
- **IDLE DEMO: ON/OFF** in Controls (`D` there too, or tap the row). A setting
  like LAST ARC: `idle_demo` in the save, written only once chosen, so every
  save from before the demo reads as on. No `VERSION` bump: older builds skip
  the line.
- **Any input ends it** and returns to the title: any key (read by the game or
  not), a pad button, an analog trigger, the stick, the back gesture, a tap.
  That input is spent: no action is read on the frame it arrives, and touches
  are settled (§11.4's "one touch, one screen"), so the tap that ends the
  demo cannot also press CONTINUE under it. Only the demo's own start settles
  touches; its duel plates do not, so a tap is never ignored mid-demo. A
  finger that goes down while the touches settle after DEMO was tapped is
  the same tap reported again, not a request to stop.
- **An idle-started demo also ends by itself** when the window loses focus
  or the app is paused (nobody is watching), and after `IDLE_DEMO_LIMIT`
  (10 min), after which the title waits for some input before it starts
  another. A demo chosen from the menu runs until it is stopped.
- **A banner, always.** `DEMO · TAP TO PLAY` on a touchscreen, `DEMO · PRESS
  ANY KEY` with keys or a controller, on a plate, bottom centre, in the
  native 3x5 face and plain UI text: clear of the instruments, the screen
  index or round, the draft cards and the duel's plates. It follows §5.2 and
  §5.6 in a run and in a duel alike; §11.4's relaxations (side colours, big
  type) stay inside versus proper. The demo draws no touch controls, no
  prompts and no onboarding. The duel's handoff plate keeps its side heading
  and score but drops TAKE THE PHONE and READY, and neither plate dims the
  board: nobody is passing a phone.

### 12.2 The loop

| Leg | What plays | Ends |
|---|---|---|
| Run (even legs) | the next of `DEMO_RUNS`: a seed and a screen to start on | `DEMO_RUN_SCREENS` (4) screens cleared, the run over, or `DEMO_RUN_TIME` (75 s) |
| Duel (odd legs) | a local versus match on the cycle's own seed, rolling guns on, aim help on for both | `DEMO_DUEL_ROUNDS` (1) round won, or `DEMO_DUEL_TIME` (40 s) at the next handoff |

Every leg opens with a **mode card** (below), and the caps count play only,
from the card's end, so they bind as they did before the card. Over the
first eight cycles a run leg, card and end hold included, takes 19–29 s and
a duel 22–44 s (17–27 s and 20–42 s before the card). The rotation carries
on from one demo to the next in a session, so a second look starts on the
leg after the one that was interrupted.

**Mode cards (owner request, October 2026).** So a viewer knows what they
are watching, each leg, the very first included, opens with a card naming
its mode, held `DEMO_CARD` (1.8 s) over the leg's first frame: the run's
opening screen, or the duel's opening ROUND 1 plate. Nothing plays while it
is up; the aim and the duel's plate hold still.

| Before | Title | Hook |
|---|---|---|
| a run | ROGUELIKE | TOPPLE TARGETS · DRAFT A BUILD |
| a duel | TWO PLAYERS | ONE PHONE · TAKE TURNS |

The hooks say what §1 and §11 make of each mode: a run clears screens by
toppling their targets and drafts a modifier every band; a duel passes one
phone between two players, a shot each. The card sits top centre on a plate,
level with the corner plates (row 3, below any cutout pad) and between them,
27 rows tall: it ends at row 29, or row 35 with a phone's pad, above the
duel's plate and the curated runs' tallest targets (row 40). The banner
still shows below.
It follows §5.2 and §5.6 like the banner: the native 3x5 face, the title in
UI text, the hook in the secondary tone (`SKY_LO` in a run, as the menus
have it; `ROCK` in a duel, where `SKY_LO` is the right player's colour), no
side colours and no big type, and like the demo's other plates it does not
dim the board. A run card does not name its seed: the line pushed the card
down onto two of the four curated targets, and DUNE or 1984 means nothing to
someone who has not seen seed entry. Input during a card ends the demo the
same way as at any other time.

The runs were picked from headless screenshots (`--salt-seed` with
`--salt-screen` and `--salt-shot`) for a striking first screen, and start
part-way in, as `--salt-screen` does, which §2.2's per-screen seeds make free:
`DUNE` at screen 8 (a water tower over a chasm, at night), `TOWER` at 13 (two
targets either side of a mesa), `1984` at 9 (a water tower on a mesa, at
night) and `MESA` at 17 (spires). A demo run is never in the run file and
never on the board.

Shots come from the solver, as `--salt-auto` aims. Misses are deliberate and
deterministic: a hash of the demo cycle (a run and a duel) and the shot's
place in it (screen and shot, or round and turn). A screen opens with a miss
two times in three and its second shot misses one time in four; a duel turn
misses two times in five. A miss is a *near* miss: the solver's aim moved in
power, then in angle, until a trial on a copy of the run or match leaves the
target standing (or the other gun unhit), so craters, chalk and landing marks
land close to the target and rolling guns get their turn. A high short lob is
kept for when no near miss is found, fired as it is, untried
(`opponent::Fallback::Lob`). The local AI (§11.7) aims the same way, but
falls back to a few short shots tried on a copy instead, so it avoids
shelling its own gun; the demo keeps the plain lob, so its duels are the
ones it has always played.

Pacing keeps it moving: each leg's mode card stays 1.8 s; each new aim
takes 0.8 s, the barrel and readouts easing to it over the first 0.55 s and
holding there before it fires; the draft cards stay 1.6 s before an offer is
taken, the duel's heading plate 0.8 s, a result plate 1.8 s, and a run's
last screen 1.2 s before the duel.

### 12.3 Nothing the player keeps changes

The demo drives the ordinary run and versus code, so it looks exactly like
the game, and must not touch the suspended run or CONTINUE, the run files,
the suspended versus match, RESUME MATCH or the match file (§11.6), the
board, learned lessons, settings (other than its own toggle, set in
Controls), crash reports or replays. Two rules (`app/demo.rs`):

1. **Set aside whole.** The player's `Play` (which holds the run kept for
   CONTINUE), the versus state and the buffer width are moved out on entry
   and moved back on exit; nothing reads or writes them in between. The
   CONTINUE offer itself (`run_file`) is never touched, but for forgetting a
   half-confirmed discard; nor is RESUME MATCH's (`saved_match`).
2. **No write is asked for.** Every place that would ask for a disk effect
   checks for the demo first: the run file, scoring, the match file (whose
   writes and clears also need a match marked kept, which the demo's duels
   never are), learned lessons, settings, the crash report's retirement.
   Settings pending when the demo
   starts are written first, before it counts as running. A backstop in the
   frame drops any disk effect a demo frame asks for, and debug builds and
   the tests fail if it ever finds one.

The tests play a run quit to the title through a whole demo cycle and back,
and check that every file is byte for byte as it was, that no disk effect was
asked for, and that CONTINUE (and a relaunch) picks the run up from the shot
it was left at; and the same for a suspended versus match and RESUME MATCH.
A crash during the demo still writes its report, with the
mode named `Demo/Playing` or `Demo/Versus` so it is not read as a player's
run.

### 12.4 Sound, screen and frame rate

- **Muted.** The demo asks for -80 dB on every player while it runs, the
  same `Volume` effect mute uses, without touching the saved volume or mute;
  the title music stops with the title and comes back with it.
- **Screen.** The project turns Godot's default keep-screen-on off
  (`display/window/energy_saving/keep_screen_on = false`), so the operating
  system's screen timeout applies unless the game holds the screen on. It
  holds it only while somebody is watching the phone, and not for ever if
  nobody touches it:

  | Holds the screen on | Lets it sleep |
  |---|---|
  | live play (`Playing`, the draft included), until 4 min without input | the title, Controls, seed entry |
  | a versus match not paused (aiming, flight, the pass-the-phone plate, the results), until 4 min without input | a paused run, the versus pause |
  | a demo chosen from the menu (DEMO, `--salt-demo`), for 15 min | the run's result (`GameOver`), the aim-help choice (`VersusSetup`) |
  | | an idle-started demo, the error screen, any screen while unfocused or the app is paused |
  | | live play or a menu demo past its quiet limit |

  The quiet limits (`AWAKE_QUIET_LIMIT`, 4 minutes; `DEMO_AWAKE_LIMIT`,
  15 minutes) count seconds without any input, a held key or stick or a
  resting finger included (`Game::input_quiet`), from zero on entering a
  screen that holds. Past one the screen sleeps on the OS timeout, so a
  phone put down mid-run or on a versus plate does not stay lit; the next
  input holds it again. A menu demo plays on past its limit, it just stops
  holding the screen. The dev autopilot (`--salt-auto`, the profiling
  build) and a pending `--salt-shot` capture are exempt, being untouched
  by design.

  The idle demo holds nothing, so a phone left on the title sleeps on its
  normal timeout, demo or not, and an unattended demo does not burn the
  screen all night. To showcase the game, start DEMO from the title. The
  wanted state is worked out in one place from the mode, the pause, the
  demo and the quiet count (`Game::screen_awake_wanted`, over
  `Game::live_play`, which desktop GameMode also asks) every time effects
  are taken, and an
  `Effect::ScreenAwake` goes out only when it changes, so the focus and
  app-pause notifications let go as surely as a frame does. The adapter
  calls `DisplayServer::screen_set_keep_on`: on Android the window's
  keep-screen-on flag, which counts only while the window is visible; on
  iOS the idle timer; on desktop it holds off the screensaver and display
  sleep. The error screen after a panic always sends a release, since a
  panic in the adapter may have skipped the last one.
- **30 fps.** The demo runs at `frame::IDLE_FPS`, the menus' cap: it is
  watched, not played, a shell's flight reads well enough at 30, and it may
  run for a long while on a battery. It never asks for desktop GameMode.

### 12.5 Open questions

- ~~Should the demo show a short card between legs naming the mode
  ("ROGUELIKE", "TWO PLAYERS")?~~ Yes (owner, October 2026): see "Mode
  cards" in §12.2.
- Sound on as an option, for a demo started from the menu?

---

## 13. Layouts, the editor and custom games

**Status: Phase 3 built (October 2026); challenges, courses and sharing
proposed.** Phase 1 is the data model, forking a generated screen into it,
the share code, validation, and a dev flag that plays a custom duel on a
code (`--salt-layout`). Phase 3 is the map editor in the game, the slot
file, the draft kept on disk, custom duels on the pre-match screen's
GROUND row, bundled layouts, forking the round on screen, the clipboard,
and resuming a match on a layout (§13.8, §13.9, §13.11). Phases 4 and 5
(§13.12) are still a plan.

**Owner decisions (October 2026):**

- Custom duels come first. Single-player challenges come later.
- The editor is in the game, built for a phone first (touch), and usable
  with a mouse.
- A layout sets the wind and nothing else about the rules: a fixed wind, a
  range, or calm. No loadout or modifier overrides yet.
- Every layout is 320 columns wide, letterboxed on wider buffers.
- **Fork from a seed** is the base of the editor. Any generated screen, a run
  seed and a screen index or a duel's match seed and round, can be turned
  into an editable layout. The generator is the first draft.

**Owner decisions for Phase 3 (October 2026), final:**

- Custom duels first. The editor is phone-first and touch-driven, usable
  with a mouse, and has a controller and keyboard path.
- **Fork this round** (a match's pause or result plate) crops the live
  screen to 320 columns keeping both guns (§13.11 has the rule). **Fork
  from a typed seed** makes a labelled 320-column draft (`FROM SEED X R2`).
  The fork's origin is recorded beside its code (§13.8), not in it.
- **A floating structure is a warning, not a block**, everywhere. Duels have
  no structures; the rule stays for runs and challenges, downgraded.
- **Rolling guns stay off on a layout**, and the pre-match screen's ROLLING
  GUNS row reads OFF over one.
- **A fixed duel wind turns round every pair of turns**: each player shoots
  once a pair in the same wind, and the next pair blows the other way, from
  the start of every round, so neither side has it behind them more often
  over a best of three. Ranges and calm are unchanged.
- **SCULPT stays as built** (the ground rises or falls by how far the
  finger is dragged from where it went down), and **MAKE MAP stays on the
  versus pre-match screen**.

### 13.1 Concepts

| Term | Meaning |
|---|---|
| **Layout** | One screen written down as data: the ground, the targets, the guns, the wind and a name. Built (Phase 1). |
| **Custom duel** | A local versus match (§11) whose every round is played on one layout. Built behind a dev flag (Phase 1), then on the pre-match screen's GROUND row (Phase 3). |
| **Challenge** | A one-gun layout played for shots, as a run screen is. Later. |
| **Course** | An ordered list of layouts played as a short run, for challenges. Later. |

A generated screen and a layout are opposites. A generated screen is a
function of its seed (§2.2): nothing about it is stored, because the
generator can always make it again, and a 20-character seed is the whole of
it. A layout stores everything and generates nothing. That costs about two
hundred characters instead of twenty, and buys two things a seed cannot
offer:

- **It can be edited.** A seed means what the generator makes of it, all or
  nothing; a layout is just numbers, and an editor can change any of them.
- **It outlives the generator.** A seed means a different screen after any
  change to generation (§9.3); a layout means the same ground for ever. Only
  how shots fly over it can change (§13.4).

### 13.2 The data model

`salt_line_sim::layout::Layout`, all integers:

| Field | Holds | Unit |
|---|---|---|
| `heights` | ground height per column; its length is the width | sixteenths of a world unit |
| `structures` | each target's kind and leftmost column | columns |
| `guns` | barrel pivot columns: one for a challenge, two for a duel | columns |
| `wind` | `Calm`, `Fixed(w)` or `Range { min, max }` | thousandths of a unit/s² |
| `name` | up to 12 characters from the seed keypad, `A`–`Z` and `0`–`9` | |

**Why integers, when the sim keeps everything in `f64` (§3)?** A layout is
not sim state. It is something a person edits, a share code carries and a
file keeps, and integers make all three exact:

- An edit is a whole number of steps, so undo, redo and mirroring give back
  exactly what was there.
- The share code can code differences between columns without rounding
  anything (§13.7).
- Every device builds the same doubles from the same layout. `h / 16` and
  `w / 1000` are single correctly rounded IEEE operations, and `h / 16` is
  exact, so nothing depends on the platform's maths library.

**Why sixteenths.** A height is used four ways: drawn in whole pixels, collided
with between columns (sub-pixel matters a little, §2.1), and read for seating
and support (the highest and the mean of a footprint, §2.7). Rounding to the
nearest sixteenth moves a height by at most 1/32 of a pixel. The harness
(`just layouts --seeds 50`, NOTES §9) measured each grid over a sample of
2,000 run forks and 150 duel forks: how long the codes get, and how often a
shot fired at a fork ends the same way as at the screen it was forked from.
(The full 100-seed run in §13.6 and §13.7 gives slightly different medians,
206 and 194 at sixteenths.)

| Grid | Median code: runs | duels | Shots ending the same: runs | duels |
|---|---|---|---|---|
| whole units | 113 | 103 | 98.25% | 86.4% |
| quarters | 150 | 143 | 99.76% | 97.0% |
| **sixteenths** | **202** | **198** | **99.93%** | **99.3%** |
| 1/64 | 305 | 298 | 99.97% | 99.8% |
| 1/256 | 426 | 423 | 99.98% | 99.97% |
| every bit | about 3,080 | 3,050 | 100% | 100% |

Each halving of the grid costs at least a bit a column (64 characters), and
more at the finer grids.
Sixteenths is where the curve flattens: the shots that still differ are
grazing ones, where a ground moved by 1/32 changes which 1/120 s step a shell
lands on. Wind is in thousandths for the same reason: a fork moves it by at
most 0.0005 units/s², which drifts a three-second shot by about two
thousandths of a pixel.

**Structures store a kind and a column, not a height.** A structure is seated
by the generator's own rule, on the highest column of its footprint
(`run::seat_at`, shared by both). So editing the ground under a structure can
never bury it or leave it hanging in mid-air: it moves with the ground. The
same holds for the guns.

**Names** use only what the seed keypad can type (§13.10).

### 13.3 From layout to screen

`Screen::from_layout(&layout, wind)` builds the same kind of `Screen` the
generator does, with one difference: the base buffer (§2.1) is the layout's
heights instead of the generator's output. Everything after that is
unchanged, because none of it can tell where a base came from:

- Scars, materials, craters and their clamps (§2.6), collapse and debris
  (§2.7) act on the scar buffer, as on any screen.
- Structures are seated by the generator's rule and start intact.
- The first gun is the emplacement (`muzzle_x`). In a duel, versus places the
  second gun as the mirror of the first (§11.2), which is why a duel layout's
  guns must be mirror images (§13.5).
- The solver, the integrator and versus's mirrored frame run on it as they
  are.

The screen's `seed` is a digest of the layout, so the renderer's ground
speckle is the same every time the layout is shown. Its `template` reads
`Flat` and its `rung` is `None`, since nothing was generated.

A match on a layout keeps the layout's width. `Game::start_layout_match`
sets the buffer to 320 the way a resumed run keeps its recorded width
(§9.6), and the display poll leaves it there while the match is on.

### 13.4 Determinism and versioning

**Nothing that existed changes.** Generation, runs, duels, replays, the
pinned screen digests, the golden runs and the CI determinism digest are
untouched, so there is no `SIM_EPOCH` bump. The only edit inside the sim's
generation path is a refactor: seating a structure at a column
(`run::seat_at`) is split out of seating it near an x, and the pinned
digests prove the split changed nothing. A match without a layout runs
exactly as before; versus gained an optional `Ground` that a generated match
never sets.

**The share code has its own version, `LAYOUT_FORMAT`**, the first byte of
every code. A decoder reads only its own format and names any other
(`CodeError::Format`); a later format is a new number and a new decoder
branch, never a reinterpretation. The format version and `SIM_EPOCH` are
independent, on purpose:

- A layout is not a seed. A change to generation, which bumps `SIM_EPOCH`,
  does not change what a layout means. That is the main reason layouts are
  data (§13.14).
- A change to ballistics can still change how a layout *plays*, and in
  principle whether it passes the solvability gate. So validation is not a
  certificate stamped on a layout when it is made. It runs every time a
  layout is loaded (it costs well under a millisecond, §13.5), and a layout
  that stops passing is shown as such, not silently dropped.
- A fork's code does move with the generator: the same seed forks to a
  different layout after an epoch bump. The snapshot test pins two codes, a
  hand-made layout (moves only with `LAYOUT_FORMAT`) and a fork (moves with
  either), so a change to either is seen.

**Resuming a match on a layout (built).** A suspended match is its seed,
its choices and its shots (§11.6); a match on a layout needs the layout as
well, so its line carries the code: `match_log` format 2,
`match 2 … <help R> <code> <n> <log>`. A match on generated screens is still
written as format 1, byte for byte, so every existing line and the pinned
one are unchanged; a build reads both, and one from before layouts reads a
format-2 line as a newer version and drops it with a note. A ground that
does not decode, is not a duel or does not fit the match's width is a
damaged line. No `MATCH_EPOCH` bump: the duel rules a format-1 line replays
by are unchanged (the golden match's digest holds), and the one rule that
changed, the fixed wind turning round, applies only to matches on a layout,
which no earlier line could hold. That rule was first a flip every round
and is now a flip every pair of turns (owner, October 2026), changed in
place, with no format or epoch bump, because no format-2 line was ever
written by a released build: Phase 3 had not merged.

### 13.5 Validation

`layout::check_placement` applies cheap rules, flying no shots, and returns
every problem it finds, in a fixed order, each naming the structure or gun
at fault, so the editor can mark it:

| Rule | Problem |
|---|---|
| exactly 320 columns | `Width` |
| every column from `WORLD_FLOOR` to `WORLD_CEIL` (12–108) | `HeightOutOfWorld` |
| one gun or two | `GunCount` |
| a gun's whole chassis on screen (pivot 14 or more from either edge) | `GunOffScreen` |
| **gun seated:** at least half its 26-column chassis within 12 units of its seat | `GunNotSeated` |
| a duel's guns are mirror images | `GunsNotMirrored` |
| a duel's first gun is the left one, the pivots at least 44 apart (the chassis plus twice the blast's reach, so no one blast reaches both guns) | `GunsOutOfOrder`, `GunsTooClose` |
| a challenge has 1–3 targets; a duel has none (Phase 1) | `NoTargets`, `TooManyTargets`, `StructuresInDuel` |
| a structure's footprint is off the edge columns | `StructureOffScreen` |
| **not floating:** at least half its footprint within 4 units of its seat (a warning, not a block: owner decision) | `StructureFloating` |
| **not buried:** no ground higher than its top on both sides within 3 columns | `StructureBuried` |
| in front of the gun, 8 columns clear of its chassis | `StructureBehindGun`, `StructureOnGun` |
| target centres at least 40 apart (§2.3 step 5) | `StructuresTooClose` |
| wind within 7.0 either way; a range's ends in order | `Wind` |

The right gun of a duel is seated as versus seats it, in the mirrored frame:
its chassis is the mirror image of the left gun's, columns `g - 12` to
`g + 13` rather than `g - 13` to `g + 12`.

Floating is measured at the footprint's median column because a structure
always stands on its highest column: on a slope, part of it is always in the
air. "At least half its footprint near the ground" lets a silo stand on a
hillside and stops one balanced on a spire's tip.

**The generator does not meet that rule.** In about 9% of run screens it
stands a structure on a spire, a ridge's flank or a chasm lip with most of
the footprint in the air (a quarter of `Spires` screens, a sixth of `Ridge`
ones; NOTES §9). Forks of those screens need an edit before they pass. That
is the rule working, but it is also a question about the generator (§13.13).

**Solvability.** A layout that passes placement then goes through the same
gate as a generated screen (§2.4), for the base loadout, at each **extreme
of its wind**: the fixed wind itself; or, for a range, its strongest and
weakest strengths blowing each way (calm, when the range starts at zero).

- **One gun** (`layout::validate`): every target needs `MIN_SOLUTIONS` (3)
  distinct shots that fell it, from `solver::validate`.
- **Two guns** (`custom::validate`, in the Godot crate because the sim knows
  nothing of versus): each gun needs 3 distinct shots that hit the other, from
  the duel gate's own `shots_at` in the mirrored frame (§11.3).

A one-gun check takes 47 µs at the median and under 300 µs at worst, a duel check
about 100 µs, so the editor can recheck after every edit.

A **fixed duel wind is checked both ways** (`custom::duel_winds`), since a
match on the layout turns it round every pair of turns (owner decision).

**Fairness and mirror tools** for duels: `Layout::asymmetry` (the largest
difference between a column and its mirror), `Layout::mirror_left` (copy the
left half onto the right, exactly) and `custom::duel_counts` (each gun's
shot count in each wind checked; a side with fewer shots has the harder
ground). On mirrored ground the counts are mirror images exactly, which a
test holds. The editor's fairness sign (§13.11) counts past the gate's cap
of 8, which nearly every playable duel reaches on both sides: to
`editor::FAIRNESS_CAP` (64), a side more than a tenth (and two shots) short
reading HARDER. That costs about 0.9 ms against the gate's 0.1, so a drag's
frames check only the gate (`editor::quick_verdict`) and the stroke's end
the whole verdict (NOTES §9).

### 13.6 Fork from a seed

- `Layout::from_run_screen(seed, screen)`: the run's screen generated at 320
  for the base loadout, written down. Its wind becomes a fixed wind.
- `custom::fork_duel(match_seed, round)`: the duel round generated at 320
  under the game's rules. Its guns are at 30 and 289, its wind the game's
  per-turn range (`Range { min: 0, max: 2500 }`), held exactly.

Two things the fork cannot carry, both documented on the functions:

- **The loadout.** A layout has none, so a run screen is forked as a fresh run
  with the base loadout meets it. A real run deep into its drafts may have
  been dealt a screen the base loadout could not reach.
- **Another width.** A fork is generated at 320. On a wider phone a run's
  screen can differ toward its right edge, and a duel's differs everywhere,
  since it is laid out between the guns and mirrored about the middle.

**What is exact and what is not.** The fork keeps the structures (kind and
column), the guns and a duel's wind exactly, and moves every height and
structure seat by at most 1/32 and a run's wind by at most 0.0005. It is
**not** bit-identical to the generated screen, so its state hash is not the
generated screen's hash. The hash covers every height's bit pattern (§9.3)
and a generated height is an arbitrary double, which no grid of integers
short of the full 49-bit fixed point can hold (about 3,080 characters a
code, §13.2).

What is exact, and tested:

- A fork is a fixed point: forking the screen a fork builds gives back the
  same layout.
- A fork's code rebuilds a screen whose state hash equals that of the screen
  the fork builds, so every device that reads a code plays the same screen.
- Shots fired at a fork end as they do at the generated screen 99.92% of the
  time for runs and 99.3% for duels, over 4,000 run screens and 300 duel
  rounds (NOTES §9). In that sample every forked duel, and every forked run
  screen that passes placement, passes the solvability gate.

The brief asked for the generated screen's own hash. That is only possible
by keeping every bit, or by keeping the seed, both rejected in §13.14.

### 13.7 Share codes

A code is `SL` and then Crockford base32 of the bytes: the format byte, a
bit-packed body, and a CRC-16. The body is the width, the name, the wind,
the guns, the structures, the first height, and each later column's
**second difference**, the change in slope, Rice-coded. The layout of every
field is in `layout/code.rs`.

- **Second differences**, because ground is smooth: a column is almost where
  the two before it point, so a hill becomes a run of small numbers and a
  flat or evenly sloping stretch becomes zeros.
- **Rice coding**, with the parameter chosen per code, spends `1 + k` bits on
  small values. A value that would run long is escaped and written raw, so no
  column costs more than 42 bits.
- **Run-length** as a second mode, picked by one bit when it is shorter:
  editor-made ground has long flat or straight stretches, and level ground
  codes in 28 characters rather than 89.
- **Base32, not base64.** A fifth longer, but every character is on the seed
  keypad and in a QR code's alphanumeric set, case does not matter, and
  reading `O` as `0` and `I` or `L` as `1` (spaces and hyphens skipped) lets
  a code survive being read aloud or retyped.
- **CRC-16** catches every single mistyped character (a test tries them all)
  and all but one in 65,536 other errors. A layout that passes the checksum
  still goes through validation before it is played.

**Size budget: 400 characters for any fork.** Measured over 4,000 run forks
and 300 duel forks: a median of 206 (runs) and 194 (duels), a worst case of
380 and 374. A hand-made layout of level ground and one target is 28.
Copying, pasting, a QR code or a relay message carries that easily; typing
it on the keypad does not, and isn't meant to.

**Decoding is total.** Text longer than 4,096 characters is refused before
anything is read, every count comes from a fixed-width field and is checked
against its limit before it is used, and every read past the end is an
error. Any text at all is a layout or a `CodeError`, never a panic, and a
code that decodes re-encodes to a code for the same layout. A property test
round-trips random layouts, and a fuzz target (`layout_code`, in the weekly
job) feeds the decoder both raw text and bodies framed with a valid checksum,
so mutations reach the bit-level decoder rather than stopping at the CRC.

### 13.8 Local storage (built)

`layouts_file.rs`, engine-free and fuzzed (`layouts_file`, in the weekly
job).

- **Slots in a text file**, `user://layouts.txt`: a `layouts 1` header, then
  one line per saved layout, `layout <slot> <code> [origin]`, the slot a
  number from 1 to 32 (`MAX_SLOTS`). Plain text the player owns, like the
  save (README, "Nothing for sale"): a layout is already text, so the file
  is just a list of codes. `origin` is where a fork came from
  (`seed:<text>:<round>` or `match:<seed>:<round>:<width>`), kept beside the
  code rather than in it, so the share code's format is unchanged (§13.13,
  Provenance).
- **Durable as the save is** (`app.rs`, "Order is behaviour"): written only
  through an `Effect::Store` (`Store::Layouts`), replaced atomically through
  a temp file and a rename (`save::write`), never written after a panic, and
  reported back through `Game::stored`. Memory takes the new file only once
  the write is confirmed, and only then is a draft it saved cleared, so a
  failed write says MAP NOT SAVED - DRAFT KEPT and loses nothing. At launch
  the copy is chosen as the save's is (a whole file over a whole temp).
- **Bundled layouts** ship read-only as codes (`custom::bundled`), compiled
  in rather than read from `res://`: the same text, testable without an
  engine. They are listed before the slots.
- **Kept when unread.** A line this build cannot read (another tag, a slot
  out of range or taken twice) is kept verbatim and written back after the
  slots; a code in a newer `LAYOUT_FORMAT` keeps its slot, shown dimmed as
  NEWER VERSION; a layout that no longer passes the duel gate is shown
  dimmed and marked FAILS, and does not start a match. A file whose header
  is from a newer build is never written. Nothing a player made is thrown
  away by a build that does not understand it.
- **The draft**, `user://draft.txt`: the layout being edited, one
  newline-terminated, versioned line (`draft 1 <mirror> <slot> <origin>
  <code>`), kept by `resume`'s file rules at its own three paths as the
  match file is (§11.6): replaced atomically after every edit that changed
  it (as a finger lifts, or at once if the app loses focus mid-drag), set
  aside while a launch reads it. It is cleared only once saved (the
  revision that was saved) or discarded and the editor left. A draft from a
  newer build (a later line format, a later code format, or a layout this
  editor cannot hold: one gun, another width) is put back unread and never
  written over or cleared; a note at launch and on opening the editor and
  the start page say so, and a map made meanwhile is kept by saving it. A
  draft the last launch died reading is dropped with a note. A code format
  before this build's is an older build's, not a newer one's.
- **A line's origin it cannot read** (a newer build's kind, a hand edit)
  never costs the layout: in the slot file it is kept verbatim after the
  code, in the draft it is dropped and the draft kept. A slot named by any
  line, read or kept, is never offered as free, and one whose code this
  build cannot read is never saved over or deleted. A slot file that cannot
  be read at all (too large, or torn with NULs) is never written over.
- **Demo isolation (§12.3).** The demo never reads or writes the layouts
  file or the draft: it starts only from the title and never opens the
  editor, every write checks for the demo too, and both are disk effects,
  so the demo's backstop drops and flags any that slip through. The demo's
  duels are generated: the GROUND choice is set aside with the rest of the
  versus state. A test runs a whole demo cycle over both files and checks
  them byte for byte.

### 13.9 Custom duels

**Built (Phase 1).** `custom::layout_match` turns a two-gun layout into a
versus `Match`. Its screen becomes a `versus::Ground`, dealt fresh (no
craters, guns home) at the start of every round instead of a generated
screen. Its wind becomes the rules' wind range, so each turn draws from the
layout's range by `turn_wind`, or, for a fixed wind, holds that wind every
turn in the shared frame, turned round every pair of turns from the start
of each round (Phase 3, owner decision: `Match::wind_at`, a pure function
of the turn, which a match log replays). Hits, rounds, equal shots and aim
help are
versus's own. **The guns do not roll on a layout** (`Match::on_ground`
turns the roll off whatever the pre-match screen says): versus's ladder is
counted from `EMPLACEMENT_X`, so a layout's guns would snap to the default
spots or never roll, and filling the craters under them would rewrite the
author's ground. Rolling from a layout's own spots is later work, and off
by owner decision for Phase 3 (§13.13).

`--salt-layout <code>` starts a custom duel at the first handoff, so no
pre-match screen is shown to say the guns will not roll; the GROUND row
does (below). A code that does not read or pass, or a one-gun layout,
leaves the title up with a note (and the console says why).

**Built (Phase 3, with the editor; §13.12).**

- The pre-match screen has a GROUND row: GENERATED (today's duel), the
  bundled layouts, then the player's slots, each with its name and its
  four-character fingerprint (no silhouette: the row budget went to the
  `<` beside it). The row steps on (Down, or tap it) and back (`<`). With a
  layout picked, ROLLING GUNS reads OFF, greyed, and does not toggle. A
  layout that no longer passes is shown dimmed and does not start.
- A rematch keeps the ground; so does RESUME MATCH, which picks it again
  on the GROUND row. A match suspended to fork it picks its slot again by
  where it came from, so if the map was saved over that slot meanwhile the
  match goes on on the map it began on and the rematch plays the map as
  saved (a note says so). A map neither bundled nor saved is checked
  against the gate before a rematch may start on it.
- Four bundled duel layouts, RIDGE, MESAS, CANYON and SPIRES (fixed wind),
  each made with the editor's own tools from a blank draft
  (`custom::bundled::recipe`). A CI test decodes every code, holds it to its
  recipe, the duel gate and a FAIR fairness sign.
- A match on a layout is kept on disk like any other (§11.6), its line
  carrying the layout's code (§13.4). A pasted code goes into the editor,
  from where it can be saved and played.

**Challenges and courses (later).** A one-gun layout plays as a run screen
does: a gun, targets, the base loadout, shots counted. A course strings a few
together with a par. Both reuse `Screen::from_layout` and the one-gun gate as
they are. Scoring and where results are kept are open (§13.13).

**Sharing (later).** The online versus design (an async relay with invite
codes) could carry layout codes as they are: a code is text, small, and
checks itself. Nothing in Phase 1 depends on it.

### 13.10 Independence and scope (§7)

- **Existing structures only.** A layout places the five kinds of §5.4 and
  nothing else: no figures, no new art, no imported images.
- **No free text.** A name is at most 12 characters from the seed keypad
  (`A`–`Z`, `0`–`9`), the same as a seed already allows, and nothing else in
  a code is text. Whatever a shared code carries, it can say no more than a
  shared seed can. (Whether names need a word filter before sharing exists
  is an open question.)
- **Its own terrain.** Layouts are ground the game's own generator drafted or
  the player drew, with the game's own rules.

### 13.11 The editor (built)

`editor.rs` is the model (the draft and every edit, engine-free and
proptested); `app/editor.rs` the pages, input and files; `render/editor.rs`
the drawing.

**Where it lives.** The phone title has no row to spare (§11.6), so the
editor is reached from the versus pre-match screen, the mode its maps are
for: a MAKE MAP plate in its top-right corner on a touchscreen (as the
title's DEMO and MUS plates are, so the rows stay put) and a keyed MAKE MAP
row (`E`) otherwise; and from a match's pause and result plates, MAKE MAP OF
THIS ROUND (`E`). With a draft kept, it opens straight onto it; otherwise on
a start page: NEW BLANK MAP, FORK A SEED, OPEN A MAP (bundled and saved),
PASTE A CODE. Leaving goes back to the pre-match screen; opened by MAKE MAP
OF THIS ROUND over a match it suspended, the menu's last row is BACK TO
MATCH and goes straight back into it. A match on a map forks to the map
itself, name and slot and all.

**The editing screen.** Three rows of buttons, each 18 rows tall (as tall as
the aim buttons), all above the world's ceiling: the top 72 rows of the
180-row buffer are sky whatever the ground does, so no button ever covers
ground a finger means to edit. Row one: MENU, the verdict, UNDO, REDO. Row
two: SCULPT, SMOOTH, STAMP, GUNS, WIND, MIRROR, TEST. Row three: the tool's
options (brush S/M/L; the five stamps; CALM/RANGE/FIXED and the wind's
numbers with `-` `+`). Everything below is the canvas, where a drag is a
stroke with the tool in hand, one undo step however long. The draft's name
(or origin) and fingerprint sit bottom left. Drawn at 320 columns,
letterboxed on a wider phone, inside its corner pad. A kid's path: tap a
tool, put a finger on the ground and pull it up into a hill; tap STAMP and
tap where a mesa goes; tap TEST and fire both guns; UNDO undoes anything.

**Keys and a controller.** `[` `]` (LB/RB) step the tool, `,` `.` (X/Y) its
option, the arrows or d-pad move a cursor (Shift: five columns) and nudge
the ground a unit under the brush, Enter (A) applies a stamp or a smoothing,
Z/Y undo and redo, I mirrors, T tests, C copies, P pastes, Esc (B) opens the
menu, whose rows a focus reaches (with UNDO, REDO, MIRROR and TEST among
them when no touchscreen is in use). The keypad pages take typed keys, or
the d-pad over the keypad as seed entry does.

**SCULPT is reversible within a stroke.** How far the finger is above or
below where it went down is how far the ground under the brush rises or
falls, from the stroke's start, in whole world units (coarse snapping: a
finger is not a stylus; a buffer pixel is a world unit), the brush's centre
snapped to two columns. Sliding sideways paints the change along, and
dragging back to the start undoes it. A soft brush (`dmath::bump`) at three
sizes. **SMOOTH** smooths a pass under the brush at each move. **STAMP**
adds the generator's own template (`terrain::shape` on level ground) at the
point tapped; dragging moves it rather than stamping a trail.
**GUNS** drags the gun nearer the finger, within the edge margin and
`MIN_GUN_GAP`, stopping at the last column it can be seated on.

**MIRROR** (on by default for a duel, owner decision, forks included: a
fork's tenth of asymmetry stays, every edit after it is mirrored) makes
every edit on both sides at once: brushes and stamps mirrored about the
middle and the guns moved as a pair. Turning it back on makes the draft
symmetric (the left half copied onto the right, the right gun put at the
left's mirror) as one undoable step. With it off, either side and either
gun can be edited alone, and the verdict says GUNS NOT MIRRORED, which a
duel cannot be played with (versus places the right gun as the left's
mirror; §13.13).

**WIND** sets CALM, RANGE or FIXED; `-` `+` move the chosen number by a
tenth, added to what is there, so a fork's thousandths (1.234) are never
snapped unasked; a range's ends stay in order. **NAME** is the seed keypad,
twelve characters at most.

**UNDO / REDO** keep 64 whole-draft snapshots. A stroke, a run of key
nudges at one spot, a run of WIND presses and a run of typing are each one
step; a press that changes nothing leaves no step. Replacing the draft
(BLANK, a fork, a paste, a map opened over it) is an edit too, so UNDO
brings the old draft back.

**The verdict** after every edit: a stop sign and the first problem that
blocks in a few words (LEFT GUN NOT ON GROUND, GUNS TOO CLOSE, RIGHT CAN'T
HIT - WIND 2.5, with `+N` for more), a warning triangle for one that only
warns, or a flag and READY with the fairness sign (LEFT HARDER, FAIR, RIGHT
HARDER); a warning mark over a gun a problem names. SAVE refuses while
anything blocks, saying why; warnings save.

**TEST FIRE** puts the draft's ground on screen with both guns: the gun to
fire (tap to switch), the wind (tap to step through calm and every wind
the gate checks), the gate's shot count for that gun in that wind, and the
match's aim controls. Shots are resolved exactly as a match's
(`versus::resolve`), with the HIT, MISS or OOPS called out; craters stay
until DONE and are thrown away with the test.

**Save, open, copy, paste.** SAVE lists the first free slot and the saved
ones (the draft's own slot chosen); a tap saves there. A saved draft is
cleared from disk, and the editor carries on with it. Overwriting a slot
(saving over a different layout) and DELETE in the slot list leave an UNDO
toast up, as does DISCARD MAP for the draft, its history and all (its file
kept until the editor is left, so a kill meanwhile loses nothing). A toast
goes as soon as the draft changes, so no UNDO from before an edit can take
the edit away; on the editing screen it takes the verdict's place in the
top row, over the sky, out of a drag's way. A page change spends the tap
that caused it (§11.4's "one touch, one screen"), so no double tap can
overwrite or delete. A finger that is dragging when another button or a
key ends its stroke is spent, so the stroke is never applied twice. COPY CODE
puts the share code on the clipboard; PASTE CODE reads it
(`Effect::ReadClipboard`, the adapter handing the text to `Game::pasted`),
decodes and loads it: a duel of the layout width goes in as the draft,
problems and all; text that is not a code, a one-gun layout or another
width is refused with a note.

**Fork this round** takes the round on screen's ground as dealt (no
craters) and the guns where they stand, cropped to 320 columns
(`editor::crop_live`): if both guns, chassis and margin, fit the centred
320-column window, that window; otherwise each gun keeps its own side, the
leftmost and rightmost 160 columns joined and the middle lost, the eight
columns either side of the join eased to meet half way. The guns stay
mirror images either way. An unfinished match is suspended, not ended. On a
Pixel (400 columns) with the guns home it is always the second rule. The
draft is labelled FROM MATCH R2 (a match seed is twenty digits nobody
reads). FORK A SEED is the keypad and a round: FROM SEED DUNE R2.

**Not built, from the proposal.** PLACE (structures, for challenges; hidden
for duels, which have no structures until structures as cover are designed,
§13.13) and HINTS (the gate's shots drawn as arcs) are Phase 4's. The mouse
wheel does not size the brush: S, M and L do, for every input alike. Codes
are pasted, never typed: two hundred characters is no one's idea of typing
on a keypad.

### 13.12 Phases

| Phase | What | Done when |
|---|---|---|
| **1 (built)** | data model, forks, share code, validation, `--salt-layout`, `--salt-fork-run`, `--salt-fork-duel`, `just layouts` | forks within 1/32 and the code round trip are tested; the decoder is fuzzed; generation and goldens unchanged; a code plays as a custom duel on desktop at 320 columns, letterboxed in a wider window. There is no phone path for `--salt-layout`: a phone has no command line, so the first phone play is Phase 3's |
| **3 (built)** | the editor and custom duels in the menus together: SCULPT, SMOOTH, STAMP, GUNS, MIRROR, WIND, NAME, UNDO, TEST FIRE, the draft kept on app kill, slots in the layouts file, the GROUND row on the pre-match screen, a rematch keeps the ground, fork this round and fork from a seed (§13.11), bundled layouts made with the editor's tools, match resume on a layout and pasting a code | a child makes a duel layout and plays it without help, and the owner and his son pick a bundled layout from the pre-match screen, both on the Pixel (to be confirmed by the owner: the PR's test script) |
| 4 | challenges and courses: one-gun layouts as a mode, PLACE and HINTS (STAMP came with Phase 3) | a course of bundled challenges plays start to finish with a result |
| 5 | sharing through the relay | a code sent from one phone plays on another |

The phase that was 2 (custom duels in the menus, alone) is folded into 3: a
GROUND row with only bundled layouts to pick needs those layouts made, and
the editor is the tool to make them.

### 13.13 Open questions

- ~~**Floating generated structures.**~~ Resolved (owner, October 2026): the
  rule stands as a warning, not a block, everywhere (`editor::severity`);
  nothing generated changes.
- **Structures in duels**, as cover. Versus flies a duel shot against the
  other gun's box only, so structures would need adding to its world, and
  rules for a gun behind a collapsed one.
- ~~**OWNER QUESTION: forking the round you just played, on a wider
  phone.**~~ Resolved (owner, October 2026): (a) for fork this round, a
  crop of the live screen keeping both guns (the rule is in §13.11), and
  (b) for fork from a typed seed, labelled as such.
- **Rolling guns on a custom duel (later).** Off on layouts (owner,
  October 2026), and shown OFF on the GROUND row (§13.9). The fix, when it
  is wanted, is a ladder measured from the layout's own guns, each
  spot checked for seating (§13.5) before the guns may stand there, craters
  under the guns filled back to the layout's ground (as now), and the roll's
  gate checking only the winds the layout can blow: a range's ends either
  way (and its weakest), or a fixed wind's own value, not calm plus
  `wind_max` both ways as versus's roll gate does today.
- **The gate checks the extremes, not the middle.** A range is checked at
  its ends (and its weakest strength); a wind in between is assumed no
  harder. That holds for every generated duel so far, but a layout built to
  defeat it (a gap that only a middling wind closes) could pass the gate and
  still have no shot at some winds. Sampling a few middle winds would cost
  little (§13.5's timings).
- **Guns that are not mirror images.** Versus places the right gun as the
  left's mirror. Freeing that means a second muzzle in its frames.
- ~~**A fixed wind in a duel**~~ Resolved (owner, October 2026): it turns
  round every pair of turns (`versus::Match::wind_at`), and the gate checks
  it both ways. A flip every round, built first, left rounds one and three
  of a best of three blowing the same way.
- ~~**SCULPT's model**~~ Resolved (owner, October 2026): kept as built, the
  ground moving by how far the finger is dragged, not pulled to the finger.
- ~~**Where the editor lives**~~ Resolved (owner, October 2026): on the
  versus pre-match screen (MAKE MAP), not the title.
- ~~**A floating structure on a fork**~~ Resolved: a warning, not a block,
  on any layout.
- ~~**Provenance.**~~ Resolved: a fork's origin is kept beside its code in
  the slot file and the draft (§13.8), not in the code, so the share code
  and its format are unchanged and a shared code says no more than the
  layout.
- **Guns that are not mirror images** stay unplayable in a duel: with
  MIRROR off the editor lets either gun be dragged alone, and the verdict
  blocks until they are mirrored again.
- **Names.** A word filter for names once codes can be shared?
- **Challenge scoring**, and where its results are kept.

### 13.14 Alternatives considered

- **A seed plus a diff.** An unedited fork would be a seed and nothing
  else, which is short. But it ties every layout to one version of the
  generator: each `SIM_EPOCH` bump would turn every shared layout into a
  different screen, unless old generators were kept for ever (§9.6 already
  declines that for replays). A diff against arbitrary doubles is no smaller
  than the heights themselves once anything is edited, and loading a layout
  would mean generating first. Layouts would stop being data.
- **Keeping every bit of the generated heights.** It gives the exact state
  hash of the generated screen, at about 3,080 characters a code, fifteen
  times the size, for differences in under 0.1% of shots (§13.2).
- **Base64url** is a fifth shorter than base32, but case-sensitive, full of
  characters easily misread (`l`, `1`, `I`, `O`, `0`), and has characters
  the keypad cannot type.
- **A readable text format** (one number per column) would be hand-editable,
  but a kilobyte or more per layout, and nobody edits 320 heights by hand. The
  layouts file is still plain text, as a list of codes.
- **Coding first differences, or bytes.** Slopes cost bits on every column
  under first differences; byte-aligned varints cost at least eight bits a
  column, over 500 characters.
- **Control points (a spline).** Small and pleasant to drag, but a fork of
  generated ground would not be exact to any useful grid, and stamping and
  sculpting would fight the spline. The editor can still offer smooth brushes
  over a heightfield.
- **Layouts at any width.** Each phone would show a different amount of the
  same layout, or stretch it. One width, letterboxed, means a layout plays
  the same everywhere (owner decision).
