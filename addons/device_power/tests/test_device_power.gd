# SPDX-FileCopyrightText: 2026 Kevin Coughlin
#
# SPDX-License-Identifier: MIT
extends SceneTree

## Headless checks for the policy and the polling facade:
##   godot --headless --script res://addons/device_power/tests/test_device_power.gd
## Exits non-zero on any failure. A script error does not change Godot's
## exit code, so each section must also reach its end to count as passed.

const DevicePowerScript := preload("res://addons/device_power/device_power.gd")
const S := DevicePowerScript.PowerSource
const M := DevicePowerScript.GameMode

var _failures := 0
var _finished: Array[String] = []


func _initialize() -> void:
	# A parse error in the addon fails the preload above, and Godot then exits 1.
	_policy()
	_facade()
	for section in ["policy", "facade"]:
		if section not in _finished:
			_fail("%s stopped early (script error above)" % section)
	if _failures == 0:
		print("device_power: all tests passed")
	quit(1 if _failures else 0)


func _policy() -> void:
	var cap := DevicePowerScript.fps_cap
	# Standard (or no game mode at all): external power is uncapped, battery is held.
	for mode in [M.UNSUPPORTED, M.STANDARD, M.CUSTOM]:
		_eq(cap.call(S.EXTERNAL, false, mode, 120.0), 0, "external, mode %d" % mode)
		_eq(cap.call(S.BATTERY, false, mode, 120.0), 60, "battery, mode %d" % mode)
		_eq(cap.call(S.UNKNOWN, null, mode, 120.0), 0, "unknown imposes nothing, mode %d" % mode)
	# Battery saver counts as battery, even on a charger.
	_eq(cap.call(S.EXTERNAL, true, M.STANDARD, 120.0), 60, "saver on a charger")
	_eq(cap.call(S.UNKNOWN, true, M.STANDARD, 120.0), 60, "saver, source unknown")
	# A saver value that is not a bool (an int, as Java booleans can arrive) is
	# unknown, not a script error; release exports would turn that error into null.
	_eq(cap.call(S.EXTERNAL, 0, M.STANDARD, 120.0), 0, "int saver is unknown")
	_eq(cap.call(S.EXTERNAL, 1, M.STANDARD, 120.0), 0, "int saver is unknown even if truthy")
	# An explicit OS game mode is the player's choice and wins over the power source.
	_eq(cap.call(S.BATTERY, false, M.PERFORMANCE, 120.0), 120, "performance on battery")
	_eq(cap.call(S.EXTERNAL, true, M.PERFORMANCE, 90.0), 90, "performance beats saver")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, 59.94), 60, "refresh rounds")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, -1.0), 60, "no refresh rate")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, NAN), 60, "NaN refresh rate")
	_eq(cap.call(S.EXTERNAL, false, M.BATTERY, 120.0), 30, "battery mode on a charger")
	# The game's own numbers are used.
	_eq(cap.call(S.BATTERY, false, M.STANDARD, 120.0, 45, 20), 45, "custom battery_fps")
	_eq(cap.call(S.BATTERY, false, M.BATTERY, 120.0, 45, 20), 20, "custom battery_mode_fps")
	var paced := DevicePowerScript.paced_cap
	var pixel := PackedFloat32Array([60.000004, 120.00001])
	var tablet := PackedFloat32Array([90.0])
	_eq(paced.call(60, pixel), 60, "60 on a 60/120 Hz panel")
	_eq(paced.call(30, pixel), 30, "30 on a 60/120 Hz panel")
	_eq(paced.call(60, tablet), 90, "60 on a fixed 90 Hz panel rounds up to 90")
	_eq(paced.call(30, tablet), 30, "30 on a fixed 90 Hz panel")
	_eq(paced.call(90, tablet), 90, "the panel's own rate")
	_eq(paced.call(60, PackedFloat32Array([144.0])), 72, "60 on 144 Hz rounds up to 72")
	_eq(paced.call(45, tablet), 45, "45 on 90 Hz")
	_eq(paced.call(100, tablet), 100, "a cap above every rate is left alone")
	_eq(paced.call(60, PackedFloat32Array([90.0, 60.0])), 60, "a panel that can switch to 60")
	_eq(paced.call(60, PackedFloat32Array([59.94])), 60, "59.94 Hz counts as 60")
	_eq(paced.call(0, tablet), 0, "no cap stays no cap")
	_eq(paced.call(60, PackedFloat32Array()), 60, "unknown rates leave the cap")
	_eq(paced.call(60, PackedFloat32Array([NAN, -1.0])), 60, "garbage rates leave the cap")
	var vote := DevicePowerScript.frame_rate_vote
	_eq(vote.call(0, 120.0), 120.0, "uncapped votes the peak")
	_eq(vote.call(60, 120.0), 60.0, "a cap votes the cap")
	_eq(vote.call(0, 0.0), 0.0, "nothing known, no request")
	_finished.append("policy")


