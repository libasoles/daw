/**
 * The bridge to `daw-core`, across the Tauri IPC boundary. This module owns
 * no state and does no rendering — it is only the thinnest possible mirror of
 * `core/src/lib.rs`'s `Command`, `ProjectState`, `Effect` and `Applied`
 * types, pinned by the wire-format tests in `core/tests/wire_format.rs`. If
 * that file's JSON shape changes, this is the other half that must change
 * with it.
 */

import { invoke } from "@tauri-apps/api/core";

export interface RecordedNote {
  pitch: number;
  velocity: number;
  start_pulse: number;
  end_pulse: number;
}

export interface Take {
  raw_notes: RecordedNote[];
  notes: RecordedNote[];
  trim: { start_pulse: number; end_pulse: number };
  quantisation: Quantisation;
}

export interface Block {
  id: number;
  name: string;
  color: string;
  instrument: number;
  notes: RecordedNote[];
}

/** A block placed on the timeline (issue #17): an independent copy of the
 * block's notes/name/colour/instrument, never a reference. */
export interface Placement {
  id: number;
  /** The block this placement was copied from (issue #23), kept only so
   * deleting that block can find every placement it produced. */
  block_id: number;
  track: number;
  start_pulse: number;
  name: string;
  color: string;
  instrument: number;
  notes: RecordedNote[];
  /** A deliberate silence, in pulses, held before this placement (issue
   * #22), set by dragging with `Cmd` held. Stored on the placement itself
   * so it survives insertions, reorders and deletions elsewhere. */
  gap_before: number;
}

export type Quantisation = "off" | "whole" | "half" | "quarter" | "eighth";

export interface ProjectState {
  is_dirty: boolean;
  bpm: number;
  time_signature: [number, number];
  instrument: number;
  reverb: number;
  metronome_enabled: boolean;
  count_in_enabled: boolean;
  take: Take | null;
  blocks: Block[];
  next_block_id: number;
  placements: Placement[];
  next_placement_id: number;
  loop_enabled: boolean;
  is_recording: boolean;
  is_playing: boolean;
}

export type Command =
  | { type: "newProject"; payload: { force: boolean } }
  | { type: "setBpm"; payload: number }
  | {
      type: "setTimeSignature";
      payload: { beats_per_bar: number; beat_unit: number };
    }
  | { type: "setInstrument"; payload: number }
  | { type: "setReverb"; payload: number }
  | { type: "setMetronomeEnabled"; payload: boolean }
  | { type: "setCountInEnabled"; payload: boolean }
  | { type: "setLoopEnabled"; payload: boolean }
  | { type: "startRecording"; payload: { force: boolean } }
  | { type: "setTakeTrim"; payload: { start_pulse: number; end_pulse: number } }
  | { type: "setTakeQuantisation"; payload: Quantisation }
  | { type: "addTakeToLibrary" }
  | { type: "playBlock"; payload: number }
  | { type: "addPlacement"; payload: { block_id: number; track: number } }
  | {
      type: "insertPlacementAt";
      payload: { block_id: number; track: number; index: number };
    }
  | { type: "reorderPlacement"; payload: { id: number; new_index: number } }
  | { type: "deletePlacement"; payload: number }
  | { type: "setPlacementGap"; payload: { id: number; gap_before: number } }
  | { type: "renameBlock"; payload: { id: number; name: string } }
  | { type: "recolourBlock"; payload: { id: number; color: string } }
  | { type: "deleteBlock"; payload: { id: number; force: boolean } }
  | { type: "playTake" }
  | { type: "playTimeline" }
  | { type: "undo" }
  | { type: "redo" };

export type Effect =
  | { type: "nothingToUndo" }
  | { type: "nothingToRedo" }
  | { type: "noMidiDeviceAvailable" }
  | { type: "confirmOverwriteRecording" }
  | { type: "confirmDiscardUnsavedChanges" }
  | { type: "confirmDeleteBlockInUse"; uses: number }
  | ({ type: "playSchedule" } & Take);

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

/** Saves through the shell's app-data storage, never a system file dialog. */
export function saveProject(requestedName?: string): Promise<ProjectState> {
  return invoke<ProjectState>("save_project", { requestedName });
}

export function listProjects(): Promise<string[]> {
  return invoke<string[]>("list_projects");
}

export function openProject(name: string, force = false): Promise<Applied> {
  return invoke<Applied>("open_project", { name, force });
}

/** A crash-recovery snapshot (issue #15): the project as it stood when it
 * was last written, roughly every ten seconds while unsaved changes
 * existed, plus the name it was saved under, if any. */
export interface RecoverySnapshot {
  project_name: string | null;
  document: ProjectState;
}

/** The recovery snapshot found at launch, if any. Reading it doesn't consume
 * it — only `resolveRecovery` does that, once the user has decided. */
export function fetchRecoverySnapshot(): Promise<RecoverySnapshot | null> {
  return invoke<RecoverySnapshot | null>("recovery_snapshot");
}

/** Accepts or declines the found recovery snapshot. Accepting restores the
 * session (still reporting unsaved changes, since a snapshot was never a
 * manual save); either way the snapshot itself is deleted. */
export function resolveRecovery(accept: boolean): Promise<Applied> {
  return invoke<Applied>("resolve_recovery", { accept });
}

/**
 * Finishes recording: the shell turns the raw MIDI it captured natively into
 * a `Take` and applies it as `Command::StopRecording`. This can't be a
 * regular `applyCommand` call — the webview never sees the raw MIDI stream,
 * only the shell does.
 */
export function stopRecording(): Promise<Applied> {
  return invoke<Applied>("stop_recording");
}

/**
 * Stops whatever is currently playing (issue #18): silences the audio
 * engine immediately rather than waiting for a schedule to finish on its
 * own. A no-op if nothing is playing.
 */
export function stopPlayback(): Promise<Applied> {
  return invoke<Applied>("stop_playback");
}

/** What happened when {@link exportMidi} was called. */
export type ExportOutcome =
  | { status: "exported" }
  | { status: "nothingToExport" }
  | { status: "cancelled" };

/**
 * Exports the timeline as a `.mid` file (issue #24). The shell asks
 * `daw-core` for the encoded bytes reflecting exactly what playback would
 * sound — trims, quantisation, deliberate gaps and tempo/time signature
 * all included — then opens a native "Save As" dialog and writes them
 * wherever the user chooses. This can't be a regular `applyCommand` call:
 * the native save dialog and the file write are shell-only capabilities
 * the webview has no access to.
 */
export function exportMidi(): Promise<ExportOutcome> {
  return invoke<ExportOutcome>("export_midi");
}

/**
 * Sounds a single fixed note through the bundled SoundFont, for manual
 * verification that audio reaches the speakers (issue #4). There is no MIDI
 * input (#6) yet, so this is a deliberately temporary debug trigger. It uses
 * the current global instrument and reverb selected in project state.
 * Rejects if no audio output device is available.
 */
export function playTestNote(): Promise<void> {
  return invoke<void>("play_test_note");
}

export interface MidiDevice {
  id: string;
  name: string;
}

export interface MidiStatus {
  devices: MidiDevice[];
  selectedDeviceId: string | null;
  message: string;
}

export function listMidiDevices(): Promise<MidiStatus> {
  return invoke<MidiStatus>("list_midi_devices");
}

export function selectMidiDevice(deviceId: string): Promise<MidiStatus> {
  return invoke<MidiStatus>("select_midi_device", { deviceId });
}
