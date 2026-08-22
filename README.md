# daw

A minimal MIDI DAW: record short takes from a MIDI keyboard, keep the ones worth
keeping, and arrange them on a timeline. Dark, large-typed and low-density —
meant to be used at arm's length while holding an instrument.

macOS only. See [issue #1](https://github.com/libasoles/daw/issues/1) for the
full specification and [`CONTEXT.md`](CONTEXT.md) for the vocabulary.

## Status

Milestone **H1**, in progress. The shell opens a window and now makes sound: a
"Play test note" button sounds a piano note through a bundled SoundFont. No
MIDI input yet, and no way to play more than one fixed debug note.

## Requirements

- macOS 12 or later, with Xcode command line tools (for the system WebView).
- [Rust](https://rustup.rs) (stable).
- Node 22 or later, with npm.

```sh
npm install
```

## Run, build, test

| What | Command |
| --- | --- |
| Run in development | `npm run dev` |
| Release build | `npm run build` |
| Test the core | `cargo test` |

- **`npm run dev`** starts Vite and the Tauri shell together, and opens the
  window. The Rust side rebuilds on change; the frontend hot-reloads.
- **`npm run build`** type-checks the frontend, bundles it, compiles the shell
  in release mode and packages `daw.app` and a `.dmg` into
  `target/release/bundle/`. Add `-- --no-bundle` to stop at the binary.
- **`cargo test`** runs the whole workspace; `cargo test -p daw-core` runs just
  the core, which is where every test lives.

Also available: `npm run typecheck` (frontend only) and `npm run frontend:build`
(type-check plus Vite build, no Rust).

## Layout

```
Cargo.toml        Rust workspace root
core/             daw-core: the musical core. No Tauri, no I/O, all the tests.
src-tauri/        daw: the desktop shell. The only crate that knows Tauri exists.
src-tauri/assets/soundfonts/  The bundled SoundFont; see its NOTICE.md for license and source.
src/              The frontend. Presentation only — no musical logic.
index.html        Vite entry point
```

The split is the project's central architectural rule: musical logic lives in
`daw-core`, behind one seam (`Command -> DawCore -> (ProjectState, Vec<Effect>)`),
so it can be tested without a window, a keyboard or a sound card. The shell owns
the ports the core cannot — MIDI input, synthesis, audio output, storage — and
the webview only turns gestures into commands and renders state. CI fails the
build if `daw-core` ever acquires a Tauri dependency. See
[`docs/adr/0001-core-crate-independent-of-tauri.md`](docs/adr/0001-core-crate-independent-of-tauri.md).

## Testing conventions

All tests live in `core/tests/`, an *integration* test target: it can only reach
`daw-core`'s public API, which makes it impossible to write a test coupled to
internals. A good test here drives the core through public commands and asserts
on `ProjectState` or on the note stream the core produces. It never needs a MIDI
keyboard, an audio device or a filesystem — the four ports are faked.

The webview and the Tauri bridge are deliberately untested: both are thin enough
that pushing the seam this high is what makes leaving them alone defensible.

## Sound

`daw-core` defines the `Synth` port (`note_on`/`note_off`/`render`, instruments
named by an opaque id) and is tested against a spy fake — see
`core/tests/synth_port.rs`. The real implementation, `src-tauri`'s `RustySynth`,
is backed by [`rustysynth`](https://crates.io/crates/rustysynth) and the full
bundled GeneralUser GS 2.0.3 General MIDI/GS SoundFont
(`src-tauri/assets/soundfonts/GeneralUser-GS.sf2`; licence, source and engine
compatibility note in that directory's `NOTICE.md`).

Audio output is split across two threads because `cpal`'s render callback runs
on a real-time thread where blocking and allocation are forbidden, and neither
`DawCore` nor `rustysynth`'s rendering belongs there: a non-real-time **synth
thread** owns the real `Synth` and renders into a lock-free queue
([`rtrb`](https://crates.io/crates/rtrb)); the real-time callback only pops
already-rendered samples from that queue. See `src-tauri/src/audio/mod.rs` for
the full design, and `CONTEXT.md`'s "synth thread" entry for the vocabulary.

This is not exercised by `cargo test` — there is no sound hardware in CI to
confirm against, and the spec is explicit that `rustysynth` and `cpal` are
"exercised by hand" behind their ports. Confirming that a note actually sounds
means running `npm run dev` (or the built app) and pressing "Play test note."

## Typography and the dark palette

The interface uses the **macOS system typeface, SF Pro** (`ui-sans-serif` /
`-apple-system`), with **SF Mono** (`ui-monospace`) for numeric readouts. It is
optically sized by the OS, ships with every macOS install so it costs the bundle
nothing, and has true tabular figures — which matters as soon as BPM and bar
numbers appear.

Sizes are a fixed scale, defined once in `src/styles.css`. **16px is the floor**;
the body default is 18px:

| Token | Size | Used for |
| --- | --- | --- |
| `--text-xs` | 16px | labels, status lines |
| `--text-sm` | 18px | body and controls (default) |
| `--text-md` | 22px | section headings |
| `--text-lg` | 28px | primary readouts |
| `--text-xl` | 40px | the one thing on screen that matters |
| `--text-display` | 64px | the app mark |

Surfaces are near-black (`#0d0f12`) rather than grey, so the coloured blocks in
the library and timeline carry. Spacing is deliberately generous: low density is
a feature, not an oversight.

## CI

`.github/workflows/ci.yml` runs on every push and pull request:

- `cargo fmt --check`, `cargo clippy` and `cargo test` against `daw-core`
- a `cargo tree` check that `daw-core` has not gained a Tauri dependency
- the frontend type-check and build
- a full release build of the macOS shell
