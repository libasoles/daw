/**
 * The frontend is a presentation layer only. It turns gestures into commands
 * and renders `ProjectState`; it holds no musical logic, and it never computes
 * anything a test in `daw-core` could have asserted on.
 *
 * Issue #3 wires the command bridge end to end with the simplest possible
 * command: a tempo control. Changing the BPM sends `Command::SetBpm`, the
 * core returns the new `ProjectState`, and this module renders it back.
 * `Cmd+Z` / `Cmd+Shift+Z` prove the same round trip is undoable. The three
 * real regions (recording area, library, timeline) follow in H1-H3.
 *
 * The debug "Play test note" button still bypasses `Command`/`DawCore`, but
 * the global instrument and reverb controls are commands, so it audibly uses
 * the same project state future live and timeline trigger paths will use.
 */

import {
  applyCommand,
  fetchProjectState,
  listMidiDevices,
  playTestNote,
  selectMidiDevice,
  stopRecording,
  type MidiStatus,
  type ProjectState,
  type Quantisation,
} from "./core";

/** Three stacked blocks on a timeline — the shape the whole app is about. */
const mark = /* html */ `
  <svg class="placeholder__mark" viewBox="0 0 48 48" fill="none" aria-hidden="true">
    <rect x="4" y="9" width="26" height="8" rx="3" fill="currentColor" />
    <rect x="4" y="20" width="40" height="8" rx="3" fill="currentColor" opacity="0.66" />
    <rect x="4" y="31" width="16" height="8" rx="3" fill="currentColor" opacity="0.4" />
  </svg>
`;

const BPM_MIN = 20;
const BPM_MAX = 300;
const BPM_STEP = 1;
const REVERB_MIN = 0;
const REVERB_MAX = 100;
const PIANO = 0;
const ACCORDION = 1;

function takeEndPulse(state: ProjectState): number {
  return state.take?.raw_notes.reduce((end, note) => Math.max(end, note.end_pulse), 0) ?? 0;
}

function takeGrid(state: ProjectState): string {
  const take = state.take;
  if (!take) return "";
  const end = takeEndPulse(state);
  const width = Math.max(end, 1) * 32;
  const lines = Array.from({ length: end + 1 }, (_, pulse) =>
    `<line x1="${pulse * 32}" y1="0" x2="${pulse * 32}" y2="88" />`,
  ).join("");
  const notes = take.notes.map((note) =>
    `<rect x="${note.start_pulse * 32}" y="${72 - note.pitch % 5 * 12}" width="${Math.max(2, (note.end_pulse - note.start_pulse) * 32)}" height="9" rx="2" />`,
  ).join("");
  return `<div class="take-editor"><svg viewBox="0 0 ${width} 88" aria-label="Take notes against the pulse grid"><g class="take-grid">${lines}</g><g class="take-notes">${notes}</g></svg><div class="take-trim-handles" aria-label="Trim handles"><input class="take-trim-handle take-trim-handle--start" type="range" data-trim-start min="0" max="${end}" step="1" value="${take.trim.start_pulse}" aria-label="Trim start" /><input class="take-trim-handle take-trim-handle--end" type="range" data-trim-end min="0" max="${end}" step="1" value="${take.trim.end_pulse}" aria-label="Trim end" /></div></div>`;
}

