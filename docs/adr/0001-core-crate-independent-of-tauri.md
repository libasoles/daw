# ADR-0001: The core crate does not depend on Tauri

- Status: accepted
- Date: 2026-08-22
- Context: issue #1 (spec), issue #2 (scaffold)

## Context

The app is a Tauri desktop app for macOS, and WebKit has never implemented the
Web MIDI API — so `navigator.requestMIDIAccess` is undefined inside WKWebView.
MIDI input must be read natively, and rather than split the musical clock across
two languages, the whole core lives in Rust: MIDI input, synthesis, audio output,
the sequencer and all project state.

That decision leaves an obvious failure mode. If the musical logic and the
desktop shell share one crate, `tauri::AppHandle` and `tauri::State` leak into
the sequencer within a few tickets, and from then on nothing can be tested
without starting a window.

## Decision

The Rust side is a Cargo workspace with two crates:

- **`core/` (`daw-core`)** — the musical core. Zero dependency on Tauri, on a
  webview, or on anything that only exists inside a desktop app. It performs no
  I/O: side effects come back out as `Effect` values for the shell to carry out,
  and the things it cannot do itself sit behind four ports (`Synth`,
  `MidiInput`, `AudioOutput`, `Storage`).
- **`src-tauri/` (`daw`)** — the shell. Depends on `daw-core`, hosts the webview,
  implements the ports. It is the only crate that knows Tauri exists.

The dependency points one way only, and CI enforces it: a `cargo tree` check
fails the build if `daw-core` ever resolves a Tauri dependency.

## Consequences

- Every test in the project runs as a plain `cargo test` against `daw-core`,
  with no window, no MIDI keyboard and no sound card. Tests live in
  `core/tests/` — an integration target, so they can only reach the public API.
- The shell stays thin by construction: there is nowhere in it to put logic that
  wouldn't be better off in the core.
- Swapping the shell (a different windowing toolkit, a headless harness, a CLI)
  is a rewrite of one crate rather than of the app.
- The cost is indirection: some plumbing between the webview and the core that a
  single crate would not need. This is accepted.
