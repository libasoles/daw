# Context

The domain model for the daw. Terms here are load-bearing: use them exactly, in
code, in tests, in issue titles and in the interface.

## Glossary

**Take** — a recording in the recording area. There is at most one at a time. It
holds the raw captured notes, which are never rewritten, plus two views over
them: a *trim* and a *quantisation* setting, both applied on read. A take is
editable and unfrozen.

**Block** — a take that has been added to the library and thereby **frozen**: its
trim and quantisation are resolved and fixed. A block has a name, a colour and an
instrument. Blocks cannot be edited; re-editing means recording again.

**Placement** — a block on the timeline, stored as an independent *copy* of it.
Because blocks are immutable, a copy behaves identically to a reference during
playback, and the copy means deleting from the library cannot silently alter an
arrangement.

Take, block and placement are three different things and the distinction is the
one that keeps the model honest. Don't call a block a "take"; don't call a
placement a "block".

**Pulse** — the unit of time. Notes are stored as pulse offsets, never as seconds
and never as bars. Changing the tempo changes only how pulses map to wall-clock
time at playback, so material recorded earlier speeds up or slows down without
being rewritten. Grid lines, trim handles and deliberate gaps all snap to the
pulse.

**Deliberate gap** — a silence a musician placed on purpose, by dragging with
`Cmd`. Stored as an explicit property of the placement that follows it, not
inferred from coordinates, so ripple edits elsewhere preserve it.

**Ripple** — the timeline's default behaviour: placements sit flush and never
overlap, so inserting one pushes the remainder later and deleting one pulls the
remainder earlier.

**Time signature** — governs the metronome's accent, the length of the count-in
and a purely visual accent on every Nth grid line. It has **no structural
power**: it anchors nothing and can be changed at any time without moving a
recorded note or a placement.

**Command** — the complete vocabulary of things a user can do. Every gesture in
the interface becomes one. Commands are the only way into `DawCore`.

**Effect** — what the core asks the shell to do rather than doing itself: hand a
schedule to the audio thread, write a file, raise a confirmation prompt, report a
missing device. The core performs no I/O.

**Port** — a trait the core depends on and the shell implements: `Synth`,
`MidiInput`, `AudioOutput`, `Storage`. Real in production, faked in tests.

## Shape of the system

```
gesture -> Command -> DawCore -> (ProjectState, Vec<Effect>) -> shell -> pixels/sound
```

`DawCore` (in `core/`) is a pure in-memory state machine and holds all musical
logic, including the sequencer. The shell (`src-tauri/`) owns the ports and the
window. The webview holds no musical logic: every pixel is derived from
`ProjectState`.
