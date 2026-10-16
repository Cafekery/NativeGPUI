# Contributing

## Build

You need macOS 26 or later and rustc 1.97 or later. Build every example in both feature configs:

```sh
cargo build --release --examples
cargo build --release --examples --features private-api
```

`private-api` is off by default. Examples that need it are skipped by the default build.

## Run Found'er

```sh
cargo run --release --example finder_demo
```

A bare Cargo binary never becomes the key window. For anything that depends on focus or the key window, build the app and open it:

```sh
./scripts/bundle.sh finder_demo && open -n "dist/Found'er.app"
```

## The gate

Before you send a change, run:

```sh
./scripts/check.sh
```

It must end with `== VERDICT: PASS`. It builds both feature configs with zero warnings, runs `cargo fmt --all --check`, the unit tests and the probes, and runs `scripts/surface.sh`, which keeps the public surface additive. It opens a few small windows, so run it in an unlocked session.

[docs/testing-and-tools.md](docs/testing-and-tools.md) covers the scripts and capture rules.

## Code

Read [AGENTS.md](AGENTS.md) first. It holds the comment rule (code carries a comment only if it is incomprehensible without one) and the invariants a change must keep. [docs/native-gpui.md](docs/native-gpui.md) maps the crate.

## Commits

* Conventional Commits, with a lowercase imperative subject: `fix(finder_demo): keep the selection after a rename`.
* The scope is the area you changed, such as `finder_demo`, `finder-fs`, `sidebar` or `materials`.
* One concern per commit.
* No AI or tool attribution trailers.

## License

By contributing, you agree that your contributions are licensed under the MIT license in [LICENSE](LICENSE).