function render(root: HTMLElement, state: ProjectState): void {
  root.innerHTML = /* html */ `
    <section class="placeholder">
      ${mark}
      <h1 class="placeholder__name">daw</h1>
      <p class="placeholder__tagline">
        Record takes, keep the good ones, arrange them on a timeline.
      </p>
      <div class="tempo" role="group" aria-label="Tempo">
        <button class="tempo__step" type="button" data-tempo-step="-1" aria-label="Decrease tempo">&minus;</button>
        <label class="tempo__readout">
          <input
            class="tempo__input"
            type="number"
            inputmode="numeric"
            min="${BPM_MIN}"
            max="${BPM_MAX}"
            step="${BPM_STEP}"
            value="${state.bpm}"
            aria-label="Tempo in beats per minute"
          />
          <span class="tempo__unit">bpm</span>
        </label>
        <button class="tempo__step" type="button" data-tempo-step="1" aria-label="Increase tempo">+</button>
      </div>
      <div class="pulse-controls" aria-label="Pulse controls">
        <label class="sound-control">
          <span class="sound-control__label">Time signature</span>
          <span class="time-signature">
            <select class="sound-control__select" data-beats-per-bar aria-label="Beats per bar">
              <option value="2"${state.time_signature[0] === 2 ? " selected" : ""}>2</option>
              <option value="3"${state.time_signature[0] === 3 ? " selected" : ""}>3</option>
              <option value="4"${state.time_signature[0] === 4 ? " selected" : ""}>4</option>
              <option value="5"${state.time_signature[0] === 5 ? " selected" : ""}>5</option>
              <option value="6"${state.time_signature[0] === 6 ? " selected" : ""}>6</option>
              <option value="7"${state.time_signature[0] === 7 ? " selected" : ""}>7</option>
            </select>
            <span aria-hidden="true">/</span>
            <select class="sound-control__select" data-beat-unit aria-label="Beat unit">
              <option value="2"${state.time_signature[1] === 2 ? " selected" : ""}>2</option>
              <option value="4"${state.time_signature[1] === 4 ? " selected" : ""}>4</option>
              <option value="8"${state.time_signature[1] === 8 ? " selected" : ""}>8</option>
              <option value="16"${state.time_signature[1] === 16 ? " selected" : ""}>16</option>
            </select>
          </span>
        </label>
        <label class="pulse-toggle">
          <input type="checkbox" data-metronome${state.metronome_enabled ? " checked" : ""} />
          <span>Metronome</span>
        </label>
        <label class="pulse-toggle">
          <input type="checkbox" data-count-in${state.count_in_enabled ? " checked" : ""} />
          <span>One-bar count-in</span>
        </label>
      </div>
      <div class="sound-controls" aria-label="Global sound controls">
        <label class="sound-control">
          <span class="sound-control__label">Instrument</span>
          <select class="sound-control__select" data-instrument aria-label="Global instrument">
            <option value="${PIANO}"${state.instrument === PIANO ? " selected" : ""}>Piano</option>
            <option value="${ACCORDION}"${state.instrument === ACCORDION ? " selected" : ""}>Accordion</option>
          </select>
        </label>
        <label class="sound-control sound-control--reverb">
          <span class="sound-control__label">Reverb <output data-reverb-value>${state.reverb}%</output></span>
          <input
            class="sound-control__range"
            data-reverb
            type="range"
            min="${REVERB_MIN}"
            max="${REVERB_MAX}"
            value="${state.reverb}"
            aria-label="Global reverb"
          />
        </label>
      </div>
      <div class="recording-area" aria-label="Recording area">
        <button
          class="record-button${state.is_recording ? " record-button--active" : ""}"
          type="button"
          data-record
          aria-pressed="${state.is_recording}"
          ${state.is_playing ? "disabled" : ""}
        >
          ${state.is_recording ? "Stop (R)" : "Record (R)"}
        </button>
        ${
          state.take
            ? /* html */ `
              <button class="take-play" type="button" data-play-take ${state.is_playing ? "disabled" : ""}>
                Play take
              </button>
              <span class="take-summary">${state.take.notes.length} note${state.take.notes.length === 1 ? "" : "s"}</span>
              <button type="button" data-add-to-library>Add to library</button>
              ${takeGrid(state)}
              <div class="take-edit-controls" aria-label="Take editing">
                <label>Quantise <select data-quantisation aria-label="Quantisation">
                  ${(["off", "whole", "half", "quarter", "eighth"] as Quantisation[]).map((value) => `<option value="${value}"${state.take?.quantisation === value ? " selected" : ""}>${value}</option>`).join("")}
                </select></label>
                <button type="button" data-apply-quantisation>Apply</button>
              </div>
            `
            : /* html */ `<span class="take-summary take-summary--empty">no take yet</span>`
        }
      </div>
      <aside class="library" aria-label="Library">
        <h2>Library</h2>
        ${state.blocks.map((block, index) => `<div class="library-block" style="--block-color: ${block.color}">${block.name}<button type="button" data-play-block="${index}"${state.is_playing ? " disabled" : ""}>Play</button></div>`).join("") || "<span class=\"take-summary take-summary--empty\">no blocks yet</span>"}
      </aside>
      <div class="midi-control" aria-label="MIDI input">
        <label class="sound-control">
          <span class="sound-control__label">MIDI input</span>
          <select class="sound-control__select" data-midi-input aria-label="MIDI input" disabled>
            <option>Checking MIDI inputs…</option>
          </select>
        </label>
        <p class="midi-control__status" data-midi-status aria-live="polite">Checking MIDI inputs…</p>
      </div>
      <button class="test-note" type="button" data-play-test-note>
        Play test note
      </button>
      <p class="placeholder__status" data-audio-status>no project &mdash; nothing to play yet</p>
    </section>
  `;
}

