# kynex-core

Kynex's logic without UI. Rules for this crate:

- **No gpui** (and no other UI crate). If something needs a UI type, keep it opaque here
  (see `session::Preview`) and let the app fill it in.
- Functions may block (processes, git, files, network); say so in the doc comment. The app
  calls them through `cx.background_spawn`.
- Errors are `Result<_, String>` with a message fit to show the user as-is.
- Every module gets `#[cfg(test)] mod tests` next to the code. Tests that run the real
  `claude` CLI are `#[ignore]` (they use quota); everything else must run offline.
- OS-specific code goes behind `#[cfg(...)]` / `cfg!(...)` with a fallback for the others.
- Check with `./scripts/check.ps1 -Core` from the repo root (seconds; no gpui build).
