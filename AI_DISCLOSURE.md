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
| Art: engine sprites | **Yes, partly image-model** | Chassis and structure sprites are cut from the supplied *Salt & Iron* concept board (`art/source/salt-and-iron/01-salt-and-iron.png`) by `tools/import_concept_art.py`. Two cleanup images there (`clean-background.png`, `extracted-barrel.png`) were made with the built-in **ImageGen** tool (2026-10-10). **Origin of the concept board itself: owner to confirm** before submission. |
| Art: Counterweight's own pieces | Yes, code-generated | The Siege grade, trebuchet, stone and medieval structure skins are drawn by procedural scripts written with Claude Code during the jam (`gameoff/tools/`). The Salt Line title art and icon are not used. |
| Audio | Yes. Code-synthesized | Pre-existing Salt Line WAVs synthesized by `tools/gen_audio.py` (written with Claude Code). The SHIFT sound is synthesized the same way. |
| Design | Mixed | The phase/echo design came from the developer, refined in conversation with Claude. |
| itch page art (`salt-line/marketing`) | TBD | Record the file names and how each was made when imported. |

## Imported assets

None yet. Every file brought in from the workspace asset folders gets a row
here: file name, source, and how it was made.
