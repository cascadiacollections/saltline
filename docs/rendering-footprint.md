# One texture, two renderers: a Salt Line footprint audit

> Since #49 the frame is one whole-frame texture with up to twelve strip textures drawn over it, and only the strips that changed are uploaded. Since the layered frame (`backdrop`), the playfield's sky and ground are two more textures under those, uploaded only when they change. The one-texture, one-draw-call description below predates both; the memory findings still apply.

Salt Line draws its 320 × 180 playfield in Rust, uploads one RGBA image to Godot, and displays it as one textured quad. That made the desktop renderer seem like a minor choice. A memory snapshot showed otherwise.

I ran the same screen (`--salt-seed 4242 --salt-screen 6`) in Godot 4.7.2 on Windows ARM64, using the x64 build under emulation. The window was 1280 × 720 and hidden for measurement. Each row is a fresh process; these are process counters, so the exact numbers will vary by machine and graphics driver.

| Renderer | Private memory, 5 s | Private memory, 30 s | Working set, 30 s | Draw calls |
| --- | ---: | ---: | ---: | ---: |
| Forward+ | 569 MiB | 554 MiB | 599 MiB | 1 |
| Compatibility | 375 MiB | 363 MiB | 403 MiB | 1 |

Godot's own five-second monitors told the same story: tracked static memory fell from 50.1 MB to 32.9 MB, and reported video memory from 15.8 MB to 7.1 MB. The rendered frame still cost one draw call. [Godot's renderer guide](https://docs.godotengine.org/en/stable/tutorials/rendering/renderers.html) describes Compatibility as the low-base-cost choice for 2D games that do not need advanced rendering features. Salt Line's software-composited frame fits that description, so the project now selects Compatibility on desktop as well as mobile. Mobile already used it, so the change targets desktop runtime memory.

Package trimming was much smaller. The Windows export pack fell from **2,221,076** to **2,200,688 bytes** after excluding unused preview and parallax images and a Cargo metadata file. We kept `all_resources`: sprites and audio are loaded from constructed `res://` paths, so dependency-based export filtering could silently omit them. We also kept the imported project icon. Excluding it saved about 0.93 MB, but an isolated pack could no longer resolve the icon through `ResourceLoader`.

The optimized Rust release DLL is 4,192,768 bytes. The [successful CI build](https://github.com/cascadiacollections/saltline/actions/runs/36299878549) published a 39,285,419-byte Windows x64 ZIP and a 35,032,701-byte Android arm64 APK. Compared with the previous build (`6cb900b`), those downloads shrank by 18,514 and 21,820 bytes respectively; the Linux, macOS, and Windows ARM64 archives each shrank by about 18 KB. These are distribution sizes, not measured installed footprints. Selecting a renderer does not rebuild Godot's stock export template, so the large runtime memory saving translated into only a small download saving. The pack was also exported and smoke-tested locally, and Android and iOS pack exports completed without errors.

The useful lesson was to measure the engine's base cost before polishing the one draw call. Texture atlases and shaders would have done little for this renderer; selecting the leaner Godot backend saved roughly **190 MiB of process private memory** on the measured desktop setup.