func _facade() -> void:
	var saved_max_fps := Engine.max_fps
	Engine.max_fps = 0
	var power: Node = DevicePowerScript.new()
	var mock := DevicePowerMockBackend.new()
	mock.refresh_rates = PackedFloat32Array([60.0, 120.0])
	var seen := [0]
	power.changed.connect(func() -> void: seen[0] += 1)
	power.set_backend(mock)
	_eq(seen[0], 1, "set_backend emits once")
	_eq(power.get_backend_name(), "mock", "backend name")
	_eq(typeof(power.recommended_fps_cap(120.0)), TYPE_INT, "the cap is an int")
	_eq(power.recommended_fps_cap(120.0), 0, "unknown state, no cap")
	_eq(mock.votes, [120.0] as Array[float], "uncapped game votes the peak at once")

	power.refresh()
	_eq(seen[0], 1, "an unchanged poll is silent")
	_eq(mock.votes.size(), 1, "an unchanged vote is not resent")

	mock.power_source = S.BATTERY
	power.refresh()
	_eq(seen[0], 2, "unplugging emits")
	_eq(power.get_power_source(), S.BATTERY, "reads the source")
	_eq(power.recommended_fps_cap(120.0), 60, "battery cap")

	# The vote follows the cap the game applies, not the recommendation.
	Engine.max_fps = 60
	power._process(0.0)
	_eq(mock.votes.back(), 60.0, "votes the game's cap within a frame")
	Engine.max_fps = 30
	power._process(0.0)
	_eq(mock.votes.back(), 30.0, "a menu cap is voted too")

	mock.power_source = S.EXTERNAL
	power.refresh()
	_eq(seen[0], 3, "plugging in emits")
	_eq(power.recommended_fps_cap(120.0), 0, "external, uncapped")
	Engine.max_fps = 0
	power._process(0.0)
	_eq(mock.votes.back(), 120.0, "uncapped votes the peak again")
	var before := mock.votes.size()
	power._notification(Node.NOTIFICATION_APPLICATION_RESUMED)
	_eq(mock.votes.size(), before + 1, "resuming resends the vote")

	# A failed vote (no surface yet) is retried on polls, not every frame.
	mock.vote_ok = false
	Engine.max_fps = 60
	power._process(0.0)
	before = mock.votes.size()
	power._process(0.0)
	power._process(0.0)
	_eq(mock.votes.size(), before, "a failed vote is not retried every frame")
	mock.vote_ok = true
	power.refresh()
	_eq(mock.votes.size(), before + 1, "a poll retries it")
	power.refresh()
	_eq(mock.votes.size(), before + 1, "and stops once it is made")
	Engine.max_fps = 0

	# Game mode is read on full reads (start, resume, focus), not every poll.
	mock.game_mode = M.BATTERY
	power.refresh()
	_eq(power.get_game_mode(), M.UNSUPPORTED, "a poll does not read the game mode")
	power._notification(Node.NOTIFICATION_APPLICATION_FOCUS_IN)
	_eq(power.get_game_mode(), M.BATTERY, "focus reads it")
	_eq(power.recommended_fps_cap(120.0), 30, "battery mode")
	mock.game_mode = M.PERFORMANCE
	power._notification(Node.NOTIFICATION_APPLICATION_RESUMED)
	_eq(power.recommended_fps_cap(0.0), 120, "performance uses the peak")
	var emitted: int = seen[0]

	# A saver that is not a bool is unknown, with no script error.
	mock.power_saver = 1
	power.refresh()
	_eq(power.is_power_saver_enabled(), null, "an int saver reads as unknown")
	_eq(seen[0], emitted, "and changes nothing")
	mock.power_saver = true
	power.refresh()
	_eq(power.is_power_saver_enabled(), true, "reads the saver")
	_eq(seen[0], emitted + 1, "a saver change emits")
	power.free()

	# A broken peak read leaves the peak unknown; nothing else breaks.
	var broken: Node = DevicePowerScript.new()
	var bad := DevicePowerMockBackend.new()
	bad.refresh_rates = PackedFloat32Array([NAN, -5.0])
	broken.set_backend(bad)
	_eq(broken.get_peak_refresh_rate(), 0.0, "garbage rates are unknown")
	_eq(broken.get_refresh_rates().size(), 0, "and are dropped")
	_eq(bad.votes, [0.0] as Array[float], "nothing known: the vote is withdrawn")
	broken.free()

	var plain: Node = DevicePowerScript.new()
	plain.set_backend(DevicePowerNullBackend.new())
	_eq(plain.get_power_source(), S.UNKNOWN, "null backend is unknown")
	_eq(plain.get_game_mode(), M.UNSUPPORTED, "null backend has no game mode")
	_eq(plain.recommended_fps_cap(120.0), 0, "null backend imposes nothing")
	_eq(plain.get_peak_refresh_rate(), 0.0, "null backend knows no peak")

	# The recommendation is paced to the panel: a fixed 90 Hz tablet on battery
	# keeps 90 (60 cannot be paced there), Battery game mode still gets 30.
	var tab: Node = DevicePowerScript.new()
	var tab_mock := DevicePowerMockBackend.new()
	tab_mock.refresh_rates = PackedFloat32Array([90.0])
	tab_mock.power_source = S.BATTERY
	tab.set_backend(tab_mock)
	_eq(tab.recommended_fps_cap(90.0), 90, "battery on a fixed 90 Hz panel")
	tab_mock.game_mode = M.BATTERY
	tab._notification(Node.NOTIFICATION_APPLICATION_FOCUS_IN)
	_eq(tab.recommended_fps_cap(90.0), 30, "battery mode on it")
	tab_mock.game_mode = M.STANDARD
	tab._notification(Node.NOTIFICATION_APPLICATION_FOCUS_IN)
	tab_mock.power_source = S.EXTERNAL
	tab.refresh()
	_eq(tab.recommended_fps_cap(90.0), 0, "external on it, uncapped")
	tab.free()
	plain.free()
	Engine.max_fps = saved_max_fps
	_finished.append("facade")


func _eq(actual: Variant, expected: Variant, what: String) -> void:
	if typeof(actual) != typeof(expected) or actual != expected:
		_fail("%s: expected %s, got %s" % [what, expected, actual])


func _fail(message: String) -> void:
	_failures += 1
	push_error("device_power: " + message)
