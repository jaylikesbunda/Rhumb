# Contributing

Thanks for taking a look. Rhumb is a small, focused program: a fast file
explorer with a built-in editor. Patches that keep it fast, keep it small and
keep it honest about its limits are welcome.

## Before you start

Open an issue first for anything larger than a bug fix, so we can agree on the
shape before you write it. Small fixes can go straight to a pull request.

## Building and testing

Needs Rust 1.88 or newer.

    cargo build --release
    cargo test
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

CI runs the same four, plus the exhaustive editor sweeps in a job of their own,
and a Linux build. The full suite is slow (the `codeedit::sweep` and
`app::full_app` tests are the long pole), so during development it is fine to
run just what you touched, for example:

    cargo test codeedit::behaviour

## The one rule that matters

The editor and the file list must not do work proportional to the size of a
document or a folder **per frame**. Frame cost is allowed to grow with what is
on screen, never with the file behind it. If a change adds an `O(document)` or
`O(folder)` pass to a frame, it needs a very good reason and a benchmark.

Two habits follow from it:

- Pure logic lives apart from egui (`buffer`, `fs_model`, `codeedit::window`,
  `index`, `search`, ...) so it can be tested without a window. Keep it that
  way; a new rule belongs in a free function that a test can call.
- Failure is returned, not panicked. Production code does not `unwrap` or
  `expect` - a broken file, a locked one, a permission error or a full disk is
  a message to the reader, not a crash.

## Style

- Match the surrounding code, including its comment style. Comments explain
  *why*, especially where something is subtle or was once wrong.
- `cargo clippy -- -D warnings` and `cargo fmt` must pass.
- A fix for a bug should come with a test that fails without it.

## Commits and pull requests

- Keep each commit to one idea, with a message that says what changed and why.
- Describe the change, what you tested, and any limit it has. Screenshots help
  for anything visual.
- By contributing you agree your work is licensed under the MIT licence in
  [`LICENSE`](LICENSE).
