# Vendored: godot-device-power

This directory is a copy of `addons/device_power` from
<https://github.com/KevinTCoughlin/godot-device-power> at commit
`40f357f2322e55e76ebf2df380a54405e54e9f29`. Change it there first, then copy it here again and update this file.

The AARs in `bin/` were built from `android-plugin/` at that commit with
`scripts/build_android.sh` (JDK 21, AGP 8.6.1, Kotlin 2.1.21):

| File | sha256 |
|---|---|
| `bin/debug/device_power-debug.aar` | `3aadb6099484abc43af61b9f865d7b159440e2f2027c09cd660a92541a59c21a` |
| `bin/release/device_power-release.aar` | `95264e16e992ea44f1de84c1a10f8aa2a4d593aeabef78386e10990b110c3605` |

The addon's tests (GDScript suite, JVM unit tests, Android lint) run in the
upstream repository's CI, not here.
