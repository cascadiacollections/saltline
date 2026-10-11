"""Guard platform selection and the determinism artifact contract."""
import json
import unittest

from ci_platforms import needs_apple, plan


class PlatformTests(unittest.TestCase):
    def test_gameplay_and_docs_defer_apple_to_main(self):
        self.assertFalse(needs_apple(["crates/salt-line-sim/src/world.rs", "godot/game.gd", "README.md"]))

    def test_build_and_apple_changes_require_apple(self):
        for path in ["Cargo.lock", "crates/salt-line-godot/Cargo.toml",
                     ".github/workflows/ci.yml", "tools/ci_platforms.py",
                     "export_presets.cfg", "crates/salt-line-godot/src/ios.rs"]:
            with self.subTest(path=path):
                self.assertTrue(needs_apple([path]))

    def test_matrix_matches_required_digest_count(self):
        for apple in (False, True):
            outputs = plan(apple)
            sim = json.loads(outputs["sim"])
            playable = json.loads(outputs["playable"])
            self.assertEqual("macos-latest" in sim, apple)
            self.assertEqual(any(p["os"] == "macos-latest" for p in playable), apple)
            self.assertEqual(int(outputs["digests"]), len(sim) + 1 + int(apple))
            self.assertEqual(len(playable), 3 + int(apple))


if __name__ == "__main__":
    unittest.main()