function clampBpm(bpm: number): number {
  return Math.min(BPM_MAX, Math.max(BPM_MIN, Math.round(bpm)));
}

async function setBpm(root: HTMLElement, bpm: number): Promise<void> {
  const applied = await applyCommand({ type: "setBpm", payload: clampBpm(bpm) });
  render(root, applied.state);
}

async function setInstrument(root: HTMLElement, instrument: number): Promise<void> {
  const applied = await applyCommand({ type: "setInstrument", payload: instrument });
  render(root, applied.state);
}

async function setTimeSignature(
  root: HTMLElement,
  beatsPerBar: number,
  beatUnit: number,
): Promise<void> {
  const applied = await applyCommand({
    type: "setTimeSignature",
    payload: { beats_per_bar: beatsPerBar, beat_unit: beatUnit },
  });
  render(root, applied.state);
}

async function setMetronomeEnabled(root: HTMLElement, enabled: boolean): Promise<void> {
  const applied = await applyCommand({ type: "setMetronomeEnabled", payload: enabled });
  render(root, applied.state);
}

async function setCountInEnabled(root: HTMLElement, enabled: boolean): Promise<void> {
  const applied = await applyCommand({ type: "setCountInEnabled", payload: enabled });
  render(root, applied.state);
}

async function setReverb(root: HTMLElement, reverb: number): Promise<void> {
  const value = Math.min(REVERB_MAX, Math.max(REVERB_MIN, Math.round(reverb)));
  // Deliberately does not call `render`: a full re-render replaces the
  // `<input type="range">` element, which would sever the browser's native
  // drag gesture on every `input` event and make the knob unusable while
  // dragging. The readout is updated directly instead; the slider's own
  // value is already authoritative from the user's gesture.
  const output = root.querySelector<HTMLOutputElement>("[data-reverb-value]");
  if (output) {
    output.textContent = `${value}%`;
  }
  await applyCommand({ type: "setReverb", payload: value });
}

/**
 * Arms recording. If the recording area already holds a take, the core
 * reports `confirmOverwriteRecording` and leaves recording off; this asks
 * the user to confirm, then resends with `force: true` if they agree, per
 * the spec's "recording over a take that has not been added to the library
 * asks for confirmation first."
 */
async function startRecording(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "startRecording", payload: { force: false } });
  const needsConfirmation = applied.effects.some(
    (effect) => effect.type === "confirmOverwriteRecording",
  );
  if (needsConfirmation) {
    render(root, applied.state);
    if (!window.confirm("Recording will replace the current take. Continue?")) {
      return;
    }
    const forced = await applyCommand({ type: "startRecording", payload: { force: true } });
    render(root, forced.state);
    return;
  }
  render(root, applied.state);
}

