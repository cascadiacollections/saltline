# SPDX-FileCopyrightText: 2026 Kevin Coughlin
#
# SPDX-License-Identifier: MIT
extends Node

## What the device's power situation is, and how fast a game should draw
## because of it.
##
## Installed as the [code]DevicePower[/code] autoload by the
## [code]device_power[/code] plugin. It polls a backend chosen at startup
## ([DevicePowerAndroidBackend] on Android, [DevicePowerNullBackend]
## elsewhere, or one injected by tests) and emits [signal changed] when
## anything it reports changes.
##
## [codeblock]
## func _ready() -> void:
##     DevicePower.changed.connect(_apply_cap)
##     _apply_cap()
##
## func _apply_cap() -> void:
##     Engine.max_fps = DevicePower.recommended_fps_cap(DisplayServer.screen_get_refresh_rate())
## [/codeblock]
##
## [b]Android's own cap.[/b] An app classified as a game can still be held
## below the panel's refresh rate by the platform (Android's game default
## frame rate, 60 on Pixels). That cap outranks anything the app asks for,
## and is the price of keeping OS game modes; this addon only ever lowers a
## game's rate, it cannot raise it past the platform's.

## Emitted after a poll finds a different power source, battery saver state
## or game mode, and once after a backend is set.
signal changed

## Where the power comes from.
enum PowerSource {
	UNKNOWN = -1,  ## No answer: no bridge on this platform, or the read failed.
	BATTERY = 0,  ## Running on the battery.
	EXTERNAL = 1,  ## On a charger, USB, a dock or wireless charging, whether or not it is charging right now.
}

## Android [code]GameManager[/code] modes, in Android's numbering.
enum GameMode {
	UNSUPPORTED = 0,  ## Not a game, before Android 12, or not Android.
	STANDARD = 1,  ## The default: the game's own policy applies.
	PERFORMANCE = 2,  ## The player asked for the smoothest play.
	BATTERY = 3,  ## The player asked to save battery.
	CUSTOM = 4,  ## OEM or adb intervention settings; treated like Standard.
}

## Seconds between polls while the app is in the foreground. A charger
## plugged in mid-game is picked up within this long; resuming the app
## polls at once.
@export var poll_interval: float = 2.0

var _backend: DevicePowerBackend = null
var _power_source: int = PowerSource.UNKNOWN
var _power_saver: Variant = null
var _game_mode: int = GameMode.UNSUPPORTED
var _clock: float = 0.0


func _ready() -> void:
	process_mode = Node.PROCESS_MODE_ALWAYS
	if _backend == null:
		set_backend(_create_default_backend())


func _process(delta: float) -> void:
	_clock += delta
	if _clock >= poll_interval:
		refresh()


func _notification(what: int) -> void:
	if what == NOTIFICATION_APPLICATION_RESUMED or what == NOTIFICATION_APPLICATION_FOCUS_IN:
		refresh()


## Replaces the backend and reads it at once. Tests inject a
## [DevicePowerMockBackend] here.
func set_backend(backend: DevicePowerBackend) -> void:
	assert(backend != null, "DevicePower.set_backend() requires a backend; use DevicePowerNullBackend.new().")
	_backend = backend
	_read()
	changed.emit()


func get_backend_name() -> String:
	return _backend.get_backend_name() if _backend else "none"


## Reads the backend now; emits [signal changed] if anything differs.
func refresh() -> void:
	if _backend and _read():
		changed.emit()


## A [enum PowerSource] value, as of the last poll.
func get_power_source() -> PowerSource:
	return _power_source as PowerSource


## Battery saver, as of the last poll: [code]true[/code], [code]false[/code],
## or [code]null[/code] when unknown.
func is_power_saver_enabled() -> Variant:
	return _power_saver


## A [enum GameMode] value, as of the last poll.
func get_game_mode() -> GameMode:
	return _game_mode as GameMode


## [method fps_cap] for the current state.
func recommended_fps_cap(refresh_rate: float, battery_fps: int = 60, saver_fps: int = 30) -> int:
	return fps_cap(_power_source, _power_saver, _game_mode, refresh_rate, battery_fps, saver_fps)


## The frame cap for a power state, for [member Engine.max_fps]: 0 means
## "no cap of ours", leaving the display's refresh rate (with V-Sync) as the
## limit.
##
## [br]- An explicit OS game mode is the player's choice and wins:
##   Performance runs at the panel's refresh rate, Battery at [param saver_fps].
## [br]- Otherwise external power is uncapped and the battery is held to
##   [param battery_fps]; battery saver counts as battery even on a charger.
## [br]- Unknown imposes nothing: a platform without a bridge keeps whatever
##   the game would do anyway.
static func fps_cap(
	power_source: int,
	power_saver: Variant,
	game_mode: int,
	refresh_rate: float,
	battery_fps: int = 60,
	saver_fps: int = 30,
) -> int:
	match game_mode:
		GameMode.PERFORMANCE:
			if is_finite(refresh_rate) and refresh_rate > 0.0:
				return clampi(roundi(refresh_rate), 1, 240)
			return battery_fps
		GameMode.BATTERY:
			return saver_fps
	if power_saver == true or power_source == PowerSource.BATTERY:
		return battery_fps
	return 0


func _read() -> bool:
	_clock = 0.0
	var source := _backend.get_power_source()
	var saver: Variant = _backend.is_power_saver_enabled()
	var mode := _backend.get_game_mode()
	var differs: bool = source != _power_source or saver != _power_saver or mode != _game_mode
	_power_source = source
	_power_saver = saver
	_game_mode = mode
	return differs


func _create_default_backend() -> DevicePowerBackend:
	if OS.get_name() == "Android" and Engine.has_singleton("AndroidRuntime"):
		return DevicePowerAndroidBackend.new(Engine.get_singleton("AndroidRuntime"))
	return DevicePowerNullBackend.new()
