# SPDX-FileCopyrightText: 2026 Kevin Coughlin
#
# SPDX-License-Identifier: MIT
class_name DevicePowerBackend
extends RefCounted

## Backend contract used by the [code]DevicePower[/code] facade.
##
## Every read has an honest default meaning "unknown". Concrete backends are
## [DevicePowerAndroidBackend], [DevicePowerNullBackend] and
## [DevicePowerMockBackend]. Reads are polled, so they must be cheap.


## Short identifier used for diagnostics, e.g. [code]"android"[/code].
func get_backend_name() -> String:
	return "abstract"


## A [code]DevicePower.PowerSource[/code] value.
func get_power_source() -> int:
	return -1


## [code]true[/code] or [code]false[/code], or [code]null[/code] when unknown.
func is_power_saver_enabled() -> Variant:
	return null


## A [code]DevicePower.GameMode[/code] value (Android's own numbering).
func get_game_mode() -> int:
	return 0


## The display's highest refresh rate at its current resolution, or
## [code]0.0[/code] when unknown.
func get_peak_refresh_rate() -> float:
	return 0.0


## Asks the platform to run the game's surface at [param rate] frames per
## second ([code]0.0[/code] withdraws the request). Returns whether the
## request was made.
func vote_frame_rate(_rate: float) -> bool:
	return false
