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

**Synth** — the port (`daw_core::ports::Synth`) that turns note on/off
requests into sound: `note_on`, `note_off`, `render`. Instruments are named
by an opaque `InstrumentId`, never by an enum baked into the trait, so a
third instrument is a data change, not a trait change. The real
implementation (`src-tauri`'s `RustySynth`) is backed by `rustysynth` and the
bundled GeneralUser GS 2.0.3 SoundFont; tests use a spy that records which
notes it was asked to sound and at which pulse. GeneralUser GS is the canonical
bundled bank: do not substitute a smaller bank merely to reduce the bundle.
Its advanced SoundFont modulators make full-fidelity rendering a requirement
for any future synth-engine change; see the asset NOTICE and issue #1.

**The synth thread** — the non-real-time OS thread (`src-tauri`'s `audio`
module) that owns the real `Synth` and does the actual rendering. It exists
because `cpal`'s render callback runs on a real-time audio thread where
blocking and allocation are forbidden, and neither `DawCore` nor
`rustysynth`'s rendering is safe to run there. The synth thread drains note
on/off requests and pushes rendered samples into a lock-free queue; the
real-time callback only pops already-rendered samples from that queue and
copies them to the output device — it never calls into `Synth` or `DawCore`
directly. The two threads are connected by `rtrb` ring buffers: fixed
capacity, allocated once, wait-free to push and pop. There is no timeline yet
to schedule from, so today the synth thread's input is a single debug note
on/off request; the queue is the extension point later tickets (#6 live
MIDI-through, #8 recording/playback) feed a real schedule through, without
restructuring the thread split.

**Undo log** — how undo is implemented: a bounded ring of at least 50 applied
commands, each paired with its inverse. `Undo` applies the most recent inverse
and moves that entry to a redo log; `Redo` reapplies it and moves it back.
`Undo` and `Redo` are never themselves logged. Applying a fresh command after
an undo discards the redo log — there is no branching timeline. `Undo`/`Redo`
against an empty log are no-ops that report an `Effect` (`NothingToUndo` /
`NothingToRedo`) rather than erroring, so the shell can, say, disable the undo
button. The log is a sequence of commands, not state snapshots, and it can be
cleared outright (`DawCore::clear_history`) — a later "switch projects" ticket
does this, per the spec's "undo history cleared when I switch projects".

## Shape of the system

```
gesture -> Command -> DawCore -> (ProjectState, Vec<Effect>) -> shell -> pixels/sound
```

`DawCore` (in `core/`) is a pure in-memory state machine and holds all musical
logic, including the sequencer. The shell (`src-tauri/`) owns the ports and the
window. The webview holds no musical logic: every pixel is derived from
`ProjectState`.
