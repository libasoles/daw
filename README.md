# daw

A minimal MIDI DAW: record blocks from a MIDI keyboard, arrange them on a timeline.

Planned as a Tauri desktop app: a Rust core (MIDI input via `midir`, synthesis via
`rustysynth`, audio output via `cpal`) with a web frontend used for the UI only.

## Status

Specification phase. Nothing is implemented yet — this repository currently holds
only the project scaffolding while the design is worked out.
