# Device Power

A Godot 4 addon that tells a game where its power comes from and turns that
into a frame cap: full refresh rate on a charger, a lower cap on battery, and
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
its own state, or replace it.

| State | Cap |
|---|---|
| OS game mode Performance | the display's refresh rate |
| OS game mode Battery | `saver_fps` (30) |
| Battery saver on | `battery_fps` (60), even on a charger |
| On battery | `battery_fps` (60) |
| On external power | 0 (no cap: V-Sync and the display decide) |
| Unknown (no bridge) | 0 |

Menus and other idle screens are the game's business; apply your own lower
cap on top (the tighter of the two wins).

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
- **Android's own cap.** A game can be held below the panel's refresh rate
  by the platform's game default frame rate (60 on Pixels), and the user's
  peak refresh setting (Smooth Display) caps every app. Both outrank anything
  an app asks for. This addon only lowers a game's rate.
- Polling costs a few binder calls every `poll_interval` (2 s) and on resume.

## Tests

```sh
godot --headless --script res://addons/device_power/tests/test_device_power.gd
```

## License

MIT
