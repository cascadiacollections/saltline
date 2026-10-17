extends Node2D

# GameMode is a Linux desktop hint, not a rule or a prerequisite for play.
# Pausing releases it so an idle Steam Deck can return to its normal profile.
# DevicePower (addons/device_power) caps play by power source: the display's
# full rate on a charger, 60 on battery, and an Android game mode over both.
@onready var game: SaltLine = $Game
var _active := false
var _attempted := false
var _registered := false


func _ready() -> void:
	# Export smoke: make a missing native library visible even when the null
	# backend would otherwise let the game keep rendering.
	if OS.get_cmdline_user_args().has("--salt-require-portals") and not ClassDB.class_exists(&"XdgPortalNative"):
		push_error("Salt Line Linux export is missing the XDG Portals bridge")
		get_tree().quit(1)
		return
	DesktopServices.capabilities_changed.connect(_on_capabilities_changed)
	DevicePower.changed.connect(_apply_power_cap)
	_apply_power_cap()


func _apply_power_cap() -> void:
	if not is_instance_valid(game):
		return
	game.set_platform_fps_cap(DevicePower.recommended_fps_cap(DisplayServer.screen_get_refresh_rate()))
	print(
		"Device power: source=%d saver=%s game_mode=%d"
		% [DevicePower.get_power_source(), DevicePower.is_power_saver_enabled(), DevicePower.get_game_mode()]
	)


func _process(_delta: float) -> void:
	var active := game.is_active_play()
	if active != _active:
		_active = active
		_attempted = false
		if not active:
			_release_game_mode()
	if active and not _attempted and DesktopServices.has_capability(DesktopServices.Capability.GAME_MODE):
		_attempted = true
		_registered = DesktopServices.request_game_mode()


func _exit_tree() -> void:
	_release_game_mode()


func _on_capabilities_changed() -> void:
	var available := DesktopServices.has_capability(DesktopServices.Capability.GAME_MODE)
	if _registered and not available:
		_registered = false
	# A refresh must not register the same process twice: losing the result of
	# the second call would also prevent the first registration from being freed.
	_attempted = _registered


func _release_game_mode() -> void:
	if _registered:
		DesktopServices.release_game_mode()
		_registered = false
