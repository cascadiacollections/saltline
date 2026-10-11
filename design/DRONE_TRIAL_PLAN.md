# Drone Trial implementation plan

Status: shared motion plus headless Trial state and in-memory replay implemented;
no playable mode yet. Text replay persistence and a per-tick digest workload are implemented.
Cross-platform verification is wired into CI and awaits remote results.
Branch: codex/drone-trial. Existing artillery behavior remains the baseline.

## Decisions challenged

- Godot supports the reference style, but this project renders one CPU pixel
  buffer. Engine capability is not evidence that the current renderer supports
  detailed 720p assets efficiently. Measure a representative scene first.
- Reference images are flattened concept art, not production sprites. Obtain
  separate transparent hull, barrel, drone frames and clean background layers.
- Do not make a renderer rewrite a prerequisite for proving interception.
- Keep 320-world-unit artillery geometry. Display resolution must not change
  range, gravity, hitboxes, terrain samples, generation or old replay outcomes.
- Retain fixed 120 Hz simulation, seeded randomness and tick-addressed commands.
  Cross-platform determinism is an acceptance gate, not an untested promise.
- Preserve artillery's complete-shot lifecycle. Share small physics/presentation
  pieces rather than forcing both modes into a premature universal framework.
- Artillery's solvability solver does not validate timed moving targets. Trial
  needs reachable interception windows and playtesting for readable difficulty.
- Multiple simultaneous targets require earliest collision selection along each
  tick; vector order alone must not decide which drone gets hit.

## Initial scope

One coastal arena, fixed emplacement, angle/power aiming, limited ammunition,
short timed rounds, up to four drones, direct hits and separate trial scores.
No drafts, drone AI, seasonal gameplay arenas or terrain destruction initially.
These are proposed implementation defaults; evaluate duration, shell count,
reload delay, salvage rewards and drone speed in the prototype.

## Milestones and acceptance gates

1. Shared motion and collision primitives (started).
   Fixed-step projectile movement and swept translating-box collision; crossing
   and equal-motion regression tests. No dependencies added to the sim crate.
2. Headless Trial state and replay.
   Bounded drone/projectile collections, seeded schedules, integer tick clock,
   fire commands, reload, hit/escape events, terminal round state and scoring.
   Replay header carries mode, rules version, seed and configuration; commands
   carry tick and stable sequence order. Reject unsupported versions explicitly.
   Test expiry boundaries, multiple collisions, ammo rejection, pause semantics,
   repeated replay equality and cross-platform state digests in CI.
3. Playable low-resolution prototype.
   Menu entry, existing keyboard/controller aim, live projectiles and drones,
   timer/ammo/score HUD, restart and pause. Cap catch-up work without silently
   dropping simulation ticks. Decide overload behavior and input tick mapping.
   Tune interception feel before commissioning a full asset set.
4. Shared high-resolution presentation experiment.
   Render one artillery fixture and the coastal Trial arena using Godot 2D
   sprites and layers. Explicit world-to-display transform, y-up conversion,
   barrel pivot, camera mapping and collision visualization. Measure frame time
   and memory at 720p on an agreed minimum machine before committing a renderer.
   Artillery terrain visual masking must follow actual damaged heightfield;
   a decorative background cannot substitute for collision terrain.
5. Backport and release.
   Share HUD, tank animation, audio/event effects and camera tools where useful.
   Confirm original screenshot/replay baselines, modifiers, terrain damage,
   existing saves and daily runs. Isolate score tables by mode; migrate saves
   only when persistence is implemented. Complete existing release checks plus
   Trial controller, pause/resume, frame-rate and readability playtests.

## Reference asset checklist

Coastal image: clean sky/cloud/mountain/industry/water/foreground layers;
transparent side-view hull and pivot-aligned barrel; drone silhouette, rotor
and destruction frames; HUD font/icons with usable licenses.
Seasonal sheet: menu artwork initially. Gameplay variants need consistent
side-view perspective, dimensions and pivots, not crops of the illustration.
Keep large source images out of runtime assets until licensed and prepared.

## Completion evidence

Report measured frame times, playable-mode limitations, replay validation and
which shared improvements actually work in artillery. Do not label the mode
release-ready based only on unit tests or a convincing static screenshot.

## Current prototype rules

30 seconds (3,600 ticks), eight shells, half-second reload, 25 points per hit.
A spawn opportunity every 1.5 seconds, at most four drones, seeded heights and
leftward speeds. Wind is seeded once per round. Shells leave play at the arena
boundary; the arena has no destructible terrain. Commands execute before their
addressed tick; duplicate-tick commands retain recorded order. Expiry cancels
remaining shells without awarding late hits. Pause means not advancing ticks.
These rules are provisional, not a balance or reachability certification.

Headless validation covers reload/ammo/expiry, moving target crossings,
nearest-contact ordering, competing shells, bounded live entities and replay
agreement. Replay files now carry a mode/format/rules header and bounded command data.
The Trial digest workload compares 200 persisted replays and hashes every tick;
CI compares both artillery and Trial digests on Linux/macOS/Windows.
Next: the Godot menu/input/accumulator/rendering prototype.
