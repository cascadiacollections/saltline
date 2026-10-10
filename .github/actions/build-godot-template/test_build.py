import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load('portable', HERE / 'build.py')
saltline = load('saltline', ROOT / 'tools/godot_template.py')


class BuilderTests(unittest.TestCase):
    def env(self, platform='android', arch='arm64'):
        return dict(ENGINE_TAG=saltline.GODOT_TAG, ENGINE_COMMIT=saltline.GODOT_COMMIT,
                    BUILD_PROFILE=str(saltline.PROFILE), BUILD_PLATFORM=platform,
                    BUILD_ARCH=arch, SCONS_VERSION=saltline.SCONS_VERSION,
                    BUILD_FLAGS=json.dumps(['swappy=no'] if platform == 'android' else ['speechd=no', 'fontconfig=no']),
                    BUILD_TOOLCHAIN='test-compiler')

    def test_dogfood_matches_existing_build_recipe_and_names(self):
        for platform, arch in [('android', 'arm64'), ('linux', 'x86_64')]:
            with self.subTest(platform=platform):
                config = builder.configuration(self.env(platform, arch))
                self.assertEqual(dict(arg.split('=', 1) for arg in builder.scons_args(config)),
                                 dict(arg.split('=', 1) for arg in saltline.scons_args(platform, arch)))
                self.assertEqual(builder.output_name(config), saltline.output_name(platform, arch))
                self.assertEqual(config['scons'], saltline.SCONS_VERSION)

    def test_published_release_key_tracks_portable_recipe_changes(self):
        baseline = saltline.key()
        for recipe in saltline.PORTABLE_RECIPE:
            with self.subTest(recipe=recipe.name), tempfile.TemporaryDirectory() as directory:
                changed = Path(directory) / recipe.name
                changed.write_bytes(recipe.read_bytes() + b"\n# recipe change\n")
                paths = tuple(changed if path == recipe else path for path in saltline.PORTABLE_RECIPE)
                with patch.object(saltline, 'PORTABLE_RECIPE', paths):
                    self.assertNotEqual(baseline, saltline.key())

    def test_configuration_rejects_unpinned_and_override_inputs(self):
        for key, value in [('ENGINE_COMMIT', 'main'), ('ENGINE_TAG', '--help'),
                           ('SCONS_VERSION', 'latest'), ('BUILD_ARCH', '../arm64'),
                           ('BUILD_FLAGS', '["target=editor"]'), ('BUILD_FLAGS', '["x=$(evil)"]')]:
            with self.subTest(key=key, value=value):
                env = self.env(); env[key] = value
                with self.assertRaises(ValueError):
                    builder.configuration(env)

    def test_malformed_profiles_fail_with_actionable_errors(self):
        for profile in [[], {"type": "build_profile", "disabled_build_options": []},
                        {"type": "build_profile", "disabled_classes": "Node"}]:
            with self.subTest(profile=profile), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'profile.json'
                path.write_text(json.dumps(profile))
                env = self.env(); env['BUILD_PROFILE'] = str(path)
                with self.assertRaises(ValueError):
                    builder.configuration(env)

    def test_content_key_covers_build_inputs_and_toolchain(self):
        env = self.env(); original = builder.configuration(env)
        key = builder.content_key(original, env)
        for field, value in [('commit', '0' * 40), ('arch', 'x86_64'), ('flags', ['swappy=yes']), ('profile', {})]:
            changed = dict(original); changed[field] = value
            self.assertNotEqual(key, builder.content_key(changed, env))
        for field in ['BUILD_TOOLCHAIN', 'ImageVersion', 'RUNNER_ARCH']:
            changed = dict(env); changed[field] = 'different'
            self.assertNotEqual(key, builder.content_key(original, changed))
        self.assertEqual(key, builder.content_key(json.loads(json.dumps(original)), env))

    def test_moved_source_tag_stops_before_build(self):
        config = builder.configuration(self.env())
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(builder, 'run') as run, patch.object(builder.subprocess, 'check_output', return_value='0' * 40):
                with self.assertRaisesRegex(ValueError, 'tag resolves'):
                    builder.build(config, Path(directory) / 'source', Path(directory) / 'output')
                self.assertEqual(run.call_count, 1)  # clone only; never SCons

    def test_report_checksums_and_preserves_executable(self):
        config = builder.configuration(self.env('linux', 'x86_64'))
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            binary = out / builder.output_name(config)
            binary.write_bytes(b'engine'); binary.chmod(0o755)
            with patch.dict(os.environ, {}, clear=True):
                builder.report(config, out)
            report = json.loads((out / 'build-report.json').read_text())
            self.assertEqual(report['bytes'], 6)
            self.assertEqual(report['sha256'], builder.sha256(binary))
            self.assertEqual((out / 'SHA256SUMS').read_text(), f"{report['sha256']}  {binary.name}\n")
            self.assertTrue(binary.stat().st_mode & 0o111)

    def test_unknown_profile_option_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory); (source / 'SConstruct').write_text('"disable_3d"')
            builder.check_profile(source, {'disabled_build_options': {'disable_3d': True}})
            with self.assertRaisesRegex(ValueError, 'unknown'):
                builder.check_profile(source, {'disabled_build_options': {'module_missing_enabled': False}})


if __name__ == '__main__':
    unittest.main()
