# Build optimized Godot templates

A composite GitHub Action with a dependency-free Python driver for Godot's
size-optimized native release templates. Salt Line uses it in
`godot-templates.yml` for Android releases and `engine-validation.yml` for
Linux replay parity. Game exports, APK packaging, asset validation, licenses,
release publishing and attestation policies remain with the caller.

The first version supports Linux runners building Android (`arm64`, `arm32`,
`x86_64`, `x86_32`) or Linux (`x86_64`, `arm64`). Linux cross-compilation needs a
caller-provided cross toolchain. Android requires an SDK with `sdkmanager` and
`ANDROID_HOME`; Godot selects/installs its required NDK. Linux needs the native
development packages shown in Salt Line's validation workflow. The action
installs pinned uv and executes an exact SCons version. It does not install
system packages or silently change the caller's build profile.

## Example

Inside a checked-out Salt Line repository:

```yaml
permissions:
  contents: read
jobs:
  engine:
    runs-on: ubuntu-latest
    timeout-minutes: 90
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: ./.github/actions/build-godot-template
        id: engine
        with:
          tag: 4.7.2-stable
          commit: ed1daf0bf001b61586d9930840f2f1394092c079
          profile: tools/godot_build_profile.json
          platform: android
          arch: arm64
          flags: '["swappy=no"]'
          toolchain-id: godot-4.7.2-default-ndk
          cache: 'false'
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: engine
          path: ${{ steps.engine.outputs.directory }}
```

Other repositories can use
`cascadiacollections/saltline/.github/actions/build-godot-template@<full-commit-sha>`
after this change merges, supplying their own profile. Pin the source commit;
this action is not yet published as a separately versioned Marketplace product.
The Python driver works locally too: set the environment variables listed in
`action.yml` and run `build.py key`, `build.py build`, then `build.py report`.
Local execution requires Python 3.9+, git and uv; the source directory must be
fresh and absent.

## Inputs and outputs

Required inputs: `tag`, full lowercase source `commit`, JSON `profile` path,
`platform`, `arch`, and `toolchain-id` (compiler/NDK identity). Optional inputs:
`scons-version` (4.11.1), `flags` (JSON array of additional `name=value` options),
`output-directory` (`build/godot-templates`), and `cache` (`true`). Use a dedicated
output directory; cache archives that entire directory.

The fixed recipe is `target=template_release production=yes lto=full
optimize=size_extra debug_symbols=no`. Extra flags cannot override it. Profile
options unknown to the pinned Godot source fail before SCons. Source tag moves
fail against the full commit pin. Different games need different profiles;
removing rendering, networking or resource classes requires game-level tests.

Outputs are `key`, absolute `directory`, absolute `binary`, `sha256`, `bytes`,
and absolute `report`. The directory contains the native release binary,
`SHA256SUMS`, and `build-report.json` with source/profile/flags, byte size and
digest. Linux executable permissions are preserved. Android emits a native
library, not an installable APK. Salt Line retains its own compiler-compatible
APK repacking and ABI selection in `tools/godot_template.py`.

The cache key includes canonical profile JSON, source/tag/SCons, flags,
platform/architecture, driver bytes, runner OS/architecture/image version, and
caller toolchain identity. There are no broad restore keys. This prevents stale
reuse but does not promise reproducible bytes or cryptographic cache provenance.
Disable cache for newly attested release builds, as Salt Line does. A cache hit
regenerates the checksum/report from the restored binary. Build provenance and
reviewed digest pins belong in the caller's publishing job.

Salt Line's published template key additionally hashes this action, driver and
publishing workflow (including caller flags and toolchain identity).
The initial extraction changes that key: rebuild templates after merge and
review `python3 tools/godot_template.py lock --release` before shipping the new
engine. Lock schema, native artifact names, signing workflow and replay parity
remain unchanged. Until the new lock is reviewed, ordinary exports use official
templates; release tags require a current template lock.

Debug information is omitted rather than exported into separate symbol files.
Detached symbols and additional platforms are follow-up work requiring native
toolchain validation. The initial version has not demonstrated byte-for-byte
parity or measured build-time savings; CI must compile and run replay parity.

Tests: `python3 -m unittest discover -s .github/actions/build-godot-template -p
 'test_*.py' -v`. They cover recipe/output compatibility, invalid inputs, moved
source tags, key invalidation, unknown profile options and checksum/size outputs.