async function endRecording(root: HTMLElement): Promise<void> {
  const applied = await stopRecording();
  render(root, applied.state);
}

async function playTake(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "playTake" });
  render(root, applied.state);
}

async function setTakeTrim(root: HTMLElement, startPulse: number, endPulse: number): Promise<void> {
  const applied = await applyCommand({ type: "setTakeTrim", payload: { start_pulse: startPulse, end_pulse: endPulse } });
  render(root, applied.state);
}

async function applyTakeQuantisation(root: HTMLElement): Promise<void> {
  const select = root.querySelector<HTMLSelectElement>("[data-quantisation]");
  if (!select) return;
  const applied = await applyCommand({ type: "setTakeQuantisation", payload: select.value as Quantisation });
  render(root, applied.state);
}

async function addTakeToLibrary(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "addTakeToLibrary" });
  render(root, applied.state);
}

async function playBlock(root: HTMLElement, index: number): Promise<void> {
  const applied = await applyCommand({ type: "playBlock", payload: index });
  render(root, applied.state);
}

async function undo(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "undo" });
  render(root, applied.state);
}

async function redo(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "redo" });
  render(root, applied.state);
}

function wireTempoControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-tempo-step]",
    );
    if (!button) return;
    const input = root.querySelector<HTMLInputElement>(".tempo__input");
    const current = input ? Number(input.value) : 0;
    const step = Number(button.dataset.tempoStep);
    void setBpm(root, current + step);
  });

  root.addEventListener("change", (event) => {
    const input = event.target as HTMLElement;
    if (!input.matches(".tempo__input")) return;
    void setBpm(root, Number((input as HTMLInputElement).value));
  });
}

function wireSoundControls(root: HTMLElement): void {
  root.addEventListener("change", (event) => {
    const input = event.target as HTMLElement;
    if (input.matches("[data-instrument]")) {
      void setInstrument(root, Number((input as HTMLSelectElement).value));
    }
    if (input.matches("[data-beats-per-bar], [data-beat-unit]")) {
      const beatsPerBar = root.querySelector<HTMLSelectElement>("[data-beats-per-bar]");
      const beatUnit = root.querySelector<HTMLSelectElement>("[data-beat-unit]");
      if (beatsPerBar && beatUnit) {
        void setTimeSignature(root, Number(beatsPerBar.value), Number(beatUnit.value));
      }
    }
    if (input.matches("[data-metronome]")) {
      void setMetronomeEnabled(root, (input as HTMLInputElement).checked);
    }
    if (input.matches("[data-count-in]")) {
      void setCountInEnabled(root, (input as HTMLInputElement).checked);
    }
  });

  root.addEventListener("input", (event) => {
    const input = event.target as HTMLElement;
    if (input.matches("[data-reverb]")) {
      void setReverb(root, Number((input as HTMLInputElement).value));
    }
  });
}

function showAudioStatus(root: HTMLElement, message: string): void {
  const status = root.querySelector<HTMLElement>("[data-audio-status]");
  if (status) {
    status.textContent = message;
  }
}

function wireTestNoteButton(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-play-test-note]",
    );
    if (!button) return;

    playTestNote()
      .then(() => showAudioStatus(root, "played a test note"))
      .catch((error: unknown) => showAudioStatus(root, `could not play a test note: ${String(error)}`));
  });
}

function renderMidiStatus(root: HTMLElement, status: MidiStatus): void {
  const select = root.querySelector<HTMLSelectElement>("[data-midi-input]");
  const message = root.querySelector<HTMLElement>("[data-midi-status]");
  if (message) message.textContent = status.message;
  if (!select) return;

  select.replaceChildren();
  if (status.devices.length === 0) {
    const option = new Option("No MIDI input available", "");
    option.selected = true;
    select.add(option);
    select.disabled = true;
    return;
  }
  if (!status.selectedDeviceId) {
    const option = new Option("Choose a MIDI input…", "");
    option.disabled = true;
    option.selected = true;
    select.add(option);
  }
  for (const device of status.devices) {
    const option = new Option(device.name, device.id, false, device.id === status.selectedDeviceId);
    select.add(option);
  }
  select.disabled = false;
}

