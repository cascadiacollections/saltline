extends Node2D

# GameMode is a Linux desktop hint, not a rule or a prerequisite for play.
# Pausing releases it so an idle Steam Deck can return to its normal profile.
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
	_refresh_android_game_mode()


func _notification(what: int) -> void:
	if what == NOTIFICATION_APPLICATION_RESUMED:
		_refresh_android_game_mode()


func _refresh_android_game_mode() -> void:
	if OS.get_name() != "Android" or not is_instance_valid(game):
		return
	var mode := 1
	var runtime = Engine.get_singleton("AndroidRuntime")
	if runtime:
		var version = JavaClassWrapper.wrap("android.os.Build$VERSION")
		if JavaClassWrapper.get_exception() == null and version and version.SDK_INT >= 31:
			var context = runtime.getApplicationContext()
			if JavaClassWrapper.get_exception() == null and context:
				var manager = context.getSystemService("game")
				if JavaClassWrapper.get_exception() == null and manager:
					var queried = manager.getGameMode()
					if JavaClassWrapper.get_exception() == null:
						mode = queried
	game.set_android_game_mode(mode, DisplayServer.screen_get_refresh_rate())


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
