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
	# An explicit OS game mode is the player's choice and wins over the power source.
	_eq(cap.call(S.BATTERY, false, M.PERFORMANCE, 120.0), 120, "performance on battery")
	_eq(cap.call(S.EXTERNAL, true, M.PERFORMANCE, 90.0), 90, "performance beats saver")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, 59.94), 60, "refresh rounds")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, -1.0), 60, "no refresh rate")
	_eq(cap.call(S.EXTERNAL, false, M.PERFORMANCE, NAN), 60, "NaN refresh rate")
	_eq(cap.call(S.EXTERNAL, false, M.BATTERY, 120.0), 30, "battery mode on a charger")
	# The game's own numbers are used.
	_eq(cap.call(S.BATTERY, false, M.STANDARD, 120.0, 45, 20), 45, "custom battery_fps")
	_eq(cap.call(S.BATTERY, false, M.BATTERY, 120.0, 45, 20), 20, "custom saver_fps")
	_finished.append("policy")


func _facade() -> void:
	var power: Node = DevicePowerScript.new()
	var mock := DevicePowerMockBackend.new()
	var seen := [0]
	power.changed.connect(func() -> void: seen[0] += 1)
	power.set_backend(mock)
	_eq(seen[0], 1, "set_backend emits once")
	_eq(power.get_backend_name(), "mock", "backend name")
	_eq(power.recommended_fps_cap(120.0), 0, "unknown state, no cap")

	power.refresh()
	_eq(seen[0], 1, "an unchanged poll is silent")

	mock.power_source = S.BATTERY
	power.refresh()
	_eq(seen[0], 2, "unplugging emits")
	_eq(power.get_power_source(), S.BATTERY, "reads the source")
	_eq(power.recommended_fps_cap(120.0), 60, "battery cap")

	mock.power_source = S.EXTERNAL
	power.refresh()
	_eq(seen[0], 3, "plugging in emits")
	_eq(power.recommended_fps_cap(120.0), 0, "external, uncapped")

	mock.game_mode = M.BATTERY
	power.refresh()
	_eq(seen[0], 4, "a game mode change emits")
	_eq(power.recommended_fps_cap(120.0), 30, "battery mode")

	mock.power_saver = true
	power.refresh()
	_eq(seen[0], 5, "a saver change emits")
	_eq(power.is_power_saver_enabled(), true, "reads the saver")
	power.free()

	var plain: Node = DevicePowerScript.new()
	plain.set_backend(DevicePowerNullBackend.new())
	_eq(plain.get_power_source(), S.UNKNOWN, "null backend is unknown")
	_eq(plain.get_game_mode(), M.UNSUPPORTED, "null backend has no game mode")
	_eq(plain.recommended_fps_cap(120.0), 0, "null backend imposes nothing")
	plain.free()
	_finished.append("facade")


func _eq(actual: Variant, expected: Variant, what: String) -> void:
	if typeof(actual) != typeof(expected) or actual != expected:
		_fail("%s: expected %s, got %s" % [what, expected, actual])


func _fail(message: String) -> void:
	_failures += 1
	push_error("device_power: " + message)
