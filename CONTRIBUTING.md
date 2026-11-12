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

`./scripts/perf.sh` (the "frame cadence" step) builds and launches the bundled Found'er and drives it with synthetic input, so the terminal running it needs:

* Screen Recording (frame capture)
* Accessibility (the resize drag)
* Automation of System Events (window placement)
* `swiftc`, from the Xcode command line tools

A step that exits 90, as `perf.sh` does on a locked screen, is reported as VOID and skipped: it does not fail the gate, but it did not run. `perf.sh` temporarily resets Found'er's saved window frame and puts yours back when it exits. `./scripts/keys.sh` and `./scripts/menubar.sh` have their own permission needs, listed at the top of each script.

## Signing

`./founder/scripts/bundle.sh` signs with a Developer ID or Apple Development identity if the login keychain has one, or with `NG_CODESIGN_IDENTITY`. Otherwise it creates a self-signed "NativeGPUI Dev" identity in its own keychain, `~/Library/Keychains/nativegpui-dev.keychain-db`, and changes your keychain search list to include it. That keychain's password is the public string `nativegpui`, and the key is imported so that any app can use it, so treat it as a throwaway identity.

To remove it:

```sh
security delete-keychain ~/Library/Keychains/nativegpui-dev.keychain-db
```

`security delete-keychain` also drops it from the search list.

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
