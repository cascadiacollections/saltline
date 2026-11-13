# AI disclosure: Game Off 2026 entry

This text goes verbatim into the itch.io page and the public repository README.
Update it in the same PR as any AI-assisted change.

## Summary

This game was built with substantial AI assistance. Code was written with
**Claude (Anthropic) via Claude Code**, directed and reviewed by the developer.

## By part

| Part | AI-assisted? | Detail |
| --- | --- | --- |
| `salt-line-sim` (library: terrain, ballistics, solver, RNG) | Yes | Written before the jam for Salt Line, largely with Claude Code (most commits are co-authored by Claude). Used unchanged as a library. |
| `shift-sim` (phase rules, echoes, modifiers, Expedition) | Yes | Written during the jam with Claude Code. |
| `shift-godot` (rendering, input, HUD, audio playback) | Yes | Written during the jam with Claude Code. |
| CI, web export and release tooling | Yes | Written with Claude Code. |
| Art | Yes. Code-generated, not image models | Pre-existing Salt Line sprites and backgrounds, produced by procedural scripts (`tools/gen_sprites.py` and related) written with Claude Code. Salt and Brine are palette remaps of that art. No new art was made for the jam. |
| Audio | Yes. Code-synthesized | Pre-existing Salt Line WAVs synthesized by `tools/gen_audio.py` (written with Claude Code). The SHIFT sound is synthesized the same way. |
| Design | Mixed | The phase/echo design came from the developer, refined in conversation with Claude. |
| itch page art (`salt-line/marketing`) | TBD | Record the file names and how each was made when imported. |

## Imported assets

None yet. Every file brought in from the workspace asset folders gets a row
here: file name, source, and how it was made.
