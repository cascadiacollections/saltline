"""The 4.8-beta4 trial stays off the 4.7.2 pin."""

import hashlib
import io
import json
import os
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import godot_4_8_trial as trial
import godot_template


class Godot48Trial(unittest.TestCase):
    def test_stable_pin_and_trial_key_still_match(self):
        trial.guard_stable()
        self.assertEqual(godot_template.GODOT_VERSION, "4.7.2")
        self.assertEqual(godot_template.key(), json.loads(godot_template.LOCK.read_text())["key"])
        self.assertEqual(trial.custom_build_key(), trial.LOCK["custom_build"]["key"])
        self.assertNotEqual(trial.custom_build_key(), godot_template.key())

    def test_template_directory_defaults_to_the_stable_pin(self):
        with patch.dict(os.environ, {"GODOT_TEMPLATE_VERSION": ""}):
            self.assertEqual(godot_template.template_release_dir(), "4.7.2.stable")
        with patch.dict(os.environ, {"GODOT_TEMPLATE_VERSION": "4.8.beta4"}):
            self.assertEqual(godot_template.template_release_dir(), "4.8.beta4")
        with patch.dict(os.environ, {"GODOT_TEMPLATE_VERSION": "4.8-beta4"}), \
             patch("sys.stderr", io.StringIO()):
            with self.assertRaises(SystemExit):
                godot_template.template_release_dir()

    def test_python_3_9_is_refused_only_as_a_3_10_check(self):
        self.assertIsNotNone(trial.python_requirement_error((3, 9, 18)))
        self.assertIsNone(trial.python_requirement_error((3, 10, 0)))
        self.assertIsNone(trial.python_requirement_error((3, 12, 3)))
        text = trial.python_requirement_error((3, 9, 0)) or ""
        self.assertIn("4.8", text)
        self.assertIn("4.7.2", text)

    def test_editor_version_must_match_the_lock_commit(self):
        spec = trial.trial()
        self.assertIsNone(trial.editor_version_error("4.8.beta4.official.b070e63b1\n", spec))
        self.assertIsNone(trial.editor_version_error("4.7.2.stable.official.ed1daf0bf", trial.compare()))
        self.assertIn("commit", trial.editor_version_error("4.8.beta4.official.aaaaaaaaa", spec) or "")
        self.assertIn("4.8.beta4", trial.editor_version_error("4.8.beta1.official.b070e63b1", spec) or "")
        self.assertIn("unrecognized", trial.editor_version_error("Godot Engine", spec) or "")

    def test_published_sums_must_match_the_lock(self):
        artifacts = trial.trial()["artifacts"]
        published = {item["name"]: item["sha512"] for item in artifacts.values()}
        self.assertEqual(trial.provenance_errors(published, artifacts), [])
        published = dict(published)
        published[artifacts["export_templates"]["name"]] = "0" * 128
        errors = trial.provenance_errors(published, artifacts)
        self.assertEqual(len(errors), 1)
        self.assertIn("export_templates", errors[0])

    def test_profile_drop_is_only_the_removed_regex_module(self):
        adjusted = trial.adjusted_profile()
        shared = json.loads(godot_template.PROFILE.read_text())
        self.assertNotIn("module_regex_enabled", adjusted["disabled_build_options"])
        self.assertIn("module_regex_enabled", shared["disabled_build_options"])
        self.assertEqual(
            set(shared["disabled_build_options"]) - set(adjusted["disabled_build_options"]),
            {"module_regex_enabled"},
        )

    def test_profile_sets_turn_off_rendering_device_for_the_trial_only(self):
        adjusted = trial.adjusted_profile()
        shared = json.loads(godot_template.PROFILE.read_text())
        # 4.8 gates RD_ENABLED on `rendering_device`, not `vulkan`; without this the stripped
        # Android build fails in display_server_android.cpp (free_vulkan_global_context).
        self.assertIs(adjusted["disabled_build_options"]["rendering_device"], False)
        self.assertNotIn("rendering_device", shared["disabled_build_options"])
        self.assertIs(shared["disabled_build_options"]["vulkan"], False)

    def test_profile_set_already_in_the_shared_profile_is_refused(self):
        lock = json.loads(json.dumps(trial.LOCK))
        lock["custom_build"]["profile_sets"] = {"vulkan": False}
        with self.assertRaisesRegex(trial.TrialError, "already in the shared profile"):
            trial.adjusted_profile(lock)

    def test_profile_check_accepts_a_4_8_tree_and_rejects_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = self.tree(Path(tmp), regex=False, omit=None)
            trial.check_trial_profile(src)
            stale = self.tree(Path(tmp) / "stale", regex=True, omit=None)
            with self.assertRaises(trial.TrialError):
                trial.check_trial_profile(stale)
            missing = self.tree(Path(tmp) / "missing", regex=False, omit="module_ogg_enabled")
            with self.assertRaises(trial.TrialError):
                trial.check_trial_profile(missing)
            no_rd = self.tree(Path(tmp) / "no_rd", regex=False, omit=None, rendering_device=False)
            with self.assertRaisesRegex(trial.TrialError, "rendering_device"):
                trial.check_trial_profile(no_rd)

    def test_package_inspection_reports_native_libraries(self):
        with tempfile.TemporaryDirectory() as tmp:
            apk = Path(tmp) / "game.apk"
            payload = b"arm64-engine"
            with zipfile.ZipFile(apk, "w") as archive:
                archive.writestr("lib/arm64-v8a/libgodot_android.so", payload)
                archive.writestr("lib/arm64-v8a/libsalt_line_godot.so", b"game")
            info = trial.inspect_package(apk)
            engine = trial.find_library(info, "libgodot_android.so")
            self.assertIsNotNone(engine)
            assert engine is not None
            self.assertEqual(engine["bytes"], len(payload))
            self.assertEqual(engine["sha256"], hashlib.sha256(payload).hexdigest())
            self.assertEqual(info["bytes"], apk.stat().st_size)

    def test_size_table_and_cross_wired_engines(self):
        def packed(name: str, digest: str, size: int = 10) -> dict:
            return {"bytes": size, "libraries": [{"name": name, "bytes": size, "sha256": digest}]}

        stable = {
            "linux": {"bytes": 100, "libraries": [{"name": "libsalt_line_godot.so", "bytes": 5, "sha256": "a"}]},
            "template_linux_x86_64_bytes": 70,
            "template_android": packed("lib/arm64-v8a/libgodot_android.so", "stable-engine", 20),
            "apk": packed("lib/arm64-v8a/libgodot_android.so", "stable-engine", 30),
        }
        stable["apk"]["libraries"].append({"name": "lib/arm64-v8a/libsalt_line_godot.so", "bytes": 4, "sha256": "g"})
        preview = {
            "linux": {"bytes": 110, "libraries": [{"name": "libsalt_line_godot.so", "bytes": 5, "sha256": "a"}]},
            "template_linux_x86_64_bytes": 80,
            "template_android": packed("lib/arm64-v8a/libgodot_android.so", "preview-engine", 21),
            "apk": {"skipped": "no sdk"},
        }
        rows = dict((label, (left, right, change)) for label, left, right, change in trial.size_rows(stable, preview))
        self.assertEqual(rows["Linux export (engine + pck)"][2], "+10 B")
        self.assertEqual(rows["Salt Line APK"][1], "—")
        trial.check_packaged_engines(stable, preview)
        preview["apk"] = packed("lib/arm64-v8a/libgodot_android.so", "stable-engine", 30)
        with self.assertRaisesRegex(trial.TrialError, "4.8-beta4 APK"):
            trial.check_packaged_engines(stable, preview)

    def test_build_report_must_match_commit_key_and_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            binary = directory / "libgodot.android.template_release.arm64.so"
            binary.write_bytes(b"engine-bytes")
            report = {
                "tag": trial.trial()["godot"],
                "editor_version": trial.trial()["editor_version"],
                "commit": trial.trial()["commit"],
                "version_status": "beta4",
                "key": trial.custom_build_key(),
                "python": "3.12.3",
                "file": binary.name,
                "sha256": hashlib.sha256(b"engine-bytes").hexdigest(),
                "bytes": len(b"engine-bytes"),
            }
            (directory / "build-report.json").write_text(json.dumps(report), encoding="utf-8")
            trial.validate_build(directory)
            report["key"] = "0" * 16
            (directory / "build-report.json").write_text(json.dumps(report), encoding="utf-8")
            with self.assertRaisesRegex(trial.TrialError, "key"):
                trial.validate_build(directory)

    def test_import_lines_unique_to_4_8_are_the_ones_that_matter(self):
        stable = ["ERROR: missing texture", "GDExtension loaded"]
        preview = ["ERROR: missing texture", "SCRIPT ERROR: parse at res://godot/new.gd"]
        self.assertEqual(trial.only_in_trial(preview, stable), ["SCRIPT ERROR: parse at res://godot/new.gd"])
        self.assertEqual(
            trial.interesting_lines("noise\nSCRIPT ERROR: bad\nWARNING: verbose import\n"),
            ["SCRIPT ERROR: bad"],
        )

    def tree(self, src: Path, regex: bool, omit: str | None, rendering_device=True) -> Path:
        src.mkdir(parents=True, exist_ok=True)
        profile = json.loads(godot_template.PROFILE.read_text())
        quoted = []
        for option in profile["disabled_build_options"]:
            if option == omit:
                continue
            if option == "module_regex_enabled" and not regex:
                continue
            if option.startswith("module_") and option.endswith("_enabled"):
                name = option[len("module_") : -len("_enabled")]
                module_dir = src / "modules" / name
                module_dir.mkdir(parents=True)
                (module_dir / "config.py").write_text("# module\n", encoding="utf-8")
            else:
                quoted.append(f'"{option}"')
        if rendering_device:
            quoted.append('"rendering_device"')
        (src / "SConstruct").write_text("\n".join(quoted), encoding="utf-8")
        (src / "platform").mkdir(exist_ok=True)
        return src


if __name__ == "__main__":
    unittest.main()
