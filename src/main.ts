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
  fetchAudioStatus,
  fetchProjectState,
  finishClose,
  listProjects,
  listMidiDevices,
  openProject,
  playTestNote,
  saveProject,
  selectMidiDevice,
  stopRecording,
  type Block,
  type MidiStatus,
  type ProjectState,
  type Quantisation,
} from "./core";
import { GM_CATEGORIES, GM_INSTRUMENTS } from "./gmInstruments";
import { listen } from "@tauri-apps/api/event";

/** Three stacked blocks on a timeline — the shape the whole app is about.
 * Shown only on the boot splash; the working UI never repeats the mark. */
const mark = /* html */ `
  <svg class="splash__mark" viewBox="0 0 48 48" fill="none" aria-hidden="true">
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
let projects: string[] = [];
let activeProjectName: string | null = null;
let isNamingProject = false;
let renderedProjectState: ProjectState | null = null;
let confirmRecordingOverwrite = false;
let editingBlockId: number | null = null;
let libraryCollapsed = false;
let inspectorCollapsed = false;
let projectMenuOpen = false;
let midiStatus: MidiStatus | null = null;
type PendingProjectAction = { type: "new" } | { type: "open"; name: string } | { type: "quit" };
let pendingProjectAction: PendingProjectAction | null = null;
let pendingSaveNeedsName = false;

function escapeHtml(value: string): string {
  return value.replace(/[&<>'"]/g, (character) => `&#${character.charCodeAt(0)};`);
}

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
  const startPct = end > 0 ? (take.trim.start_pulse / end) * 100 : 0;
  const endPct = end > 0 ? (take.trim.end_pulse / end) * 100 : 0;
  return `<div class="take-editor"><svg viewBox="0 0 ${width} 88" aria-label="Take notes against the pulse grid"><g class="take-grid">${lines}</g><g class="take-notes">${notes}</g></svg><div class="take-trim-handles" aria-label="Trim handles"><input class="take-trim-handle take-trim-handle--start" type="range" data-trim-start min="0" max="${end}" step="1" value="${take.trim.start_pulse}" style="--pct: ${startPct}%" aria-label="Trim start" /><input class="take-trim-handle take-trim-handle--end" type="range" data-trim-end min="0" max="${end}" step="1" value="${take.trim.end_pulse}" style="--pct: ${endPct}%" aria-label="Trim end" /></div></div>`;
}

/** Shown once at boot, while the first `fetchProjectState()` is in flight.
 * The mark/name/tagline never appear again once the working UI renders. */
function renderSplash(root: HTMLElement): void {
  root.innerHTML = /* html */ `
    <section class="splash">
      ${mark}
      <h1 class="splash__name">daw</h1>
      <p class="splash__tagline">
        Record takes, keep the good ones, arrange them on a timeline.
      </p>
    </section>
  `;
}

function projectControlsPanel(state: ProjectState): string {
  return /* html */ `
    <section class="project-controls" aria-label="Projects">
      <div class="project-controls__actions">
        <button type="button" data-new-project>New project</button>
        <button type="button" data-save-project ${state.is_dirty ? "" : "disabled"}>Save</button>
        <span class="project-controls__status">${state.is_dirty ? "Unsaved changes" : "All changes saved"}</span>
      </div>
      ${isNamingProject ? /* html */ `
        <form class="project-controls__name" data-project-name-form>
          <label>Project name <input data-project-name required autocomplete="off" /></label>
          <button type="submit">Save project</button>
          <button type="button" data-cancel-project-name>Cancel</button>
        </form>
      ` : ""}
      <div class="project-controls__list">
        <span>Projects</span>
        ${projects.length ? projects.map((name) => `<button type="button" data-open-project="${escapeHtml(name)}">${escapeHtml(name)}</button>`).join("") : "<span class=\"project-controls__empty\">No saved projects yet</span>"}
      </div>
    </section>
  `;
}

