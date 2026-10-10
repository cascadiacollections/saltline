"""Guard authored sprite export contracts and preview source-of-truth wiring."""
import hashlib
import io
import shutil
import struct
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from PIL import Image

import build_site as site
import export_sprites as exporter
import gen_sprites as art


class SpritePipelineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_native_exports_match_canvas_and_palette_contract(self):
        palette = exporter.palette()
        for name in exporter.SIZES:
            with self.subTest(name=name):
                self.assertEqual(exporter.validate(Path(exporter.DST) / (name + ".png"), palette), [])

    def test_source_frames_not_tags_determine_export_contract(self):
        path = self.root / "barrel_strip_5deg.aseprite"
        path.write_bytes(struct.pack("<I5H", 128, 0xA5E0, 19, 28, 28, 32))
        self.assertEqual(exporter.source_errors(path), [])
        path.write_bytes(struct.pack("<I5H", 128, 0xA5E0, 1, 532, 28, 32))
        self.assertIn("expected frames/width/height", exporter.source_errors(path)[0])
        path.write_text("version https://git-lfs.github.com/spec/v1")
        self.assertIn("git lfs pull", exporter.source_errors(path)[0])

    def test_validation_rejects_canvas_partial_alpha_palette_and_clipping(self):
        path = self.root / "tank_base.png"
        Image.new("RGBA", (31, 12), (1, 2, 3, 128)).save(path)
        errors = exporter.validate(path, exporter.palette())
        self.assertTrue(any("canvas" in error for error in errors))
        self.assertIn("partial alpha", errors)
        Image.new("RGBA", (30, 12), (1, 2, 3, 255)).save(path)
        self.assertTrue(any("off-palette" in error for error in exporter.validate(path, exporter.palette())))
        path = self.root / "barrel_strip_5deg.png"
        Image.new("RGBA", (532, 28), (*art.P["hull"], 255)).save(path)
        self.assertTrue(any("edge pixels" in error for error in exporter.validate(path, exporter.palette())))

    def test_barrel_uses_nearest_exported_frame(self):
        for angle, frame in ((0, 0), (2.49, 0), (2.5, 1), (45, 9), (90, 18)):
            with self.subTest(angle=angle):
                barrel, pivot = art.load_barrel(angle)
                self.assertEqual(pivot, (3, 24))
                with Image.open(Path(art.SPR) / "barrel_strip_5deg.png") as strip:
                    expected = strip.crop((frame * 28, 0, (frame + 1) * 28, 28))
                    self.assertEqual(barrel.image().tobytes(), expected.tobytes())

    def test_saved_sprite_edits_reach_previews_and_site(self):
        sprites = self.root / "sprites"
        shutil.copytree(art.SPR, sprites)
        with patch.object(art, "SPR", str(sprites)):
            builders = (
                lambda: art.scene().image(), lambda: art.sheet().image(),
                lambda: site.compose(props=site.PROPS)[0].image(),
                site.favicon,
            )
            before = [hashlib.sha256(build().tobytes()).digest() for build in builders]
            # Alter the visible hull, using another valid palette colour.
            for name in ("tank_base", "tower_leaning"):
                path = sprites / (name + ".png")
                with Image.open(path) as source:
                    image = source.convert("RGBA")
                for y in range(image.height):
                    for x in range(image.width):
                        if image.getpixel((x, y))[3]:
                            image.putpixel((x, y), (*art.P["alert"], 255))
                image.save(path)
            for index, build in enumerate(builders):
                self.assertNotEqual(before[index], hashlib.sha256(build().tobytes()).digest(),
                                    f"builder {index} did not reflect the edited sprites")
            before = art.seasonal_sheet().image().tobytes()
            path = sprites / "tower_halloween_intact.png"
            Image.new("RGBA", (17, 34), (*art.P["alert"], 255)).save(path)
            self.assertNotEqual(before, art.seasonal_sheet().image().tobytes())

    def test_preview_build_does_not_overwrite_game_sprites(self):
        sprites = self.root / "sprites"
        shutil.copytree(art.SPR, sprites)
        before = {path.name: path.read_bytes() for path in sprites.glob("*.png")}
        with patch.object(art, "SPR", str(sprites)), patch.object(art, "OUT", str(self.root)):
            art.main()
        self.assertEqual(before, {path.name: path.read_bytes() for path in sprites.glob("*.png")})

    def test_invalid_export_does_not_overwrite_any_game_png(self):
        src = self.root / "source"
        dst = self.root / "export"
        src.mkdir()
        dst.mkdir()
        (src / "tank_base.aseprite").write_bytes(struct.pack("<I5H", 128, 0xA5E0, 1, 30, 12, 32))
        committed = dst / "tank_base.png"
        Image.new("RGBA", (30, 12), (*art.P["hull"], 255)).save(committed)
        before = committed.read_bytes()

        def invalid_export(source, output):
            Image.new("RGBA", (30, 12), (1, 2, 3, 255)).save(output)

        with patch.object(exporter, "SRC", str(src)), patch.object(exporter, "DST", str(dst)), \
                patch.object(exporter, "SIZES", {"tank_base": (30, 12)}), \
                patch.object(exporter, "ASEPRITE", "aseprite"), \
                patch.object(exporter, "export", invalid_export), \
                patch.object(sys, "argv", ["export_sprites.py"]), \
                redirect_stdout(io.StringIO()) as output:
            self.assertEqual(exporter.main(), 1)
            self.assertIn("off-palette", output.getvalue())
        self.assertEqual(committed.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
