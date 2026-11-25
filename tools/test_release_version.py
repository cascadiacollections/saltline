"""F-Droid version bookkeeping in tools/release_version.py."""

import contextlib
import io
import shutil
import unittest
from pathlib import Path
from unittest.mock import patch

import release_version

ROOT = Path(__file__).resolve().parent.parent


def stage(tmp: Path) -> None:
    for name in ("Cargo.toml", "project.godot", "export_presets.cfg"):
        shutil.copy(ROOT / name, tmp / name)
    (tmp / "fdroid").mkdir()
    shutil.copy(ROOT / "fdroid" / "version.txt", tmp / "fdroid" / "version.txt")


class ReleaseVersionTests(unittest.TestCase):
    def test_tree_matches_the_version_rule(self):
        release_version.check(None)

    def test_tag_version_code_for_0_2_0(self):
        self.assertEqual(release_version.release_code_for("0.2.0"), 2009999)
        self.assertEqual(release_version.android_code(1, True), 2009999)

    def test_set_android_rewrites_three_presets_and_keeps_store_ids(self):
        with self.temporary_tree() as tmp:
            with patch.object(release_version, "ROOT", tmp):
                release_version.set_android(2000042, "0.2.0-dev.42")
                presets = release_version.presets()
                version_file = (tmp / "fdroid" / "version.txt").read_text(encoding="utf-8")
            for name in ("Android", "Android Shareware", "F-Droid"):
                self.assertEqual(presets[name]["version/code"], "2000042")
                self.assertEqual(presets[name]["version/name"], "0.2.0-dev.42")
            self.assertEqual(presets["Android"]["package/unique_name"], "com.cascadiacollections.saltline")
            self.assertEqual(
                presets["Android Shareware"]["package/unique_name"],
                "com.cascadiacollections.saltline.shareware",
            )
            self.assertEqual(
                presets["F-Droid"]["package/unique_name"],
                "com.cascadiacollections.saltline.foss",
            )
            self.assertEqual(presets["Android"]["gradle_build/use_gradle_build"], "true")
            self.assertEqual(presets["Android"]["package/signed"], "true")
            self.assertEqual(presets["F-Droid"]["gradle_build/use_gradle_build"], "false")
            self.assertEqual(presets["F-Droid"]["package/signed"], "false")
            self.assertEqual(presets["macOS"]["application/short_version"], "0.2.0")
            self.assertIn("versionCode=2000042\n", version_file)
            self.assertIn("versionName=0.2.0-dev.42\n", version_file)

    def test_set_android_without_a_name_leaves_version_names(self):
        with self.temporary_tree() as tmp:
            with patch.object(release_version, "ROOT", tmp):
                release_version.set_android(2009999, None)
                presets = release_version.presets()
                code, name = release_version.fdroid_version_file()
            self.assertEqual(presets["Android"]["version/name"], "0.2.0")
            self.assertEqual(presets["F-Droid"]["version/name"], "0.2.0")
            self.assertEqual(presets["Android"]["version/code"], "2009999")
            self.assertEqual(code, "2009999")
            self.assertEqual(name, "0.2.0")

    def test_check_fails_when_the_fdroid_code_disagrees(self):
        with self.temporary_tree() as tmp:
            version = tmp / "fdroid" / "version.txt"
            version.write_text("versionCode=1\nversionName=0.2.0\n", encoding="utf-8")
            err = io.StringIO()
            with patch.object(release_version, "ROOT", tmp), \
                 contextlib.redirect_stderr(err), \
                 self.assertRaises(SystemExit) as raised:
                release_version.check(None)
            self.assertEqual(raised.exception.code, 1)
            self.assertIn("expected 2009999", err.getvalue())

    def temporary_tree(self):
        return tempfile_tree()


class tempfile_tree:
    def __enter__(self) -> Path:
        import tempfile

        self._tmp = tempfile.TemporaryDirectory()
        path = Path(self._tmp.name)
        stage(path)
        return path

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()


if __name__ == "__main__":
    unittest.main()
