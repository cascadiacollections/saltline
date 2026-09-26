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

4. **Material layering.** Assign layers by depth below the local surface:
   `Sand` (0–8 units), `Rock` (8–30), `Bedrock` (30+). Structures are placed as
   `Steel` columns. Material controls crater radius multiplier (§2.6) and
   determines what the tunneling modifier can pass through.

5. **Target placement.** 1–3 targets, count rising with band. Placed at
   template-defined anchor points, snapped to the surface. Minimum horizontal
   separation of 40 units so a single blast cannot trivially clear two.

6. **Solvability validation.** See §2.4. Reject and regenerate with
   `screen_seed + 1` on failure. Cap at 8 attempts, then fall back to a `Flat`
   template — a boring screen is better than a hang.

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

### 2.5 Difficulty curve

Drive everything from `band = screen_index / 4`, with the primary axes ramping
on different schedules so the player never feels one dial turning:

| Axis | Schedule | Cap |
|---|---|---|
| Target distance | `110 + band·6` units | 290 |
| Wind magnitude | `band·0.35`, resampled per screen | 7.0 |
| Terrain amplitude | `1.0 + band·0.05` × base | 2.2 |
| Target count | 1 → 2 at band 3, → 3 at band 9 | 3 |
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

Cap total scar depth at bedrock. Never allow terrain to be excavated below the
world floor, or the heightfield inverts and collision breaks.

### 2.7 Structural collapse

Targets are vertical structures anchored to the heightfield across a **footprint**
of 5–11 pixel columns. Each structure tracks:

```
integrity:  f32       // 1.0 → 0.0
support:    f32       // fraction of footprint columns still at original height
state:      Intact | Leaning | Collapsed
```

Two independent kill paths, and having both is the point:

- **Direct.** Blast within the structure's bounding box reduces `integrity` by
  blast overlap × material factor.
- **Undermining.** Craters inside the footprint reduce `support`. Recompute after
  every detonation as the mean remaining height across footprint columns,
  normalized to the original.

**State thresholds:**

| Condition | State |
|---|---|
| `integrity > 0.35` and `support > 0.6` | Intact |
| `integrity ≤ 0.35` or `support ≤ 0.6` | Leaning |
| `integrity ≤ 0` or `support ≤ 0.3` | Collapsed |

Leaning is a real state, not a cosmetic one: a leaning structure tilts toward the
weaker side of its footprint, which shifts its bounding box by several pixels and
changes the shot you need. It also grants no refund — the screen is not cleared
until every target reads Collapsed.

Collapse animates as a topple in the lean direction, then the debris adds a small
positive mound to the scar buffer where it lands. Terrain gets slightly higher
where a structure fell. That is a nice consequence to leave in.

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
| **Skipstone** | Shells ricochet once off any surface at ≤20° incidence, retaining 70% velocity. | DEMO | 2 |
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
| 3 | Sky gradient, dithered between bands |
| 3 | Player chassis — body, highlight, dark edge |
| 3 | Structures — body, highlight, damage tone |
| 2 | UI — foreground text, background plate |
| 1 | Alert accent, used only for wind and the shell counter under 10 |

Pull the ramp from desert reference photography rather than from another game's
palette — that keeps you clean on trade dress and produces better colors anyway.
Salt flats have a genuinely strange color range: bleached white-pink ground
against a deep sky, with very little mid-tone. Lean into that. It gives you high
contrast, which is exactly what a game about reading terrain silhouettes needs.

Dither only for the sky gradient and for material transition bands. Dithering
everything is the other common low-res failure.

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
value, 3×5 for the screen index. Never scale a TTF down into the buffer. Draw the
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
- [ ] Single player, not alternating two-player duel
- [ ] No primate, fruit, or skyline motif anywhere in art or naming
- [ ] Title has no lineage to either reference game
- [ ] Zero code, data, or asset lineage from any prior artillery title
- [ ] Palette and UI developed from reference photography, not from another game
- [ ] Employer personal-project review submitted and cleared before publishing
