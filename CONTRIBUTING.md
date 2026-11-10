# Contributing

## Requirements

macOS 26 or later, Apple silicon, and rustc 1.97 or later.

## Build

```sh
cargo build --release --examples
cargo build --release --examples --features private-api
cargo build --release -p founder
```

`private-api` is off by default; the examples that need it are skipped by the plain build.

## Run Found'er

```sh
cargo run --release -p founder
```

A bare Cargo binary never becomes the key window. For anything that depends on focus, build the app and open it:

```sh
./founder/scripts/bundle.sh && open -n "dist/Found'er.app"
```

## The gate

Before you send a change, run:

```sh
./scripts/check.sh
```

It must end with `== VERDICT: PASS`. It builds both feature configs with zero warnings, checks formatting, runs the unit tests and the probes in `probes/`, builds the docs, and runs `./scripts/surface.sh`, which keeps the public surface additive. It opens a few small windows, so run it in an unlocked session.

## Code

* A comment only where the code is incomprehensible without one.
* Refactors preserve behaviour.
* Keep the public surface: `./scripts/surface.sh` fails on anything removed or changed.
* One gpui revision. Never gpui from crates.io, never two revisions; `cargo tree -i gpui` must show one.

`crates/gpui-macos` is zed's `gpui_macos` plus a small patch series. To move to a newer gpui, follow [crates/gpui-macos/UPSTREAM.md](crates/gpui-macos/UPSTREAM.md).

## Commits

* Conventional Commits with a lowercase imperative subject: `fix(founder): keep the selection after a rename`.
* The scope is the area you changed, such as `founder`, `finder-fs`, `sidebar`, `materials` or `gpui-macos`.
* One concern per commit.
* No AI or tool attribution trailers.

## License

By contributing, you agree that your contributions are licensed under the MIT license in [LICENSE](LICENSE). Changes under `crates/gpui-macos` are Apache-2.0, like the rest of that crate.