/** The merged transport bar: it is both the project menu and the only
 * `data-tauri-drag-region` in the app, so it must stay mostly empty. */
function topbar(state: ProjectState): string {
  const menuOpen = projectMenuOpen || isNamingProject;
  return /* html */ `
    <header class="topbar" data-tauri-drag-region>
      <div class="project-menu-wrap">
        <button class="project-menu" type="button" data-project-menu-toggle aria-expanded="${menuOpen}">
          <span class="project-menu__dot${state.is_dirty ? " is-dirty" : ""}"></span>
          <span class="project-menu__label">${activeProjectName ? escapeHtml(activeProjectName) : "Unsaved project"}</span>
          <span class="project-menu__caret">&#9662;</span>
        </button>
        ${menuOpen ? projectControlsPanel(state) : ""}
      </div>
      <div class="topbar-spacer" data-tauri-drag-region></div>
      <div class="transport">
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
        <button
          class="record-button${state.is_recording ? " record-button--active" : ""}"
          type="button"
          data-record
          aria-pressed="${state.is_recording}"
          ${state.is_playing ? "disabled" : ""}
        >
          ${state.is_recording ? "Stop (R)" : "Record (R)"}
        </button>
      </div>
    </header>
  `;
}

function libraryBlock(state: ProjectState, block: Block, index: number): string {
  return /* html */ `
    <div class="library-block" style="--block-color: ${block.color}">
      <span class="library-block__swatch"></span>
      ${editingBlockId === block.id
        ? `<label>Block name <input data-edit-block-name="${block.id}" value="${escapeHtml(block.name)}" /></label><button type="button" data-save-block-name="${block.id}">Save</button><button type="button" data-cancel-block-name>Cancel</button>`
        : `<span class="library-block__name">${escapeHtml(block.name)}</span><button type="button" data-edit-block-name-button="${block.id}">Rename</button>`}
      <input data-block-color="${block.id}" type="color" value="${block.color}" />
      <button type="button" data-delete-block="${block.id}">Delete</button>
      <button type="button" data-play-block="${index}"${state.is_playing ? " disabled" : ""}>Play</button>
    </div>
  `;
}

function libraryPane(state: ProjectState): string {
  return /* html */ `
    <aside class="library${libraryCollapsed ? " is-collapsed" : ""}" aria-label="Library">
      <div class="pane-header">
        <h2 class="pane-title">Library</h2>
        <button class="collapse-btn" type="button" data-toggle-library aria-label="${libraryCollapsed ? "Expand library" : "Collapse library"}">${libraryCollapsed ? "&rsaquo;" : "&lsaquo;"}</button>
      </div>
      ${state.blocks.length
        ? state.blocks.map((block, index) => libraryBlock(state, block, index)).join("")
        : `<span class="library-empty">no blocks yet</span>`}
    </aside>
  `;
}

/** A non-interactive preview of the library laid out in sequence. `daw-core`
 * has no arrangement/position data yet (that lands with the real timeline,
 * tracked as future H3 work) — this reserves the space and shows the blocks
 * that exist without pretending the ordering or editing is real yet. */
function timelinePreview(state: ProjectState): string {
  if (state.blocks.length === 0) {
    return /* html */ `
      <div class="timeline">
        <div class="timeline__head">
          <p class="pane-title">Timeline</p>
          <span class="timeline__note">preview</span>
        </div>
        <div class="timeline__empty">add blocks to the library to see them here</div>
      </div>
    `;
  }
  return /* html */ `
    <div class="timeline">
      <div class="timeline__head">
        <p class="pane-title">Timeline</p>
        <span class="timeline__note">preview — arranging blocks arrives in a later iteration</span>
      </div>
      <div class="timeline__track">
        ${state.blocks.map((block) => `<div class="timeline__clip" style="--block-color: ${block.color}" title="${escapeHtml(block.name)}">${escapeHtml(block.name)}</div>`).join("")}
      </div>
    </div>
  `;
}

