"""Shipping art contracts and reproducibility, including embedded sky pixels."""

import json
from pathlib import Path
import tempfile
import unittest

from PIL import Image

import import_concept_art as concept
import build_detail_art as detail


class ConceptArtTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="salt-concept-")
        cls.output = Path(cls.directory.name)
        cls.products = concept.build(cls.output)
        cls.details = detail.build(cls.output / "detail")

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def test_source_color_assets_reproduce_at_four_times_logical_bounds(self):
        contracts = json.loads((concept.SOURCE / "native-contracts.json").read_text())
        for name, image in self.details.items():
            with self.subTest(name=name):
                self.assertEqual(image.size, tuple(n * 4 for n in contracts[name]))
                shipping = Image.open(detail.OUT / f"{name}.png").convert("RGBA")
                self.assertEqual(image.tobytes(), shipping.tobytes())
                self.assertEqual(
                    set(image.getchannel("A").get_flattened_data()), {0, 255}
                )
                colors = {p[:3] for p in image.get_flattened_data() if p[3]}
                self.assertGreater(
                    len(colors), 16, "source color detail must survive conversion"
                )

    def test_detail_embedded_textures_match_kept_pngs(self):
        sky = Image.open(detail.OUT / "sky.png").convert("RGBA")
        self.assertEqual(sky.size, (1280, 720))
        self.assertEqual(sky.tobytes(), (detail.OUT / "sky.rgba").read_bytes())
        soil = b"".join(
            Image.open(detail.OUT / f"{name}.png").convert("RGBA").tobytes()
            for name in ("sand", "rock", "bedrock")
        )
        self.assertEqual(soil, (detail.OUT / "soil.rgba").read_bytes())
        for name in ("sky.rgba", "soil.rgba"):
            self.assertEqual(
                (detail.OUT / name).read_bytes(),
                (self.output / "detail" / name).read_bytes(),
            )
        for name in ("sky.png", "title.png"):
            shipping = Image.open(detail.OUT / name).convert("RGBA")
            rebuilt = Image.open(self.output / "detail" / name).convert("RGBA")
            self.assertEqual(shipping.size, rebuilt.size)
            self.assertEqual(shipping.tobytes(), rebuilt.tobytes())

    def test_detail_barrels_keep_all_frames_inside_the_original_pivot_canvas(self):
        for name, image in self.details.items():
            if not name.startswith("barrel_strip_"):
                continue
            for frame in range(19):
                with self.subTest(name=name, frame=frame):
                    bounds = image.crop(
                        (frame * 112, 0, (frame + 1) * 112, 112)
                    ).getbbox()
                    self.assertIsNotNone(bounds)
                    self.assertGreaterEqual(bounds[0], 4)
                    self.assertGreaterEqual(bounds[1], 4)
                    self.assertLessEqual(bounds[2], 108)
                    self.assertLessEqual(bounds[3], 108)

    def test_title_cleanup_preserves_the_source_scene_without_a_rectangular_patch(self):
        board = Image.open(concept.SOURCE / "01-salt-and-iron.png").convert("RGBA")
        scene = concept.title_scene(board)
        self.assertEqual(
            scene.crop((440, 0, board.width, 626)).tobytes(),
            board.crop((440, 0, board.width, 626)).tobytes(),
        )
        self.assertEqual(
            scene.crop((0, 130, board.width, 626)).tobytes(),
            board.crop((0, 130, board.width, 626)).tobytes(),
        )
        patch = scene.crop((0, 0, 440, 130))
        self.assertTrue(all(p[0] <= p[2] for p in patch.get_flattened_data()))

    def test_title_preserves_the_wide_hero_scene_aspect_ratio(self):
        board = Image.open(concept.SOURCE / "01-salt-and-iron.png").convert("RGBA")
        title = concept.title_background(board, (1280, 720))
        # The source hero is 1672x626, so it occupies 1280x479, not 1280x720.
        scene = concept.title_scene(board).resize((1280, 479), Image.Resampling.NEAREST)
        self.assertEqual(title.crop((0, 241, 1280, 720)).tobytes(), scene.tobytes())
        for y in (0, 100, 240):
            self.assertEqual(
                title.crop((0, y, 1280, y + 1)).tobytes(),
                scene.crop((0, 0, 1280, 1)).tobytes(),
            )

    def test_shipping_assets_reproduce_from_kept_sources(self):
        contracts = json.loads((concept.SOURCE / "native-contracts.json").read_text())
        self.assertEqual(set(contracts), set(self.products))
        for name, image in self.products.items():
            with self.subTest(name=name):
                self.assertEqual(list(image.size), contracts[name])
                shipping = Image.open(
                    concept.ROOT / "art/sprites" / f"{name}.png"
                ).convert("RGBA")
                self.assertEqual(image.tobytes(), shipping.tobytes())

    def test_palette_and_binary_alpha_survive_import(self):
        palette = {(*rgb, 255) for rgb in concept.PALETTE}
        for name, image in self.products.items():
            with self.subTest(name=name):
                pixels = set(image.get_flattened_data())
                self.assertTrue(pixels - {(0, 0, 0, 0)})
                self.assertLessEqual(pixels, palette | {(0, 0, 0, 0)})
                self.assertNotIn(
                    (*concept.PALETTE[15], 255),
                    pixels,
                    "decorative art must not use versus alert ink",
                )

    def test_every_barrel_frame_is_present_and_unclipped(self):
        for name, image in self.products.items():
            if not name.startswith("barrel_strip_"):
                continue
            for frame in range(19):
                with self.subTest(name=name, frame=frame):
                    tile = image.crop((frame * 28, 0, (frame + 1) * 28, 28))
                    bounds = tile.getbbox()
                    self.assertIsNotNone(bounds)
                    self.assertGreater(bounds[0], 0)
                    self.assertGreater(bounds[1], 0)
                    self.assertLess(bounds[2], 28)
                    self.assertLess(bounds[3], 28)

    def test_embedded_sky_is_the_native_png(self):
        root = concept.ROOT / "art/backgrounds"
        indices = (root / "salt_iron_sky_320x180.pal8").read_bytes()
        self.assertEqual(len(indices), 320 * 180)
        self.assertLess(max(indices), 16)
        self.assertLessEqual(set(indices), {4, 5, 6, 11, 13})
        rgba = bytes(
            channel for index in indices for channel in (*concept.PALETTE[index], 255)
        )
        png = Image.open(root / "salt_iron_sky_320x180.png").convert("RGBA")
        self.assertEqual(png.size, (320, 180))
        self.assertEqual(rgba, png.tobytes())
        self.assertEqual(
            indices,
            (self.output / "backgrounds/salt_iron_sky_320x180.pal8").read_bytes(),
        )

    def test_title_reproduces_without_baked_in_lettering(self):
        shipping = Image.open(
            concept.ROOT / "art/title_background_320x180.png"
        ).convert("RGBA")
        rebuilt = Image.open(self.output / "title_background_320x180.png").convert(
            "RGBA"
        )
        self.assertEqual(shipping.size, (320, 180))
        self.assertEqual(shipping.tobytes(), rebuilt.tobytes())
        # The old board title occupied this patch. It must now be sky tones.
        patch = shipping.crop((8, 5, 80, 18))
        sky = {(*concept.PALETTE[i], 255) for i in (4, 5, 6)}
        self.assertLessEqual(set(patch.get_flattened_data()), sky)

    def test_water_tower_footing_stays_seated(self):
        for name in ("tower_intact", "tower_leaning"):
            image = self.products[name]
            feet = [x for x in range(17) if image.getpixel((x, 32))[3]]
            self.assertTrue(feet, name)
            self.assertGreaterEqual(min(feet), 4)
            self.assertLessEqual(max(feet), 12)
            self.assertFalse(any(image.getpixel((x, 33))[3] for x in range(17)))


if __name__ == "__main__":
    unittest.main()
