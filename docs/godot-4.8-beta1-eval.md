# Godot 4.8 beta 1 evaluation (WIP)

Eval branch only. Production stays on 4.7.2 (owner decision: no engine switch).

## Raw measurements so far (2026-10-19)

| Build | APK bytes | libgodot_android.so | Installed (du, KB) |
|---|---|---|---|
| 4.7.2 official release | 87,500,499 | 71,114,944 | 85,543 |
| 4.7.2 stripped (lock 96290a6e) | 39,577,299 | 23,186,040 | 38,687 |
| 4.8 beta1 official release | 81,913,301 | 65,524,792 | 80,075 |

- 4.8 export needed export/android/java_sdk_path in editor_settings-4.8.tres (empty value fails; JAVA_HOME only seeds a missing key).
- Device (Pixel 10): 4.8 launches, DevicePowerAndroid registers, backend=android_plugin, frame rate vote 120 ok; Daily Run, angle/power taps and FIRE work.