function canvasPane(state: ProjectState): string {
  return /* html */ `
    <section class="canvas" aria-label="Recording and arrangement">
      ${confirmRecordingOverwrite ? /* html */ `<div class="inline-confirmation" role="status">Recording will replace the current take. <button type="button" data-confirm-recording-overwrite>Replace</button><button type="button" data-cancel-recording-overwrite>Cancel</button></div>` : ""}

      ${timelinePreview(state)}

      <hr class="divider" />

      <div class="canvas-head">
        <div>
          <h2>Current take</h2>
          <span class="meta">${state.take ? `${state.take.notes.length} note${state.take.notes.length === 1 ? "" : "s"}` : "no take yet"}</span>
        </div>
        ${state.take ? /* html */ `
          <div class="canvas-actions">
            <button class="take-play" type="button" data-play-take ${state.is_playing ? "disabled" : ""}>Play take</button>
            <select data-quantisation aria-label="Quantisation">
              ${(["off", "whole", "half", "quarter", "eighth"] as Quantisation[]).map((value) => `<option value="${value}"${state.take?.quantisation === value ? " selected" : ""}>${value}</option>`).join("")}
            </select>
            <button type="button" data-apply-quantisation>Apply</button>
            <button type="button" data-add-to-library>Add to library</button>
          </div>
        ` : ""}
      </div>

      ${state.take ? takeGrid(state) : `<span class="take-summary--empty">Press Record to capture a take</span>`}
    </section>
  `;
}

function inspectorPane(state: ProjectState): string {
  return /* html */ `
    <aside class="inspector${inspectorCollapsed ? " is-collapsed" : ""}" aria-label="Sound and input settings">
      <div class="pane-header">
        <button class="collapse-btn" type="button" data-toggle-inspector aria-label="${inspectorCollapsed ? "Expand inspector" : "Collapse inspector"}">${inspectorCollapsed ? "&lsaquo;" : "&rsaquo;"}</button>
      </div>
      <div class="inspector-rail">
        <span class="inspector-rail__chip" title="Sound">S</span>
        <span class="inspector-rail__chip" title="Pulse">P</span>
        <span class="inspector-rail__chip" title="MIDI">M</span>
      </div>

      <div class="inspector-section" aria-label="Global sound controls">
        <p class="pane-title">Sound</p>
        <label class="sound-control">
          <span class="sound-control__label">Instrument</span>
          <select class="sound-control__select" data-instrument aria-label="Global instrument">
            ${GM_CATEGORIES.map((category, index) => {
              const nextStart = GM_CATEGORIES[index + 1]?.firstProgram ?? GM_INSTRUMENTS.length;
              const options = GM_INSTRUMENTS.slice(category.firstProgram, nextStart)
                .map((name, offset) => {
                  const program = category.firstProgram + offset;
                  return `<option value="${program}"${state.instrument === program ? " selected" : ""}>${name}</option>`;
                })
                .join("");
              return `<optgroup label="${category.name}">${options}</optgroup>`;
            }).join("")}
          </select>
        </label>
        <label class="sound-control">
          <span class="sound-control__label">Reverb <output data-reverb-value>${state.reverb}%</output></span>
          <input
            class="sound-control__range"
            data-reverb
            type="range"
            min="${REVERB_MIN}"
            max="${REVERB_MAX}"
            value="${state.reverb}"
            style="--pct: ${((state.reverb - REVERB_MIN) / (REVERB_MAX - REVERB_MIN)) * 100}%"
            aria-label="Global reverb"
          />
        </label>
      </div>

      <hr class="divider" />

      <div class="inspector-section" aria-label="Pulse controls">
        <p class="pane-title">Pulse</p>
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

      <hr class="divider" />

      <div class="inspector-section" aria-label="MIDI input">
        <p class="pane-title">MIDI</p>
        <label class="sound-control">
          <span class="sound-control__label">MIDI input</span>
          ${midiSelectMarkup()}
        </label>
        <p class="midi-control__status" data-midi-status aria-live="polite">${midiStatus ? escapeHtml(midiStatus.message) : "Checking MIDI inputs…"}</p>
        <button class="test-note" type="button" data-play-test-note>Play test note</button>
      </div>
    </aside>
  `;
}

