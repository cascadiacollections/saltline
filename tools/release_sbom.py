#!/usr/bin/env python3
"""Turn cargo-cyclonedx's SBOM for the GDExtension into the SBOM for a release.
Standard library only and Python 3.9+, like release_version.py.

    release_sbom.py --cargo-bom crates/salt-line-godot/salt-line-godot.cdx.json \\
                    --godot 4.7.2 --out dist/SaltLine-sbom.cdx.json \\
                    [--commit SHA] [--repo owner/name]

cargo-cyclonedx sees only the Cargo graph: salt-line-godot and everything it
links. A shipped build is that library exported onto Godot's prebuilt export
template, plus, on Linux, the XDG Portals bridge built from a pinned revision
by tools/build_xdg_portals.sh. Neither is a Cargo dependency, so this adds
both as components and puts one application, Salt Line, at the top:

    Salt Line (application)
      -> salt-line-godot (the Cargo crate, with its whole dependency tree)
      -> Godot export templates (framework, every platform)
      -> Salt Line's stripped Godot engine (library, Android arm64 only,
         when tools/godot_templates.lock.json is current and the
         STRIPPED_ENGINE variable is not "off", which is exactly when CI
         exports with it: docs/godot-templates.md)
      -> godot-xdg-portals (library, Linux builds only)

It also makes the document reproducible and acceptable to actions/attest:
absolute checkout paths in bom-refs become repository-relative, and the
serialNumber (which cargo-cyclonedx omits when SOURCE_DATE_EPOCH is set, and
which actions/attest requires) is a UUIDv5 of the repository and commit, so
the same commit always yields the same document.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import uuid
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parent.parent
GODOT_VERSION = re.compile(r"^\d+\.\d+(\.\d+)?$")
XDG_REVISION = re.compile(r"^revision=([0-9a-f]{40})$", re.MULTILINE)
XDG_REPO = "KevinTCoughlin/godot-xdg-portals"
APP_REF = "salt-line"


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def relativise(node: Any, prefix: str) -> Any:
    """Replace the absolute checkout path in every string, so a bom-ref reads
    path+file://./crates/... on any machine."""
    if isinstance(node, dict):
        return {k: relativise(v, prefix) for k, v in node.items()}
    if isinstance(node, list):
        return [relativise(v, prefix) for v in node]
    if isinstance(node, str):
        return node.replace(prefix, "file://.")
    return node


def xdg_revision() -> str:
    script = (ROOT / "tools/build_xdg_portals.sh").read_text(encoding="utf-8")
    found = XDG_REVISION.search(script)
    if not found:
        fail("no `revision=<40 hex>` line in tools/build_xdg_portals.sh")
    return found.group(1)


def stripped_engine(godot: str) -> dict | None:
    """The Android engine library built from source by the godot-templates
    workflow, when the lock file pins a build for the current inputs (the
    Android job then exports with it, and fails rather than fall back) and
    the STRIPPED_ENGINE off switch is not thrown."""
    if os.environ.get("STRIPPED_ENGINE") == "off":
        return None
    sys.path.insert(0, str(ROOT / "tools"))
    import godot_template  # a sibling script, not a package

    lock = godot_template.read_lock()
    if lock.get("key") != godot_template.key():
        return None
    name = godot_template.output_name("android", "arm64")
    digest = lock.get("files", {}).get(name)
    if not digest:
        return None
    if godot_template.GODOT_VERSION != godot:
        fail(f"--godot {godot} but tools/godot_template.py builds {godot_template.GODOT_VERSION}")
    return {
        "type": "library",
        "bom-ref": f"godot-engine-stripped@{lock['key']}",
        "group": "godotengine",
        "name": "godot-engine-stripped",
        "version": f"{godot}-{lock['key']}",
        "description": (
            "Godot Engine built from source at the pinned tag without the "
            "features Salt Line never uses (tools/godot_build_profile.json), "
            "replacing libgodot_android.so in the official Android template. "
            "Built and attested by the godot-templates workflow. Godot itself "
            "is MIT; it bundles third-party code under other licences, listed "
            "in its COPYRIGHT.txt."
        ),
        "scope": "required",
        "licenses": [{"license": {"id": "MIT"}}],
        "purl": f"pkg:github/godotengine/godot@{lock['commit']}",
        "hashes": [{"alg": "SHA-256", "content": digest}],
        "externalReferences": [
            {"type": "vcs", "url": "https://github.com/godotengine/godot"},
            {
                "type": "distribution",
                "url": f"https://github.com/{godot_template.REPO}/releases/tag/{lock['release']}",
            },
        ],
        "properties": [{"name": "saltline:platforms", "value": "android-arm64"}],
    }


