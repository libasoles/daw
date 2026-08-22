/**
 * The bridge to `daw-core`, across the Tauri IPC boundary. This module owns
 * no state and does no rendering — it is only the thinnest possible mirror of
 * `core/src/lib.rs`'s `Command`, `ProjectState`, `Effect` and `Applied`
 * types, pinned by the wire-format tests in `core/tests/wire_format.rs`. If
 * that file's JSON shape changes, this is the other half that must change
 * with it.
 */

import { invoke } from "@tauri-apps/api/core";

export interface ProjectState {
  bpm: number;
  time_signature: [number, number];
}

export type Command =
  | { type: "newProject" }
  | { type: "setBpm"; payload: number }
  | {
      type: "setTimeSignature";
      payload: { beats_per_bar: number; beat_unit: number };
    }
  | { type: "undo" }
  | { type: "redo" };

export type Effect = { type: "nothingToUndo" } | { type: "nothingToRedo" };

export interface Applied {
  state: ProjectState;
  effects: Effect[];
}

/** The core's single entry point: a command goes in, the new state and any effects come out. */
export function applyCommand(command: Command): Promise<Applied> {
  return invoke<Applied>("apply_command", { command });
}

/** The project state as it stands right now, to render on first load. */
export function fetchProjectState(): Promise<ProjectState> {
  return invoke<ProjectState>("project_state");
}

/**
 * Sounds a single fixed note through the bundled SoundFont, for manual
 * verification that audio reaches the speakers (issue #4). There is no MIDI
 * input (#6) or instrument choice (#5) yet, so this is a deliberately
 * temporary debug trigger, not a feature described anywhere in the spec.
 * Rejects if no audio output device is available.
 */
export function playTestNote(): Promise<void> {
  return invoke<void>("play_test_note");
}
