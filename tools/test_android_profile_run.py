"""Tests for tools/android_profile_run.py's reading of the profiling log."""

import unittest

import android_profile_run as run

LOG = [
    "10-15 00:37:50.832 I/godot   (29838): Android Game Mode: mode=1, play_cap=0, effective_cap=0",
    "10-15 00:37:50.840 I/godot   (29838): salt-profile: A/B capture starting; warmup=2s",
    "10-15 00:38:24.851 I/godot   (29838): salt-profile: done wall=34.00s seed=20261004",
    "10-15 00:38:24.851 I/godot   (29838): updated: frames=1433 rendered=1433 texture_updates=1433",
    "10-15 00:38:24.851 I/godot   (29838): updated process: n=1433 mean=2826.0us p50=2275.4us",
    "10-15 00:38:24.851 I/godot   (29838): frozen software_render: n=1443 mean=1311.3us",
    "10-15 00:38:24.852 I/godot   (29838): salt-profile: gpu check scene=run starting",
    "10-15 00:38:40.000 I/godot   (29838): salt-profile: gpu check scene=run PASS frames=600",
    "10-15 00:39:40.000 I/godot   (29838): salt-profile: gpu check scene=versus FAIL frames=2400",
]


class ReadLog(unittest.TestCase):
    def test_the_summary_is_the_done_line_and_the_timings_after_it(self):
        self.assertEqual(
            run.summary(LOG),
            [
                "salt-profile: done wall=34.00s seed=20261004",
                "updated: frames=1433 rendered=1433 texture_updates=1433",
                "updated process: n=1433 mean=2826.0us p50=2275.4us",
                "frozen software_render: n=1443 mean=1311.3us",
            ],
        )

    def test_no_summary_until_the_capture_is_done(self):
        self.assertEqual(run.summary(LOG[:2]), [])

    def test_verdicts_name_each_scene(self):
        self.assertEqual(run.verdicts(LOG), [("run", True), ("versus", False)])

    def test_merging_dumps_keeps_log_order_within_a_millisecond(self):
        seen = {}
        run.merge(seen, "\n".join(LOG[:3]))
        # The next dump repeats the buffer and adds lines logged in the
        # same millisecond as `done`, which sort before it by text.
        run.merge(seen, "\n".join(LOG[:6]))
        self.assertEqual(list(seen), LOG[:6])
        self.assertEqual(len(run.summary(list(seen))), 4)

    def test_a_lock_screen_or_shade_has_the_focus(self):
        self.assertTrue(run.focus_is_elsewhere("  mCurrentFocus=Window{9aaddfd u0 NotificationShade}"))
        self.assertFalse(
            run.focus_is_elsewhere(
                "  mCurrentFocus=Window{21995e3 u0 com.cascadiacollections.saltline.profile/com.godot.game.GodotAppLauncher}"
            )
        )


if __name__ == "__main__":
    unittest.main()