def build(cargo_bom: dict, godot: str, commit: str | None, repo: str) -> dict:
    if cargo_bom.get("bomFormat") != "CycloneDX":
        fail("the cargo SBOM is not CycloneDX")
    bom = relativise(cargo_bom, f"file://{ROOT}")

    crate = bom["metadata"]["component"]
    crate_ref = crate["bom-ref"]
    version = crate.get("version", "0.0.0")

    godot_ref = f"godot-export-templates@{godot}"
    godot_tag = f"{godot}-stable"
    godot_component = {
        "type": "framework",
        "bom-ref": godot_ref,
        "group": "godotengine",
        "name": "godot-export-templates",
        "version": godot,
        "description": (
            "Godot Engine's official prebuilt export templates: the engine "
            "runtime every build is exported onto (it bundles the engine's own "
            "third-party libraries, listed in its COPYRIGHT.txt). Installed in "
            "CI by chickensoft-games/setup-godot."
        ),
        "scope": "required",
        "licenses": [{"license": {"id": "MIT"}}],
        "purl": f"pkg:github/godotengine/godot@{godot_tag}",
        "externalReferences": [
            {
                "type": "distribution",
                "url": (
                    "https://github.com/godotengine/godot-builds/releases/download/"
                    f"{godot_tag}/Godot_v{godot_tag}_export_templates.tpz"
                ),
            },
            {"type": "vcs", "url": "https://github.com/godotengine/godot"},
        ],
    }

    rev = xdg_revision()
    xdg_ref = f"godot-xdg-portals@{rev}"
    xdg_component = {
        "type": "library",
        "bom-ref": xdg_ref,
        "name": "godot-xdg-portals",
        "version": rev,
        "description": (
            "Linux desktop portal bridge (GDExtension), built from source at "
            "this revision by tools/build_xdg_portals.sh. Linux builds only."
        ),
        "scope": "required",
        "licenses": [{"license": {"id": "MIT"}}],
        "purl": f"pkg:github/{XDG_REPO}@{rev}",
        "externalReferences": [
            {"type": "vcs", "url": f"https://github.com/{XDG_REPO}"}
        ],
        "properties": [
            {"name": "saltline:platforms", "value": "linux-x86_64,linux-arm64"}
        ],
    }

    engine_component = stripped_engine(godot)
    engine_refs = [engine_component["bom-ref"]] if engine_component else []

    app = {
        "type": "application",
        "bom-ref": APP_REF,
        "name": "Salt Line",
        "version": version,
        "licenses": [{"license": {"id": "MIT"}}],
        "externalReferences": [
            {"type": "vcs", "url": f"https://github.com/{repo}"}
        ],
    }
    if commit:
        app["purl"] = f"pkg:github/{repo}@{commit}"

    bom["metadata"]["component"] = app
    bom["components"] = (
        [crate, godot_component]
        + ([engine_component] if engine_component else [])
        + [xdg_component]
        + bom.get("components", [])
    )
    bom.setdefault("dependencies", []).insert(
        0, {"ref": APP_REF, "dependsOn": [crate_ref, godot_ref, *engine_refs, xdg_ref]}
    )
    tools = bom["metadata"].setdefault("tools", [])
    if isinstance(tools, list):
        tools.append({"vendor": repo, "name": "tools/release_sbom.py"})

    seed = f"https://github.com/{repo}/commit/{commit}#sbom" if commit else None
    serial = uuid.uuid5(uuid.NAMESPACE_URL, seed) if seed else uuid.uuid4()
    bom["serialNumber"] = f"urn:uuid:{serial}"
    return bom


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--cargo-bom", type=Path, required=True)
    parser.add_argument("--godot", required=True, help="Godot version, e.g. 4.7.2")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--commit", help="commit the release is built from")
    parser.add_argument("--repo", default="cascadiacollections/saltline")
    args = parser.parse_args()

    if not GODOT_VERSION.match(args.godot):
        fail(f"--godot {args.godot!r} is not a version like 4.7.2")
    if args.commit and not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        fail(f"--commit {args.commit!r} is not a full commit SHA")
    try:
        cargo_bom = json.loads(args.cargo_bom.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"{args.cargo_bom} not found; run cargo cyclonedx first")

    bom = build(cargo_bom, args.godot, args.commit, args.repo)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(bom, indent=2) + "\n", encoding="utf-8")

    crates = sum(1 for c in bom["components"] if c.get("purl", "").startswith("pkg:cargo/"))
    print(
        f"{args.out}: {len(bom['components'])} components "
        f"({crates} Cargo crates, Godot {args.godot}, godot-xdg-portals), "
        f"{args.out.stat().st_size} bytes, {bom['serialNumber']}"
    )


if __name__ == "__main__":
    main()
