# Development environment and enterprise rollout

The checked-in devcontainer is a small Linux environment for the Rust simulation,
GDExtension compilation, and Python asset tools. Use VS Code's **Reopen in
Container** with Docker, or select this configuration when creating a Codespace.
Native desktop development remains supported. `mise install` installs the Python
and uv pins and reads Rust from `rust-toolchain.toml`.

## Toolchain contract

| Tool | Pin | Reason |
| --- | --- | --- |
| Rust | 1.99.0 | Stable release; rustfmt and clippy included |
| uv | 0.13.0 | Exact version in mise, container, Python metadata and CI |
| Python | 3.15.0 | Current stable release; `.python-version` is the development default |
| Godot | 4.7.2 | Current stable; keep `api-4-7` and template lock aligned |

Python tools support 3.12 and newer. Python 3.10 has reached end of life. Python
3.15.0 was released October 9, 2026 and is the development pin. Locked Pillow
wheels and the Python tooling suite are validated with that interpreter. See the
[Python releases](https://www.python.org/downloads/),
[Rust release notes](https://doc.rust-lang.org/releases.html),
[uv releases](https://github.com/astral-sh/uv/releases), and
[Godot release archive](https://godotengine.org/download/archive/).

Container creation only runs `uv sync --locked`; it does not compile the game,
generate media, download Godot templates, or provision Android/Xcode/Blender.
The editor uses the unprivileged `developer` account, mapped to the local UID by the devcontainer
client. No Docker socket, host credentials directory, or privileged mode is
requested. Image build context excludes repository source and assets. Base images
use version tags with immutable multi-architecture digests. Refresh the digests
and rebuild deliberately for security fixes; use an approved enterprise mirror
when required. The Rust image uses the slim Debian variant to reduce storage. Container Python
environments live under `/home/developer/.venvs/saltline`, keeping their
interpreter links separate from the host checkout's `.venv`.

```sh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p salt-line-godot
uv run --locked python -m unittest discover -s tools -p 'test_*.py'
```

To play, use the pinned Godot editor on the host with a native extension build,
or install the matching official Linux engine into the container and run
headless checks. A Linux container cannot produce the host's macOS or Windows
extension for GUI play. Mobile SDKs, export templates, platform signing, desktop
portals and GUI forwarding are separate from this baseline; follow README.md and
RELEASING.md. Do not raise Godot independently of the binding API and custom
engine template inputs/lock.

The devcontainer workflow builds and smoke-tests the environment only when its
configuration or runtime pins change, or on manual dispatch. It does not publish
an image or create Codespaces. Existing platform CI remains the release gate;
local toolchain validation does not substitute for cross-platform digest checks.

## Cost controls for an organization owner

The repository is internal under `cascadiacollections` on github.com. These are
recommended rollout settings, not settings applied by this commit:

- Start with local containers; enable organization-paid Codespaces for a small
  named pilot group and this repository only. Restrict machine types to the
  smallest measured usable option (start with 2 cores / 8 GB).
- Set maximum idle timeout to 15 minutes and retention to 3 days. Stopped
  Codespaces still consume storage. Contributors must push work before deletion.
- Start with no prebuild configuration. Add one default-branch prebuild only
  after measuring cold startup versus its compute/storage cost. Avoid one per
  branch or region.
- Set separate Codespaces and Actions budgets with usage alerts and **stop usage
  when budget limit is reached** where supported. Choose the cap from the actual
  pilot allocation; an alert-only budget does not cap spend. Verify the policy
  with the billing owner before broadening access.
- Review compute hours, retained GB-months, prebuild cost, and CI runner minutes
  weekly during the pilot. Estimate cost as compute hours × the current machine
  rate + retained storage × the current storage rate; include prebuilds. Use
  billing reports rather than assuming enterprise membership includes usage.
- Preserve the existing CI concurrency cancellation and platform selection.
  Keep large artifacts short-lived where downstream consumers allow; do not
  remove release provenance or determinism artifacts needed by later jobs.

GitHub documents [idle timeout policies](https://docs.github.com/en/codespaces/managing-codespaces-for-your-organization/restricting-the-idle-timeout-period),
[retention policies](https://docs.github.com/en/codespaces/managing-codespaces-for-your-organization/restricting-the-retention-period),
and [Codespaces billing](https://docs.github.com/en/billing/concepts/product-billing/github-codespaces).
For GitHub Enterprise Server, use local containers and your approved runner fleet;
confirm feature availability for that server version before adopting cloud-specific
services. The current remote is GitHub Enterprise Cloud, not a GHE Server host.

## Repository enrollment

Require pull requests and the existing final CI gate through organization rulesets;
require review for workflow, dependency and release changes. Assign CODEOWNERS to
real maintained teams only after confirming team names and access. Keep Actions
read-only by default, pin third-party actions, restrict allowed actions, and keep
release credentials in protected environments. Never run untrusted pull-request
code on a persistent privileged self-hosted runner; use disposable isolated
workers if hosted runners are unavailable. Keep signing and publishing in the
existing trusted release flow.

Before rollout, verify an x86_64 and arm64 cold create, rebuild and reattach with
the team's devcontainer client. Check UID mapping, locked dependency sync, Rust
checks, and expected disk use. A separate local Git worktree has a `.git` pointer
outside its checkout; for Git operations in a container, use a standalone clone
or a client setup that also mounts the parent repository metadata. Do not mount
unrelated host directories to work around this.

## Zed, VS Code and Codespaces

[Zed supports this devcontainer configuration](https://zed.dev/docs/dev-containers).
Open the checkout and accept **Open in Container**, or use **Project: Open Remote**.
Docker is the default; Podman users set `use_podman: true` in their personal Zed
settings. Reopen after stopping the container when configuration changes; Zed
currently does not automatically rebuild it. TOML/GDScript extensions are declared
for Zed, with Rust diagnostics and explicit verification tasks under `.zed/`.
[Check-on-save](https://zed.dev/docs/languages/rust) covers normal targets;
run the all-targets task before submitting changes. The same limit applies to
VS Code to avoid compiling every benchmark and test on each save.

For a Codespace, connect Zed through its
[OpenSSH remote workflow](https://zed.dev/docs/remote-development):

```sh
# On your laptop, after GitHub CLI login and choosing an existing Codespace:
gh codespace ssh --config > ~/.ssh/codespaces
```

Add `Include ~/.ssh/codespaces` to your personal SSH config, then select the
emitted host in Zed's Remote Projects dialog. Open `/workspaces/saltline` (confirm
the actual checkout path in the remote terminal). GitHub CLI documents the
[SSH configuration integration](https://cli.github.com/manual/gh_codespace_ssh).
The digest-pinned SSH feature supplies the required server. Container startup
runs as root to start that service; editor terminals, tasks and bootstrap run as
`developer`. Root login and password authentication are disabled by an SSH drop-in. No host
SSH port is published by this configuration. Keep host keys
verified and use GitHub's authentication rather than adding shared passwords.
For restricted egress, Zed's per-server `upload_binary_over_ssh` option can transfer
its matching server from the laptop. Keep SSH hosts and tokens out of git.

Use a private port for site previews. Start the server explicitly; do not start
long-running services during attach. Closing the editor is not a reliable stop
for a Codespace: stop it through GitHub or `gh codespace stop` when finished.
Reattach reuses the existing container and dependency caches; recreate only when
needed. Organization idle/retention and budget policies remain the spending
controls. Rust's two-job default limits memory pressure; it is not a billing cap.
