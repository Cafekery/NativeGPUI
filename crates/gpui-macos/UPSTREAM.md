# Upstream

Repo: https://github.com/zed-industries/zed
Revision: `90d8514059561f0e44af0a32e84e1c329cef3232` (main, 2026-10-09)

| zed crate                | here                                      |
| ------------------------ | ----------------------------------------- |
| `crates/gpui_macos/src`  | `src/`, verbatim plus `patches/`          |
| `crates/gpui_apple`      | git dependency, same revision             |
| `crates/gpui`            | git dependency, same revision             |
| `crates/collections`     | git dependency, same revision             |
| `crates/gpui_util`       | git dependency, same revision             |
| `crates/scheduler`       | git dependency, same revision             |

Sources and every `rev =` in `Cargo.toml` use one revision. Never mix, and
never use the crates.io `gpui` release. `media` is no longer a dependency
of this crate (zed `#64251`).

Everything that differs from upstream lives in `patches/`, one file per change,
with provenance in the file header. On this tip that is `0001` through
`0007`. `0007` drains an autorelease pool around the launch work `0003`
runs before `app.run()` (re-applied from a9ed59c; the 90d8514 roll dropped
it). `CHANGELOG.md` records each roll.

## Rolling

1. Pick one zed revision `R` (a `main` commit; check that it really is on
   `main`, see the a6c1ad5 note in `CHANGELOG.md`).
2. Copy `crates/gpui_macos/src/*` over `src/`, deleting files upstream removed.
   Commit as "gpui_macos from zed-industries/zed at R, verbatim".
3. `Cargo.toml`: every `rev =` to `R`; mirror `crates/gpui_macos/Cargo.toml`
   (features, dependency list) with concrete versions taken from zed's
   workspace `Cargo.toml` at `R`. Commit.
4. `git apply --3way patches/*.patch`, resolve, run
   `rustfmt --edition 2024 --style-edition 2024 src/*.rs` (zed's
   `rustfmt.toml`). Commit per patch. If a hunk has no target, stop and say
   which one; do not weaken the patch.
5. Regenerate each patch file from `git diff <verbatim commit> <patch commit>`
   and check `diff -r src <zed>/crates/gpui_macos/src` is exactly the patches.
6. Update this file and `CHANGELOG.md`.

On Linux, `cargo generate-lockfile` and `cargo check --target
aarch64-apple-darwin` (with `gpui_apple` build-script output stubbed)
catch manifest and type errors. gpui_apple's build script is
`cfg(target_os = "macos")` on the host, so it emits nothing there: after
the first failing check, put an empty `shaders.metallib` in gpui_apple's
`OUT_DIR`, then rerun. The `media` crate stub from earlier rolls is not
needed after `#64251`. Shaders, AppKit behavior and tests need a Mac.