async function refreshMidiDevices(root: HTMLElement): Promise<void> {
  try {
    renderMidiStatus(root, await listMidiDevices());
  } catch (error: unknown) {
    const status = root.querySelector<HTMLElement>("[data-midi-status]");
    if (status) status.textContent = `Could not check MIDI inputs: ${String(error)}`;
  }
}

function wireMidiPicker(root: HTMLElement): void {
  root.addEventListener("change", (event) => {
    const select = event.target as HTMLElement;
    if (!select.matches("[data-midi-input]")) return;
    const deviceId = (select as HTMLSelectElement).value;
    if (!deviceId) return;
    selectMidiDevice(deviceId)
      .then((status) => renderMidiStatus(root, status))
      .catch((error: unknown) => refreshMidiDevices(root).then(() => {
        const status = root.querySelector<HTMLElement>("[data-midi-status]");
        if (status) status.textContent = `Could not select MIDI input: ${String(error)}`;
      }));
  });
}

function wireUndoRedoKeys(root: HTMLElement): void {
  window.addEventListener("keydown", (event) => {
    if (!event.metaKey || event.key.toLowerCase() !== "z") return;
    event.preventDefault();
    if (event.shiftKey) {
      void redo(root);
    } else {
      void undo(root);
    }
  });
}

/** Whether recording is currently armed, read from the record button's own
 * `aria-pressed` rather than tracked separately — the rendered DOM is
 * already the single source of truth for what the last `Applied` said. */
function isRecording(root: HTMLElement): boolean {
  const button = root.querySelector<HTMLButtonElement>("[data-record]");
  return button?.getAttribute("aria-pressed") === "true";
}

function toggleRecording(root: HTMLElement): void {
  const button = root.querySelector<HTMLButtonElement>("[data-record]");
  if (button?.disabled) return;
  if (isRecording(root)) {
    void endRecording(root);
  } else {
    void startRecording(root);
  }
}

function wireRecordingControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-record]")) {
      toggleRecording(root);
    }
    if (target.closest("[data-play-take]")) {
      void playTake(root);
    }
    if (target.closest("[data-apply-quantisation]")) {
      void applyTakeQuantisation(root);
    }
    if (target.closest("[data-add-to-library]")) {
      void addTakeToLibrary(root);
    }
    const blockButton = target.closest<HTMLButtonElement>("[data-play-block]");
    if (blockButton) void playBlock(root, Number(blockButton.dataset.playBlock));
  });

  root.addEventListener("change", (event) => {
    const input = event.target as HTMLInputElement;
    if (!input.matches("[data-trim-start], [data-trim-end]")) return;
    const start = root.querySelector<HTMLInputElement>("[data-trim-start]");
    const end = root.querySelector<HTMLInputElement>("[data-trim-end]");
    if (start && end) void setTakeTrim(root, Number(start.value), Number(end.value));
  });

  window.addEventListener("keydown", (event) => {
    if (event.key.toLowerCase() !== "r") return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const active = document.activeElement;
    // Never hijack `r` while the user is typing in a field.
    if (active instanceof HTMLInputElement || active instanceof HTMLSelectElement) return;
    event.preventDefault();
    toggleRecording(root);
  });
}

async function main(): Promise<void> {
  const root = document.querySelector<HTMLElement>("#app");
  if (!root) {
    throw new Error("Missing #app mount point in index.html");
  }

  const state = await fetchProjectState();
  render(root, state);
  wireTempoControls(root);
  wireSoundControls(root);
  wireRecordingControls(root);
  wireTestNoteButton(root);
  wireMidiPicker(root);
  wireUndoRedoKeys(root);
  await refreshMidiDevices(root);
  window.setInterval(() => void refreshMidiDevices(root), 1_000);
}

void main();
