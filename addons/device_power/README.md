# Device Power

A Godot 4 addon that tells a game where its power comes from and turns that
into a frame cap (and a display refresh request): full refresh rate on a charger, a lower cap on battery, and
the player's OS game mode respected over both.

It is game-agnostic and has no native code: on Android it reads the platform
through `JavaClassWrapper`; everywhere else it reports "unknown", which
imposes nothing.

## Install

Copy `addons/device_power` into a project and enable **Device Power** under
Project Settings → Plugins. That adds the `DevicePower` autoload.

```gdscript
func _ready() -> void:
    DevicePower.changed.connect(_apply_cap)
    _apply_cap()

func _apply_cap() -> void:
    Engine.max_fps = DevicePower.recommended_fps_cap(DisplayServer.screen_get_refresh_rate())
```

## Policy

`DevicePower.fps_cap()` is a pure static function, so a game can use it with
its own state, or replace it. `recommended_fps_cap()` applies it to the
current state with the `battery_fps` and `battery_mode_fps` exports.

| State | Cap |
|---|---|
| OS game mode Performance | the panel's peak refresh rate at the current resolution, read at startup (at most 240) |
| OS game mode Battery | `battery_mode_fps` (30) |
| Battery saver on | `battery_fps` (60), even on a charger |
| On battery | `battery_fps` (60) |
| On external power | 0 (no cap: V-Sync and the display decide) |
| Unknown (no bridge) | 0 |

"External power" means plugged in (`plugged` is non-zero), not that the
battery is gaining charge: a weak port or a charge limit still counts.

Menus and other idle screens are the game's business; apply your own lower
cap on top (the tighter of the two wins).

## Asking the display for the rate

With `vote_frame_rate` on (the default), the addon asks the platform for the
rate the game actually runs at: `Engine.max_fps` when set, the panel's peak
when it is 0. A game that drops to 30 fps on menus lets the panel follow
instead of holding it at its peak. The vote follows a change of
`Engine.max_fps` within a frame, and is resent after the app resumes.

## Android notes

- **Power source** is the sticky `ACTION_BATTERY_CHANGED` broadcast's
  `plugged` extra, not `BatteryManager.isCharging()`, which turns false on a
  full or charge-limited battery still on the charger. It follows
  `adb shell dumpsys battery unplug` / `dumpsys battery reset`, so the
  battery path can be tested on a device that has to stay on USB.
- **Game modes** need the app classified as a game
  (`android:appCategory="game"`) and Android 12+. Declaring
  `supportsPerformanceGameMode` / `supportsBatteryGameMode` in a
  `game_mode_config` lets the player pick them from the Game Dashboard.
- **The vote** is `Surface.setFrameRate()` (Android 12+) on Godot's render
  surface, seamless-only, so a change mid-game never blanks the screen for
  a display mode switch. Android 15 introduced a default frame rate for
  games (60 on Pixels: `ro.surface_flinger.game_default_frame_rate_override`);
  an app's own vote takes precedence over it. Verified on a Pixel 10 with
  Android 16: 120 fps on a charger while classified as a game, 60 on
  battery. The user's peak refresh rate setting still caps every app.
- Polling reads the power source and battery saver every `poll_interval`
  (2 s): two calls into system services. The game mode is read at start
  and on resume or focus, when the Game Dashboard can have changed it.

## Tests

```sh
godot --headless --path . --script res://addons/device_power/tests/test_device_power.gd
```

## License

MIT