function midiSelectMarkup(): string {
  if (!midiStatus) {
    return `<select class="sound-control__select" data-midi-input aria-label="MIDI input" disabled><option>Checking MIDI inputs…</option></select>`;
  }
  if (midiStatus.devices.length === 0) {
    return `<select class="sound-control__select" data-midi-input aria-label="MIDI input" disabled><option selected>No MIDI input available</option></select>`;
  }
  const status = midiStatus;
  return `<select class="sound-control__select" data-midi-input aria-label="MIDI input">
    ${!status.selectedDeviceId ? `<option value="" disabled selected>Choose a MIDI input…</option>` : ""}
    ${status.devices.map((device) => `<option value="${device.id}"${device.id === status.selectedDeviceId ? " selected" : ""}>${escapeHtml(device.name)}</option>`).join("")}
  </select>`;
}

function statusBar(state: ProjectState): string {
  return /* html */ `
    <footer class="statusbar">
      <span data-audio-status>no project &mdash; nothing to play yet</span>
      <span>${state.is_dirty ? "unsaved changes" : "all changes saved"}</span>
    </footer>
  `;
}

function render(root: HTMLElement, state: ProjectState): void {
  renderedProjectState = state;
  root.innerHTML = /* html */ `
    <div class="app-shell">
      ${topbar(state)}
      <div
        class="body-grid"
        style="grid-template-columns: ${libraryCollapsed ? "var(--rail-w)" : "var(--library-w)"} 1fr ${inspectorCollapsed ? "var(--rail-w)" : "var(--inspector-w)"}"
      >
        ${libraryPane(state)}
        ${canvasPane(state)}
        ${inspectorPane(state)}
      </div>
      ${statusBar(state)}
    </div>
    ${pendingProjectAction ? /* html */ `
      <div class="unsaved-dialog" role="dialog" aria-modal="true" aria-label="Unsaved changes">
        <p>Save changes before continuing?</p>
        ${pendingSaveNeedsName ? `<label>Project name <input data-pending-project-name required autocomplete="off" /></label>` : ""}
        <button type="button" data-save-pending>Save and continue</button>
        <button type="button" data-discard-pending>Discard changes</button>
        <button type="button" data-cancel-pending>Cancel</button>
      </div>
    ` : ""}
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
  const range = root.querySelector<HTMLInputElement>("[data-reverb]");
  if (range) {
    range.style.setProperty("--pct", `${((value - REVERB_MIN) / (REVERB_MAX - REVERB_MIN)) * 100}%`);
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
    confirmRecordingOverwrite = true;
    render(root, applied.state);
    return;
  }
  render(root, applied.state);
}

async function confirmRecordingOverwriteAction(root: HTMLElement): Promise<void> {
  confirmRecordingOverwrite = false;
  const applied = await applyCommand({ type: "startRecording", payload: { force: true } });
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

async function refreshProjects(root: HTMLElement): Promise<void> {
  projects = await listProjects();
  const state = await fetchProjectState();
  render(root, state);
}

async function saveCurrentProject(root: HTMLElement, name?: string): Promise<void> {
  if (!renderedProjectState?.is_dirty) return;
  if (!activeProjectName && !name) {
    isNamingProject = true;
    projectMenuOpen = true;
    render(root, await fetchProjectState());
    root.querySelector<HTMLInputElement>("[data-project-name]")?.focus();
    return;
  }
  const state = await saveProject(name);
  activeProjectName = name ?? activeProjectName;
  isNamingProject = false;
  projects = await listProjects();
  render(root, state);
}

async function requestProjectAction(root: HTMLElement, action: PendingProjectAction): Promise<void> {
  const applied = action.type === "new"
    ? await applyCommand({ type: "requestNewProject" })
    : action.type === "quit"
      ? await applyCommand({ type: "requestQuit" })
      : await openProject(action.name);
  if (applied.effects.some((effect) => effect.type === "confirmDiscardProjectChanges")) {
    pendingProjectAction = action;
    pendingSaveNeedsName = false;
    projectMenuOpen = false;
    render(root, applied.state);
    return;
  }
  if (applied.effects.some((effect) => effect.type === "quitApplication")) {
    await finishClose();
    return;
  }
  if (action.type === "new") activeProjectName = null;
  if (action.type === "open") activeProjectName = action.name;
  render(root, applied.state);
}

async function savePendingProjectAction(root: HTMLElement): Promise<void> {
  const action = pendingProjectAction;
  if (!action) return;
  const nameInput = root.querySelector<HTMLInputElement>("[data-pending-project-name]");
  if (!activeProjectName && !nameInput) {
    pendingSaveNeedsName = true;
    render(root, renderedProjectState!);
    root.querySelector<HTMLInputElement>("[data-pending-project-name]")?.focus();
    return;
  }
  const name = nameInput?.value.trim();
  if (pendingSaveNeedsName && !name) return;
  await saveProject(name);
  activeProjectName = name ?? activeProjectName;
  projects = await listProjects();
  pendingProjectAction = null;
  pendingSaveNeedsName = false;
  const resolved = await applyCommand({ type: "resolveProjectChange", payload: "proceed" });
  if (resolved.effects.some((effect) => effect.type === "quitApplication")) {
    await finishClose();
    return;
  }
  if (action.type === "new") activeProjectName = null;
  if (action.type === "open") {
    activeProjectName = action.name;
    render(root, (await openProject(action.name)).state);
    return;
  }
  render(root, resolved.state);
}

function wireProjectControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-project-menu-toggle]")) {
      projectMenuOpen = !projectMenuOpen;
      if (renderedProjectState) render(root, renderedProjectState);
    }
    if (target.closest("[data-save-project]")) void saveCurrentProject(root);
    if (target.closest("[data-new-project]")) {
      projectMenuOpen = false;
      void requestProjectAction(root, { type: "new" });
    }
    if (target.closest("[data-cancel-project-name]")) {
      isNamingProject = false;
      void fetchProjectState().then((state) => render(root, state));
    }
    const open = target.closest<HTMLButtonElement>("[data-open-project]");
    if (open?.dataset.openProject) {
      projectMenuOpen = false;
      void requestProjectAction(root, { type: "open", name: open.dataset.openProject });
    }
    if (target.closest("[data-save-pending]")) void savePendingProjectAction(root);
    if (target.closest("[data-discard-pending]") && pendingProjectAction) {
      const action = pendingProjectAction;
      pendingProjectAction = null;
      pendingSaveNeedsName = false;
      void applyCommand({ type: "resolveProjectChange", payload: "proceed" }).then(async (applied) => {
        if (applied.effects.some((effect) => effect.type === "quitApplication")) return finishClose();
        if (action.type === "new") activeProjectName = null;
        if (action.type === "open") {
          activeProjectName = action.name;
          render(root, (await openProject(action.name)).state);
          return;
        }
        render(root, applied.state);
      });
    }
    if (target.closest("[data-cancel-pending]")) {
      void applyCommand({ type: "resolveProjectChange", payload: "cancel" });
      pendingProjectAction = null;
      pendingSaveNeedsName = false;
      if (renderedProjectState) render(root, renderedProjectState);
    }
  });

  root.addEventListener("submit", (event) => {
    const form = event.target as HTMLElement;
    if (!form.matches("[data-project-name-form]")) return;
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>("[data-project-name]");
    if (input?.value.trim()) void saveCurrentProject(root, input.value.trim());
  });

  window.addEventListener("keydown", (event) => {
    if (pendingProjectAction && event.key === "Tab") {
      event.preventDefault();
      root.querySelector<HTMLButtonElement>("[data-save-pending]")?.focus();
      return;
    }
    if (pendingProjectAction) return;
    if (!event.metaKey || event.key.toLowerCase() !== "s") return;
    event.preventDefault();
    void saveCurrentProject(root);
  });
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

  // Drag-to-adjust: up increases BPM, down decreases. A plain click (no
  // movement past the threshold) is left alone so it still focuses the
  // input for typing — only an armed drag calls preventDefault/setBpm.
  const PX_PER_BPM = 4;
  const DRAG_THRESHOLD_PX = 4;
  let dragState: { pointerId: number; startY: number; startBpm: number; armed: boolean } | null = null;

  root.addEventListener("pointerdown", (event) => {
    const readout = (event.target as HTMLElement).closest<HTMLElement>(".tempo__readout");
    const input = root.querySelector<HTMLInputElement>(".tempo__input");
    if (!readout || !input) return;
    dragState = { pointerId: event.pointerId, startY: event.clientY, startBpm: Number(input.value), armed: false };
  });

  root.addEventListener("pointermove", (event) => {
    if (!dragState || event.pointerId !== dragState.pointerId) return;
    const readout = root.querySelector<HTMLElement>(".tempo__readout");
    const input = root.querySelector<HTMLInputElement>(".tempo__input");
    if (!readout || !input) return;
    const deltaY = dragState.startY - event.clientY;
    if (!dragState.armed) {
      if (Math.abs(deltaY) < DRAG_THRESHOLD_PX) return;
      dragState.armed = true;
      readout.setPointerCapture(dragState.pointerId);
    }
    event.preventDefault();
    input.value = String(clampBpm(dragState.startBpm + Math.round(deltaY / PX_PER_BPM)));
  });

  const endTempoDrag = (event: PointerEvent) => {
    if (!dragState || event.pointerId !== dragState.pointerId) return;
    const wasArmed = dragState.armed;
    dragState = null;
    if (!wasArmed) return;
    const input = root.querySelector<HTMLInputElement>(".tempo__input");
    if (input) void setBpm(root, Number(input.value));
  };
  root.addEventListener("pointerup", endTempoDrag);
  root.addEventListener("pointercancel", endTempoDrag);
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

function showAudioStatus(root: HTMLElement, message: string, isError = false): void {
  const status = root.querySelector<HTMLElement>("[data-audio-status]");
  if (status) {
    status.textContent = message;
    status.classList.toggle("is-error", isError);
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
      .catch((error: unknown) => showAudioStatus(root, `could not play a test note: ${String(error)}`, true));
  });
}

// Rather than patching the MIDI `<select>` in the DOM out-of-band, this
// writes into the same `midiStatus` the normal `render()` reads (see
// `midiSelectMarkup`). Patching out-of-band used to get clobbered by the
// very next unrelated `render()` call — nearly every command handler
// triggers one — leaving the dropdown stuck on "Checking MIDI inputs…"
// until the next 1s poll tick repaired it.
function renderMidiStatus(root: HTMLElement, status: MidiStatus): void {
  const unchanged = midiStatus !== null
    && midiStatus.message === status.message
    && midiStatus.selectedDeviceId === status.selectedDeviceId
    && midiStatus.devices.length === status.devices.length
    && midiStatus.devices.every((device, index) => device.id === status.devices[index]?.id && device.name === status.devices[index]?.name);
  midiStatus = status;
  if (unchanged) return;
  if (renderedProjectState) render(root, renderedProjectState);
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
    if (pendingProjectAction) return;
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
    if (target.closest("[data-confirm-recording-overwrite]")) void confirmRecordingOverwriteAction(root);
    if (target.closest("[data-cancel-recording-overwrite]")) {
      confirmRecordingOverwrite = false;
      if (renderedProjectState) render(root, renderedProjectState);
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
    const deleteButton = target.closest<HTMLButtonElement>("[data-delete-block]");
    if (deleteButton) void applyCommand({ type: "deleteBlock", payload: Number(deleteButton.dataset.deleteBlock) }).then((applied) => render(root, applied.state));
    const editName = target.closest<HTMLButtonElement>("[data-edit-block-name-button]");
    if (editName?.dataset.editBlockNameButton) {
      editingBlockId = Number(editName.dataset.editBlockNameButton);
      if (renderedProjectState) render(root, renderedProjectState);
    }
    const saveName = target.closest<HTMLButtonElement>("[data-save-block-name]");
    if (saveName?.dataset.saveBlockName) {
      const id = Number(saveName.dataset.saveBlockName);
      const input = root.querySelector<HTMLInputElement>(`[data-edit-block-name="${id}"]`);
      if (input?.value.trim()) {
        editingBlockId = null;
        void applyCommand({ type: "renameBlock", payload: { id, name: input.value.trim() } }).then((applied) => render(root, applied.state));
      }
    }
    if (target.closest("[data-cancel-block-name]")) {
      editingBlockId = null;
      if (renderedProjectState) render(root, renderedProjectState);
    }
  });

  root.addEventListener("change", (event) => {
    const color = event.target as HTMLInputElement;
    if (!color.matches("[data-block-color]")) return;
    void applyCommand({ type: "recolourBlock", payload: { id: Number(color.dataset.blockColor), color: color.value } }).then((applied) => render(root, applied.state));
  });

  root.addEventListener("change", (event) => {
    const input = event.target as HTMLInputElement;
    if (!input.matches("[data-trim-start], [data-trim-end]")) return;
    const start = root.querySelector<HTMLInputElement>("[data-trim-start]");
    const end = root.querySelector<HTMLInputElement>("[data-trim-end]");
    if (start && end) void setTakeTrim(root, Number(start.value), Number(end.value));
  });

  window.addEventListener("keydown", (event) => {
    if (pendingProjectAction) return;
    if (event.key.toLowerCase() !== "r") return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const active = document.activeElement;
    // Never hijack `r` while the user is typing in a field.
    if (active instanceof HTMLInputElement || active instanceof HTMLSelectElement) return;
    event.preventDefault();
    toggleRecording(root);
  });
}

function wireLayoutControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-toggle-library]")) {
      libraryCollapsed = !libraryCollapsed;
      if (renderedProjectState) render(root, renderedProjectState);
    }
    if (target.closest("[data-toggle-inspector]")) {
      inspectorCollapsed = !inspectorCollapsed;
      if (renderedProjectState) render(root, renderedProjectState);
    }
  });
}

async function main(): Promise<void> {
  const root = document.querySelector<HTMLElement>("#app");
  if (!root) {
    throw new Error("Missing #app mount point in index.html");
  }

  renderSplash(root);

  const state = await fetchProjectState();
  render(root, state);

  // Every wiring call below is synchronous and must run unconditionally:
  // a single `await` that rejects (e.g. a missing IPC permission) must
  // never be able to abort the rest of `main()` and leave the UI
  // half-wired with no visible error, so nothing that can reject sits
  // between these calls.
  wireProjectControls(root);
  wireTempoControls(root);
  wireSoundControls(root);
  wireRecordingControls(root);
  wireTestNoteButton(root);
  wireMidiPicker(root);
  wireUndoRedoKeys(root);
  wireLayoutControls(root);

  void refreshProjects(root);

  listen("confirm-close", () => requestProjectAction(root, { type: "quit" })).catch((error: unknown) =>
    console.error("could not listen for confirm-close:", error),
  );

  void refreshMidiDevices(root);
  window.setInterval(() => void refreshMidiDevices(root), 1_000);

  fetchAudioStatus()
    .then((status) => {
      if (!status.available) showAudioStatus(root, status.message, true);
    })
    .catch((error: unknown) => showAudioStatus(root, `could not check audio output: ${String(error)}`, true));
}

void main();
