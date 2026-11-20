"""Failure cases for the engine comparison and profiling engine selection."""

import copy
import hashlib
import subprocess
import tempfile
import unittest
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

import android_profile
import engine_validation


def report():
    return {"format": 1, "audio_count": 13, "audio_hash": "1" * 16,
            "fixtures": [{"name": name, "frames": 120, "samples": 3,
                          "state_hash": "1" * 16, "frame_hash": "2" * 16,
                          "sound_hash": "3" * 16} for name in ("run", "duel", "map")]}


class EngineTools(unittest.TestCase):
    def test_android_overlay_survives_repeated_preparation_and_stock_reset(self):
        templates = android_profile.templates
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "templates"
            source.mkdir()
            android = root / "android"
            (android / "res" / "xml").mkdir(parents=True)
            (android / "AndroidManifest.xml").write_text(
                '<manifest xmlns:android="http://schemas.android.com/apk/res/android">'
                '<application android:appCategory="game"><meta-data '
                'android:name="android.game_mode_config" android:resource="@xml/game_mode_config"/>'
                '</application></manifest>')
            (android / "res" / "xml" / "game_mode_config.xml").write_text("<game-mode-config/>")
            presets = root / "export_presets.cfg"
            presets.write_text('[preset.3]\nname="Android"\n[preset.3.options]\n')
            aar = source / "stock.aar"
            entry = f"jni/{templates.ANDROID_ABI}/libgodot_android.so"
            with zipfile.ZipFile(aar, "w") as archive:
                archive.writestr(entry, b"stock")
                archive.writestr("classes.jar", b"java")
            with zipfile.ZipFile(source / "android_source.zip", "w") as archive:
                archive.writestr("src/main/AndroidManifest.xml", "<manifest><application/></manifest>")
                archive.writestr("gradlew", "")
                archive.writestr("libs/release/godot-lib.template_release.aar", aar.read_bytes())
            apk = source / "custom.apk"
            with zipfile.ZipFile(apk, "w") as archive:
                archive.writestr(f"lib/{templates.ANDROID_ABI}/libgodot_android.so", b"stripped")
            with patch.object(templates, "ROOT", root), \
                 patch.object(templates, "official_templates", return_value=source), \
                 patch.object(templates, "check_compiler"):
                presets.write_text(presets.read_text() + f'custom_template/release="{apk}"\n')
                for _ in range(2):
                    templates.prepare_android()
                    manifest = ET.parse(android / "build/src/main/AndroidManifest.xml")
                    self.assertEqual(len(manifest.findall("application/meta-data")), 1)
                    with zipfile.ZipFile(android / "build/libs/release/godot-lib.template_release.aar") as archive:
                        self.assertEqual(archive.read(entry), b"stripped")
                        self.assertEqual(archive.read("classes.jar"), b"java")
                presets.write_text('[preset.3]\nname="Android"\n[preset.3.options]\n')
                templates.prepare_android()
                with zipfile.ZipFile(android / "build/libs/release/godot-lib.template_release.aar") as archive:
                    self.assertEqual(archive.read(entry), b"stock")

    def test_android_preparation_drops_files_from_an_older_template(self):
        templates = android_profile.templates
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            android = root / "android"
            (android / "res").mkdir(parents=True)
            (android / "AndroidManifest.xml").write_text(
                '<manifest xmlns:android="http://schemas.android.com/apk/res/android"><application/></manifest>')
            (root / "export_presets.cfg").write_text('[preset.3]\nname="Android"\n[preset.3.options]\n')
            aar = root / "stock.aar"
            with zipfile.ZipFile(aar, "w") as archive:
                archive.writestr(f"jni/{templates.ANDROID_ABI}/libgodot_android.so", b"stock")

            def template(version: str, app: str) -> Path:
                source = root / version
                source.mkdir()
                with zipfile.ZipFile(source / "android_source.zip", "w") as archive:
                    archive.writestr("src/main/AndroidManifest.xml", "<manifest><application/></manifest>")
                    archive.writestr("gradlew", "")
                    archive.writestr("libs/release/godot-lib.template_release.aar", aar.read_bytes())
                    archive.writestr(app, "class App {}")
                return source

            old = template("4.7.2.stable", "src/main/java/com/godot/game/GodotApp.java")
            new = template("4.8.beta4", "src/main/java/com/godot/game/app/GodotApplication.java")
            java = android / "build" / "src" / "main" / "java" / "com" / "godot" / "game"
            with patch.object(templates, "ROOT", root):
                with patch.object(templates, "official_templates", return_value=old):
                    templates.prepare_android()
                self.assertTrue((java / "GodotApp.java").is_file())
                with patch.object(templates, "official_templates", return_value=new):
                    templates.prepare_android()
            self.assertFalse((java / "GodotApp.java").exists())
            self.assertTrue((java / "app" / "GodotApplication.java").is_file())

    def test_success_exit_without_validation_cannot_reuse_an_old_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            engine, pack, out = root / "engine", root / "pack.pck", root / "out"
            engine.write_bytes(b"engine")
            pack.write_bytes(b"pack")
            out.mkdir()
            (out / "report.json").write_text("old successful report")
            result = subprocess.CompletedProcess([], 0, "", "")
            with patch.object(engine_validation.subprocess, "run", return_value=result), \
                 self.assertRaisesRegex(RuntimeError, "failed"):
                engine_validation.capture(engine, pack, out, 1)
            self.assertFalse((out / "report.json").exists())

    def test_incomplete_run_cannot_pass_by_matching_itself(self):
        for bad in ({}, dict(report(), fixtures=[]), dict(report(), audio_count=0)):
            with self.assertRaises(ValueError):
                engine_validation.validate_report(bad)

    def test_state_or_frame_differences_fail_comparison(self):
        for key in ("state_hash", "frame_hash", "sound_hash"):
            changed = copy.deepcopy(report())
            changed["fixtures"][1][key] = "9" * 16
            with patch.object(engine_validation, "capture", side_effect=[report(), changed]), \
                 self.assertRaisesRegex(ValueError, "reports differ"):
                engine_validation.compare(Path("official"), Path("stripped"), Path("pack"), Path("out"))

    def test_current_pin_is_required_even_if_an_old_engine_is_cached(self):
        with patch.object(android_profile.templates, "read_lock", return_value={"key": "old"}), \
             patch.object(android_profile.templates, "key", return_value="new"), \
             patch.object(android_profile.templates, "fetch") as fetch:
            with self.assertRaisesRegex(RuntimeError, "no current pinned"):
                android_profile.pinned_engine(Path("cache"))
            fetch.assert_not_called()

    def test_cached_engine_is_verified_and_tampering_is_refetched(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            name = android_profile.templates.output_name("android", "arm64")
            engine = directory / name
            content = b"the pinned binary"
            lock = {"key": "key", "files": {name: hashlib.sha256(content).hexdigest()}}
            with patch.object(android_profile.templates, "read_lock", return_value=lock), \
                 patch.object(android_profile.templates, "key", return_value="key"), \
                 patch.object(android_profile.templates, "fetch") as fetch:
                engine.write_bytes(content)
                self.assertEqual(android_profile.pinned_engine(directory), engine)
                fetch.assert_not_called()
                engine.write_bytes(b"another engine")
                fetch.side_effect = lambda *_a, **_kw: engine.write_bytes(content)
                self.assertEqual(android_profile.pinned_engine(directory), engine)
                fetch.assert_called_once_with(directory, require=True)

    def test_profile_has_its_own_app_id(self):
        text = 'package/unique_name="com.cascadiacollections.saltline"\npackage/name="Salt Line"\n'
        changed = android_profile.configure_profile(text)
        self.assertIn(f'package/unique_name="{android_profile.PACKAGE}"', changed)
        with self.assertRaises(ValueError):
            android_profile.configure_profile(text + text)

    def test_failed_export_restores_the_original_preset_bytes(self):
        original = (b'[preset.3]\r\nname="Android"\r\n[preset.3.options]\r\n'
                    b'package/unique_name="com.cascadiacollections.saltline"\r\n'
                    b'package/name="Salt Line"\r\ncustom_template/release="local.apk"\r\n')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            presets = root / "export_presets.cfg"
            presets.write_bytes(original)
            with patch.object(android_profile, "ROOT", root), patch.object(android_profile.templates, "ROOT", root), \
                 self.assertRaisesRegex(RuntimeError, "export failed"), \
                 android_profile.profile_preset(root / "pinned.apk"):
                self.assertIn(android_profile.PACKAGE, presets.read_text())
                self.assertNotIn('custom_template/release="local.apk"', presets.read_text())
                raise RuntimeError("export failed")
            self.assertEqual(presets.read_bytes(), original)

    def test_sdk_settings_are_isolated_and_removed_after_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "build").mkdir()
            editor = root / "godot"
            editor.write_bytes(b"editor")
            with patch.object(android_profile, "ROOT", root), \
                 self.assertRaisesRegex(RuntimeError, "export failed"), \
                 android_profile.isolated_editor(str(editor), root / "sdk", root / "java") as staged:
                stage = Path(staged).parent
                self.assertTrue((stage / "_sc_").exists())
                settings = next((stage / "editor_data").glob("editor_settings-*.tres")).read_text()
                self.assertIn((root / "sdk").as_posix(), settings)
                self.assertIn((root / "java").as_posix(), settings)
                raise RuntimeError("export failed")
            self.assertFalse(stage.exists())
            self.assertEqual(editor.read_bytes(), b"editor")


if __name__ == "__main__":
    unittest.main()
