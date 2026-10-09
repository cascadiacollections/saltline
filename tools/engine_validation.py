#!/usr/bin/env python3
"""Compare official and stripped export templates on the same exported PCK.

python3 tools/engine_validation.py --official /path/to/linux_release.x86_64 \
    --stripped /path/to/godot.linuxbsd.template_release.x86_64 \
    --pack build/engine-validation/game.pck --out build/engine-validation/results

Each process gets a fresh export directory. Validation bypasses player-file
loading and crash-file writing. report.json is written only after all fixtures
and resource checks pass.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def validate_report(report: dict) -> None:
    if report.get("format") != 1 or report.get("audio_count") != 13:
        raise ValueError("incomplete asset validation report")
    rows = report.get("fixtures", [])
    if [row.get("name") for row in rows] != ["run", "duel", "map"]:
        raise ValueError("missing replay fixtures")
    for row in rows:
        if row.get("frames", 0) <= 0 or row.get("samples", 0) <= 1:
            raise ValueError(f"fixture did not run: {row}")
        for key in ("state_hash", "frame_hash", "sound_hash"):
            if len(row.get(key, "")) != 16:
                raise ValueError(f"missing {key}")


def capture(engine: Path, pack: Path, out: Path, timeout: int) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    report = out / "report.json"
    report.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="salt-engine-user-") as user:
        env = dict(os.environ, XDG_DATA_HOME=user, APPDATA=user)
        # Production templates disable --path/--main-pack. Stage an ordinary
        # exported executable + same-name PCK, with its native sidecar.
        stage = Path(user) / "export"
        stage.mkdir()
        binary = stage / ("game.exe" if os.name == "nt" else "game")
        shutil.copy2(engine, binary)
        shutil.copy2(pack, stage / "game.pck")
        for sidecar in pack.parent.iterdir():
            if sidecar.suffix in (".so", ".dll", ".dylib"):
                shutil.copy2(sidecar, stage / sidecar.name)
        command = [str(binary), "--headless", "--audio-driver", "Dummy",
                   "--", "--salt-validate-engine", str(out.resolve())]
        try:
            process = subprocess.run(command, cwd=stage, env=env, capture_output=True, text=True,
                                     errors="replace", timeout=timeout, check=False)
        except subprocess.TimeoutExpired as exc:
            output = exc.stdout or b""
            (out / "engine.log").write_bytes(output if isinstance(output, bytes) else output.encode())
            raise RuntimeError(f"{engine.name} timed out after {timeout}s") from exc
        log = process.stdout + process.stderr
        (out / "engine.log").write_text(log, encoding="utf-8")
        if process.returncode or "salt-engine-validation: PASS" not in log or "ERROR:" in log:
            raise RuntimeError(f"{engine.name} failed; see {out / 'engine.log'}")
    data = json.loads(report.read_text(encoding="utf-8"))
    validate_report(data)
    for name in ("run", "duel", "map"):
        if not (out / f"{name}.png").is_file():
            raise ValueError(f"missing {name}.png from {engine.name}")
    return data


def compare(official: Path, stripped: Path, pack: Path, out: Path, timeout: int = 180) -> None:
    reports = [capture(engine, pack, out / name, timeout)
               for name, engine in (("official", official), ("stripped", stripped))]
    if reports[0] != reports[1]:
        raise ValueError(f"engine reports differ; compare {out / 'official'} and {out / 'stripped'}")
    for name in ("run", "duel", "map"):
        if (out / "official" / f"{name}.png").read_bytes() != (out / "stripped" / f"{name}.png").read_bytes():
            raise ValueError(f"{name}: encoded frames differ")
    print("PASS: official and stripped engines match all replay states, sampled frames, sound events and audio lengths")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--official", type=Path, required=True)
    parser.add_argument("--stripped", type=Path, required=True)
    parser.add_argument("--pack", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=Path("build/engine-validation/results"))
    parser.add_argument("--timeout", type=int, default=180)
    args = parser.parse_args()
    compare(args.official, args.stripped, args.pack, args.out, args.timeout)
