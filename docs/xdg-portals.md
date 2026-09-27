# Linux desktop services

Salt Line includes the MIT-licensed `godot-xdg-portals` GDScript facade from
[`KevinTCoughlin/godot-xdg-portals`](https://github.com/KevinTCoughlin/godot-xdg-portals),
revision `b6405883ec05d32f214a5cdc79cbb5dc83860539`. Its license and third-party
notices are beside the copied addon. The Linux x86_64 `.gdextension` descriptor
is generated from `tools/xdg_portals_linux.gdextension.in` during the native
build. Godot 4.7.2 reports an error when a descriptor has no library for the
current OS, so other platforms carry only the addon's GDScript facade and
select its null backend without loading a descriptor.

`tools/build_xdg_portals.sh` fetches that exact revision and builds the Linux
editor, debug, and release libraries. CI runs it before Godot import and export.
On a Linux development machine, install CMake, pkg-config, and GLib/GIO headers,
then run `bash tools/build_xdg_portals.sh` before starting Godot. The native
libraries and descriptor are generated and ignored by Git.

`godot/platform_services.gd` requests GameMode while a run is actively playing
and releases it on pause, game over, or exit. It checks capability and records
whether registration succeeded, so the game plays unchanged without a portal
or GameMode implementation. The service only changes the host's performance
hint; it does not affect the deterministic simulation or saved run.

When updating the addon, copy its `addons/xdg_portals` GDScript files, license,
and notices, retain the Linux-only descriptor template, then update the revision
in this file and `tools/build_xdg_portals.sh`. Run the upstream 4.7.2 CI suite
and Salt Line's Linux exported-build smoke test.
