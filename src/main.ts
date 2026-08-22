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
  type MidiStatus,
  type ProjectState,
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

async function main(): Promise<void> {
  const root = document.querySelector<HTMLElement>("#app");
  if (!root) {
    throw new Error("Missing #app mount point in index.html");
  }

  const state = await fetchProjectState();
  render(root, state);
  wireTempoControls(root);
  wireSoundControls(root);
  wireTestNoteButton(root);
  wireMidiPicker(root);
  wireUndoRedoKeys(root);
  await refreshMidiDevices(root);
  window.setInterval(() => void refreshMidiDevices(root), 1_000);
}

void main();
