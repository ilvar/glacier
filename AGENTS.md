# AGENTS.md

Guidance for coding agents working in this repository. Humans should read
[README.md](README.md) instead; this file is about how to work on the code,
not what it does for an end user.

## What this is

`legacy` is a local-first, self-hosted life-story vault CLI, written in
Rust. See the README's "Non-negotiable design principles" section before
changing architecture — plain files on disk are the source of truth, no
cryptography is implemented here (only shelled-out `age` and the
`sssmc39` SLIP-0039 crate), no lock-in formats, offline by default, and
the archive must explain itself to a reader decades from now with no
`legacy` binary.

## Built with strictrs

This project is checked against [strictrs](https://github.com/ilvar/strictrs),
a strict subset of Rust with a machine-readable diagnostic loop. Any code
you write here must satisfy it:

- No `unsafe`.
- No `unwrap`/`expect`/slice indexing outside `#[cfg(test)]` code.
- No numeric `as` casts (use `try_from`, `try_into`, or a named helper like
  `as_u16` in `src/tui/render.rs`).
- No glob imports.
- No mutable globals (`static mut`).
- No catch-all (`_`) match arms on enums defined in this crate — match
  every variant explicitly so a new variant fails to compile instead of
  falling through silently.
- Every `pub fn` needs an explicit return type, including `-> ()` for one
  that returns nothing.
- Every filesystem, process, and network effect must live in a module
  marked `// strictrs: capability`. Today that is only `src/cap.rs` —
  everything else calls `cap::fs::*`, `cap::process::*`, `cap::net::*`
  rather than touching `std::fs`, `std::process`, or `std::net` directly.
  This is what makes the program's entire blast radius auditable by
  reading one file.

**Known strictrs sharp edge:** its `explicit_return_type` check used to
false-positive on a literal `-> ()` (it misread the return type's own
parentheses as the parameter list's closing paren). Fixed upstream in
[ilvar/strictrs#6](https://github.com/ilvar/strictrs/pull/6). Until that
lands and this project's pinned strictrs version picks it up, `src/tui/`
works around it with a local `type Unit = ();` alias in `app.rs` and
`render.rs` — use `-> Unit` there rather than reverting to bare `-> ()`.

## Architecture map

- `src/cap.rs` — the only file allowed to name `std::fs`, `std::process`,
  or `std::net`. Read this file to see everything the program can touch.
- `src/core/` — on-disk formats and domain logic: `story`, `vault`,
  `manifest`, `timeline`, `index` (SQLite FTS, rebuildable), `media`,
  `interview`, `crypto`/`seal` (age + SLIP-0039), `llm`/`voice`
  (opt-in, network-gated), `env` (the `LEGACY_*`-preferred /
  `OPENAI_*`-fallback env var lookup), `dates`, `yaml`, `clock`.
- `src/cli.rs` and `src/api.rs` — thin front ends over the same `core`
  functions (CLI verbs and a localhost-only REST API). `src/web.html` is
  embedded into API mode and served from `/`.
- `src/tui/` — the `legacy tui` ratatui browser, split into `app.rs`
  (state and transitions, no terminal, fully unit-testable),
  `render.rs` (drawing, testable via `ratatui::backend::TestBackend`),
  and `mod.rs` (the thin raw-mode/event-loop glue that actually needs a
  TTY — keep this file small).
- `src/args.rs` — hand-rolled argument parsing (no external CLI-parsing
  crate).
- `Dockerfile` — multi-stage production image; API mode is the default
  container command and `/data` is the archive volume.
- `charts/glacier/` — Helm chart for the web/API deployment.

## Working loop

Install the local commit hooks once:

```bash
pre-commit install
```

For a normal change, prefer the repository commit helper:

```bash
./commit.sh "Explain why this change exists"
```

It performs the automatic version bump, runs the complete pre-commit suite,
stages the checked tree, and creates the commit. Direct `git commit` is still
supported; if the version has not already changed, the hook bumps it and the
first attempt will stop so `Cargo.toml`/`Cargo.lock` can be staged before
retrying.

The underlying checks are:

```bash
cargo build
cargo test
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
strictrs check .
```

All must be clean before pushing. `strictrs check .` should print
`{"ok": true, "error_count": 0}`. The Rust toolchain is pinned to 1.97.1
via `rust-toolchain.toml`; don't bump it casually.

Optional external tools used at runtime (never at build time): `age` and
`age-keygen` for encryption, `par2` for sealed-archive recovery data,
`ffmpeg`/`ffprobe` for media metadata and voice recording. All of them are
optional — missing-tool errors must stay actionable, not a panic.

## Versioning and releases

`package.version` in `Cargo.toml` is the single source of truth and the binary
must report it through `legacy --version`. Use Semantic Versioning.

Every commit changes the application version. The default automatic change is
a patch bump. For a feature or breaking change, run one of these before the
commit:

```bash
./scripts/bump-version.sh minor
./scripts/bump-version.sh major
```

The automatic hook detects that the version already differs from `HEAD` and
will not add another patch bump. `Cargo.lock` must always contain the same
root-package version; `scripts/check-version.sh` enforces this.

Every push to `main` is an automatic release (normally a merged PR). The
Container/Helm/Release workflow requires the Cargo version to be new, publishes
the versioned image and Helm chart, then creates matching tag `vX.Y.Z` and a
GitHub Release with generated notes. Do not manually create routine release tags,
and do not introduce a second version source in Helm, Docker, build scripts, or
git metadata. See `docs/releasing.md` for details.

## Containers and Helm

`.github/workflows/container.yml` validates the chart and builds the Docker
image on pull requests. Every push to `main` publishes multi-architecture
`linux/amd64` + `linux/arm64` images to `ghcr.io/ilvar/glacier` with `latest`,
`X.Y.Z`, `X.Y`, and commit-SHA tags. It also publishes an OCI Helm chart under
`ghcr.io/ilvar/charts/glacier`, then creates the corresponding GitHub release.

The chart defaults to the published GHCR image. For local kind/k3d development,
override `image.repository`, `image.tag`, and set `image.pullPolicy=Never` after
loading the image into the cluster. Keep ingress disabled by default because API
mode has no authentication.

## CI

`.github/workflows/ci.yml` runs: format/clippy/strictrs on Ubuntu, tests
on Ubuntu/macOS/Windows, and release binary builds for five targets
(Linux x86_64/aarch64 musl via `cross`, macOS x86_64/aarch64, Windows
x86_64). A test that hardcodes a path as a string (e.g. comparing against
`"timeline/2001/..."`) will fail on Windows, where `PathBuf::display()`
uses `\` — build expectations with `Path::join` instead.

`.github/workflows/container.yml` additionally validates that a main release uses
a fresh Cargo version, lints/renders the Helm chart, builds the multi-architecture
image, publishes the image/chart, and creates the matching GitHub Release.

## Repository conventions

- Primary development branch is `main`.
- Commit messages explain *why*, not what — the diff already shows what
  changed.
- Prefer `./commit.sh` so the patch version bump and pre-commit checks happen
  consistently.
- Don't add features, error handling, or abstractions beyond what a task
  requires; this codebase favors a few duplicated lines over a premature
  helper.
