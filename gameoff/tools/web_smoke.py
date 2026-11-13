"""Counterweight web smoke test: serve the Web export and play a little of it
in a headless browser.

    python3 gameoff/tools/web_smoke.py gameoff/godot/build/web out/ [--channel chrome]

Serves the export with a plain static server (no COOP/COEP: the no-threads
build must not need them), loads index.html, waits for the engine to start,
screenshots the title, presses Enter (start an Expedition), fires, then
shifts era, screenshotting each. Writes console.log, boot.png, play.png, fired.png
and shifted.png to out/. Adapted from the gameoff/web-spike feasibility test.
Fails on a page error, an engine abort or a GDExtension load failure.
Needs `pip install playwright` and a browser channel (or `playwright install chromium`).
"""

from __future__ import annotations

import argparse
import functools
import http.server
import pathlib
import sys
import threading
import time

from playwright.sync_api import sync_playwright

FATAL = (
    "Aborted(",
    "LinkError",
    "RuntimeError",
    "Wasm module",
    "GDExtension dynamic library not found",
    "Can't open dynamic library",
    "No GDExtension library found",
    "SharedArrayBuffer",
    "Cross-Origin Isolation",
)


class Quiet(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".pck": "application/octet-stream",
    }

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002, D102
        pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("export_dir", type=pathlib.Path)
    ap.add_argument("out_dir", type=pathlib.Path)
    ap.add_argument("--channel", default=None)
    ap.add_argument("--boot-seconds", type=float, default=25.0)
    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    handler = functools.partial(Quiet, directory=str(args.export_dir))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_address[1]}/index.html"

    lines: list[str] = []
    with sync_playwright() as p:
        browser = p.chromium.launch(
            channel=args.channel,
            headless=True,
            # Software WebGL2 so a GPU-less runner can still draw.
            args=["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
        )
        page = browser.new_page(viewport={"width": 1280, "height": 720})
        page.on("console", lambda m: lines.append(f"[{m.type}] {m.text}"))
        page.on("pageerror", lambda e: lines.append(f"[pageerror] {e}"))
        t0 = time.monotonic()
        page.goto(url)
        page.wait_for_selector("canvas")
        # The status overlay is hidden once the engine has started the main loop.
        started = None
        deadline = t0 + args.boot_seconds
        while time.monotonic() < deadline:
            hidden = page.evaluate(
                "() => { const s = document.getElementById('status');"
                " return !s || getComputedStyle(s).visibility === 'hidden' || s.style.display === 'none'; }"
            )
            if hidden:
                started = time.monotonic() - t0
                break
            time.sleep(0.25)
        time.sleep(4)  # a few seconds of frames on the title
        page.screenshot(path=str(args.out_dir / "boot.png"))
        page.mouse.click(640, 360)  # focus the canvas (and a user gesture, for audio)
        page.keyboard.press("Enter")
        time.sleep(4)
        page.screenshot(path=str(args.out_dir / "play.png"))
        page.keyboard.press("Space")
        time.sleep(1.0)
        page.screenshot(path=str(args.out_dir / "fired.png"))
        time.sleep(4)
        page.keyboard.press("Shift")
        time.sleep(1.5)
        page.screenshot(path=str(args.out_dir / "shifted.png"))
        browser.close()
    server.shutdown()

    (args.out_dir / "console.log").write_text("\n".join(lines) + "\n")
    fatal = [line for line in lines if any(f in line for f in FATAL)]
    print(f"engine started: {'yes, after %.1fs' % started if started is not None else 'NO'}")
    print(f"console lines: {len(lines)}, fatal: {len(fatal)}")
    for line in fatal[:20]:
        print("  " + line[:300])
    return 0 if started is not None and not fatal else 1


if __name__ == "__main__":
    sys.exit(main())
