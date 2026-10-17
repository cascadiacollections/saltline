# SPDX-FileCopyrightText: 2026 Kevin Coughlin
#
# SPDX-License-Identifier: MIT
class_name DevicePowerAndroidBackend
extends DevicePowerBackend

## Reads Android's power state through [JavaClassWrapper], with no Java code
## of its own to build or ship.
##
## [b]Power source[/b] comes from the sticky [code]ACTION_BATTERY_CHANGED[/code]
## broadcast ([code]registerReceiver(null, filter)[/code] returns the last one
## without registering anything). Its [code]plugged[/code] extra is the right
## question: [code]BatteryManager.isCharging()[/code] turns false on a full or
## charge-limited battery that is still on the charger. It also follows
## [code]adb shell dumpsys battery unplug[/code], which makes the battery path
## testable on a device that has to stay on USB.
##
## [b]Game mode[/b] is [code]GameManager.getGameMode()[/code] (API 31+). It
## reports [code]UNSUPPORTED[/code] unless the app is classified as a game
## ([code]android:appCategory="game"[/code]).
##
## [b]Frame rate vote[/b] is [code]Surface.setFrameRate()[/code] (API 31+) on
## Godot's render surface. It is how a game asks for more than Android's game
## default frame rate (60 on Pixels) while staying a game, and how it lets the
## panel drop to a lower rate on battery. The user's peak refresh setting
## (Smooth Display) and battery saver still clamp it.
##
## Every Java call is checked with [method JavaClassWrapper.get_exception]; a
## failure reads as "unknown", never as a guess.

const _BATTERY_CHANGED := "android.intent.action.BATTERY_CHANGED"
const _FRAME_RATE_COMPATIBILITY_DEFAULT := 0
const _CHANGE_FRAME_RATE_ALWAYS := 1

var _runtime: Object
var _sdk: int = 0
var _filter: Object = null


func _init(runtime: Object) -> void:
	_runtime = runtime
	var version = JavaClassWrapper.wrap("android.os.Build$VERSION")
	if _ok() and version:
		_sdk = version.SDK_INT


func get_backend_name() -> String:
	return "android"


func get_power_source() -> int:
	var context = _context()
	if context == null:
		return -1
	if _filter == null:
		var intent_filter = JavaClassWrapper.wrap("android.content.IntentFilter")
		if not _ok() or not intent_filter:
			return -1
		_filter = intent_filter.IntentFilter(_BATTERY_CHANGED)
		if not _ok():
			_filter = null
			return -1
	var status = context.registerReceiver(null, _filter)
	if not _ok() or status == null:
		return -1
	var plugged: int = status.getIntExtra("plugged", -1)
	if not _ok() or plugged < 0:
		return -1
	# 0 is on battery; AC, USB, wireless and dock are all bit flags above it.
	return 1 if plugged > 0 else 0


func is_power_saver_enabled() -> Variant:
	var context = _context()
	if context == null:
		return null
	var power = context.getSystemService("power")
	if not _ok() or not power:
		return null
	var saver = power.isPowerSaveMode()
	return saver if _ok() else null


func get_game_mode() -> int:
	if _sdk < 31:
		return 0
	var context = _context()
	if context == null:
		return 0
	var manager = context.getSystemService("game")
	if not _ok() or not manager:
		return 0
	var mode = manager.getGameMode()
	return mode if _ok() else 0


func get_peak_refresh_rate() -> float:
	var activity = _runtime.getActivity()
	if not _ok() or not activity:
		return 0.0
	var display = activity.getDisplay() if _sdk >= 30 else null
	if not _ok() or not display:
		return 0.0
	var current = display.getMode()
	var modes = display.getSupportedModes()
	if not _ok() or not current or modes == null:
		return 0.0
	var width: int = current.getPhysicalWidth()
	var height: int = current.getPhysicalHeight()
	var peak: float = current.getRefreshRate()
	for mode in modes:
		if mode.getPhysicalWidth() == width and mode.getPhysicalHeight() == height:
			peak = maxf(peak, mode.getRefreshRate())
	return peak if _ok() else 0.0


func vote_frame_rate(rate: float) -> bool:
	if _sdk < 31:
		return false
	var surface = _surface()
	if surface == null:
		return false
	surface.setFrameRate(rate, _FRAME_RATE_COMPATIBILITY_DEFAULT, _CHANGE_FRAME_RATE_ALWAYS)
	return _ok()


## Godot's render surface, or null before it exists or after it is gone.
func _surface() -> Object:
	var context = _context()
	if context == null:
		return null
	var godot_class = JavaClassWrapper.wrap("org.godotengine.godot.Godot")
	if not _ok() or not godot_class:
		return null
	var godot = godot_class.getInstance(context)
	if not _ok() or not godot:
		return null
	var render_view = godot.getRenderView()
	if not _ok() or not render_view:
		return null
	var view = render_view.getView()
	if not _ok() or not view:
		return null
	var holder = view.getHolder()
	if not _ok() or not holder:
		return null
	var surface = holder.getSurface()
	if not _ok() or not surface or not surface.isValid():
		return null
	return surface


func _context() -> Object:
	var context = _runtime.getApplicationContext()
	return context if _ok() and context else null


func _ok() -> bool:
	return JavaClassWrapper.get_exception() == null
