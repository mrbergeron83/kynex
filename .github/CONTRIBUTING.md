# Contributing to Kynex

Thanks for helping. Bug reports and ideas are the most useful contributions right now.

## Issues first

Bugs and ideas live in [GitHub issues](https://github.com/mrbergeron83/kynex/issues), not TODO
files or code comments. Use the templates (bug or idea); include your OS and Kynex version
(Kynex menu → About) for bugs. Security problems go through the [security policy](SECURITY.md),
not public issues.

## Code contributions

Kynex is free software under the [GNU GPL, version 3 or later](../LICENSE). Contributions are
accepted under the same license; by opening a pull request you agree to license your change
under it.

**Open an issue before writing code** and agree on the change there, so nobody's work goes to
waste; every pull request links its issue (`Fixes #N`, or `Refs #N` if it doesn't close it).
Pull requests are reviewed by the maintainer and merge only once every check passes.

## Building from source

You need a recent stable Rust toolchain and `claude` installed and logged in.

```sh
claude              # once, to log in
cargo run --release
```

On Linux, gpui needs a few system libraries:

```sh
sudo apt-get install -y libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-x11-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev libasound2-dev
```

### Hot-patching

Edit UI code and see it live without restarting, via [subsecond](https://crates.io/crates/subsecond):

```powershell
cargo install dioxus-cli@0.7.10 --locked   # once
./scripts/dev.ps1                          # dx serve --hot-patch, with auto-restart
```

Everything reached from `KynexApp::render` is patched on save, in both workspace crates;
third-party dependencies aren't patched. A compile error leaves the running app alone. Changing
a struct's fields (adding state) can't be hot-patched and crashes the app, so `dev.ps1` restarts
dx, which relaunches the app once a full build succeeds. Tabs and Claude sessions are restored.

Set `KYNEX_DATA_DIR` to run a test instance without touching your real state.

## Checks

**Done means `./scripts/check.ps1` passes**: `cargo fmt --check`, clippy with `-D warnings`, and every
test, built into `target/claude` so it never touches the dev build. `./scripts/check.ps1 -Fix` formats
and applies clippy's fixes first; `./scripts/check.ps1 -Core` checks only `kynex-core` (seconds).

`cargo test -p kynex-core -- --ignored` runs a live round-trip against the real CLI (uses a
little quota).

### Tests

New code comes with tests, as a matter of policy:

- Logic goes in `kynex-core`, which has no UI, and every module there has unit tests next to the
  code (`#[cfg(test)] mod tests`). A new feature adds tests for its rules; the app only draws.
- A bug fix adds a test that fails without the fix, where the bug can be reproduced without a
  window.
- A new parser of outside text (CLI output, git, files Claude Code writes) gets an entry in
  `kynex_core::fuzz` and a fuzz target in `crates/kynex-core/fuzz/`; an input that made it panic
  becomes a unit test.
- Tests run offline. The few that need the real `claude` CLI or the network are `#[ignore]`.

CI runs the same checks on Windows, macOS and Linux. The Security workflow runs
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`cargo deny check`, configured in
`deny.toml`) and [zizmor](https://github.com/zizmorcore/zizmor) on the workflows; CodeQL and
OpenSSF Scorecard run too. A new dependency must have a license listed in both `deny.toml` and
`packaging/about.toml`.

## Layout

A cargo workspace: `crates/kynex-core` is the logic with no UI (the `claude` protocol, sessions,
project settings, git, terminals), and `src/` is the gpui app. [CLAUDE.md](../CLAUDE.md) has the
full map and the conventions (where code goes, UI style, Rust 2024 notes).

Kynex must build and run on Windows, macOS and Linux: keep platform-specific code behind
`#[cfg(...)]` with a working fallback for the other OSes.

State is saved to `state.json` in the per-user data dir: `%APPDATA%\kynex` on Windows,
`~/Library/Application Support/kynex` on macOS, `~/.local/share/kynex` on Linux.

## Commits

- Small commits with a sentence-case summary saying what changes for the user
  ("Show how full the context is, and compact it in one click"); `Fixes #N` closes an issue.
- User-facing changes get a line under `[Unreleased]` in [CHANGELOG.md](../CHANGELOG.md).
- Label pull requests (`enhancement`, `bug`, `security`, …): generated release notes are grouped
  by label.

## Releasing

Versions follow [SemVer](https://semver.org) and are tagged `vX.Y.Z` (`vX.Y.Z-beta.N` for a
prerelease). Only the maintainer releases:

1. In a pull request, bump `version` in `Cargo.toml` and turn `[Unreleased]` in CHANGELOG.md into
   the new version's section, with today's date. Merge it once every check passes.
2. Tag the merge commit on `main` with a signed tag and push it:

   ```sh
   git switch main && git pull
   git tag -s vX.Y.Z -m "Kynex X.Y.Z"
   git push origin vX.Y.Z
   ```

   Signing uses your SSH key, added to GitHub as a signing key, so the tag shows as Verified;
   [Signing tags](#signing-tags) sets up a machine for it. Only admins can create release tags
   (the "Protect release tags" ruleset).
3. The Release workflow checks the tag matches `Cargo.toml` and is on `main` (and warns if it
   isn't signed), re-runs CI and cargo-deny on the tagged commit, builds every OS, signs the
   Windows binary when that's set up, and publishes the archives with `SHA256SUMS` and a signed
   build provenance attestation. A tag that fails any of that publishes nothing.
4. Running installs pick the release up through the self-updater; check one updates.

### Signing tags

Release tags are signed with an SSH key. Give each machine you release from its own key rather
than copying one around: GitHub takes any number of signing keys, and a lost machine's key can be
removed on its own. Once per machine:

1. **A key.** Reuse the one you push with if the machine has one (`~/.ssh/id_ed25519.pub`), or
   make one; a passphrase is fine, git asks for it when signing:

   ```sh
   ssh-keygen -t ed25519 -C "you@example.com"
   ```

2. **Add it to GitHub as a signing key.** Copy the public key, then on
   [github.com/settings/ssh/new](https://github.com/settings/ssh/new) set *Key type* to
   **Signing Key** and paste it. A key you also push with is added twice, once per type.

   ```sh
   # Windows (PowerShell)
   Get-Content "$env:USERPROFILE\.ssh\id_ed25519.pub" | Set-Clipboard
   # macOS
   pbcopy < ~/.ssh/id_ed25519.pub
   # Linux (Wayland: wl-copy < …; or print it with cat and copy it)
   xclip -selection clipboard < ~/.ssh/id_ed25519.pub
   ```

3. **Tell git to sign with it.** The email in `git config user.email` must be a verified email
   on your GitHub account, or the tag won't show as Verified.

   ```sh
   git config --global gpg.format ssh
   git config --global tag.gpgSign true
   # macOS / Linux
   git config --global user.signingkey ~/.ssh/id_ed25519.pub
   # Windows (PowerShell)
   git config --global user.signingkey "$env:USERPROFILE\.ssh\id_ed25519.pub"
   ```

   On Windows, if signing says it can't find `ssh-keygen`, point git at Git for Windows' copy:
   `git config --global gpg.ssh.program "C:/Program Files/Git/usr/bin/ssh-keygen.exe"`.

4. **Optional: check signatures locally.** `git tag -v` needs to know which keys are yours: list
   every machine's public key in `~/.ssh/allowed_signers`, one line each, as
   `you@example.com ssh-ed25519 AAAA…`, then

   ```sh
   git config --global gpg.ssh.allowedSignersFile ~/.ssh/allowed_signers
   ```

   (on Windows, `"$env:USERPROFILE\.ssh\allowed_signers"`). GitHub doesn't need this file.

5. **Try it** without releasing anything: `git tag -s sign-test -m test`, `git tag -v sign-test`
   ("Good signature"), then `git tag -d sign-test`.

### Code signing

Windows binaries are signed with
[Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/) by the
`sign-windows` job, once it's set up:

1. Create an Artifact Signing account and a public-trust certificate profile, and an Entra app
   registration with a federated credential for this repository's `release` environment and the
   "Artifact Signing Certificate Profile Signer" role on the account.
2. Create the `release` environment: deployments from `v*` tags only, the maintainer as required
   reviewer. Add the secrets `AZURE_CLIENT_ID`, `AZURE_TENANT_ID` and `AZURE_SUBSCRIPTION_ID`.
3. Add the repository variables `AZURE_SIGNING_ENDPOINT` (e.g.
   `https://eus.codesigning.azure.net`), `AZURE_SIGNING_ACCOUNT` and `AZURE_SIGNING_PROFILE`, then
   `SIGN_WINDOWS` = `true` to turn the job on.

macOS signing and notarization (a Developer ID certificate and a `.app` bundle) aren't set up yet.
