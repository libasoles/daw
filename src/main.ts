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

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  applyCommand,
  exportMidi,
  fetchAudioStatus,
  fetchProjectState,
  fetchRecoverySnapshot,
  listProjects,
  listMidiDevices,
  openProject,
  playTestNote,
  resolveRecovery,
  saveProject,
  selectMidiDevice,
  stopPlayback,
  stopRecording,
  type Block,
  type Effect,
  type MidiStatus,
  type ProjectState,
  type Quantisation,
  type RecordedNote,
  type RecoverySnapshot,
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

/** Mirrors `core/src/lib.rs`'s `PULSES_PER_BEAT`. Stored notes are pulse
 * offsets, never beats or bars (CONTEXT.md's "Pulse"), so the timeline has
 * to know this conversion itself to draw a bar-accented grid; there's no
 * other source for it, since the core performs no rendering. */
const PULSES_PER_BEAT = 2;

/**
 * How many bars the timeline draws. There's no placement (#17) or project
 * "length" yet — a piece is just as long as its arrangement — so this is a
 * generous fixed span, wide enough to prove horizontal scrolling (#16's own
 * acceptance criterion) without needing that concept yet.
 */
const TIMELINE_BARS = 64;

type TimelineZoom = "overview" | "normal" | "close";
/** Three fixed zoom levels (#16): pixels of horizontal space per pulse.
 * This is a view setting only — never sent to the core, never part of
 * `ProjectState` — so it lives in this module-level variable, not a
 * command payload. */
const TIMELINE_ZOOM_PX_PER_PULSE: Record<TimelineZoom, number> = {
  overview: 4,
  normal: 12,
  close: 28,
};
let timelineZoom: TimelineZoom = "normal";
let projects: string[] = [];
let activeProjectName: string | null = null;
let isNamingProject = false;
let currentState: ProjectState | null = null;
let editingBlockId: number | null = null;
/** The placement selected on the timeline (issue #21), if any — a view
 * concern only, like `timelineZoom`, never part of `ProjectState`. `Delete`
 * acts on whichever placement this names. */
let selectedPlacementId: number | null = null;
let libraryCollapsed = false;
let inspectorCollapsed = false;
let projectMenuOpen = false;
let midiStatus: MidiStatus | null = null;
/** Notes captured so far in the take currently being recorded, updated live
 * from the backend's "live-note" event — see `wireLiveNoteFeed`. Cleared
 * whenever a recording starts or ends, since `state.take` only reflects the
 * finished take once recording stops. */
let liveNotes: RecordedNote[] = [];

/**
 * A "new project" / "switch project" / "quit" the user asked for while there
 * are unsaved changes. Set only long enough to show the confirm-discard
 * overlay (`render`'s one modal, per the spec's "this warning is the
 * application's only modal dialog") and resolve it; `null` the rest of the
 * time.
 */
type PendingProjectAction = { kind: "new" } | { kind: "open"; name: string } | { kind: "quit" };
let pendingProjectAction: PendingProjectAction | null = null;
/** Set only while the inline "name this project" form is standing in for the
 * unsaved-changes prompt's "Save" choice on a never-saved project, so the
 * pending action can resume once the name is submitted. */
let pendingActionAfterNaming: PendingProjectAction | null = null;
/** A crash-recovery snapshot (issue #15) found at launch, shown once as its
 * own modal asking whether to recover; `null` once resolved. */
let pendingRecovery: RecoverySnapshot | null = null;
/** Set only while this session itself started timeline playback (issue
 * #18), so the playhead shows for timeline playback and not for a take or
 * block's own "Play" button — `state.is_playing` alone doesn't say which
 * one is running, since all three share one flag. */
let timelinePlaybackActive = false;

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

/**
 * The timeline's pulse grid (issue #16): a line for every pulse, every
 * `pulsesPerBar`-th line accented so a long arrangement stays readable
 * without counting. The accent is purely visual — it comes from the time
 * signature but constrains nothing about where a block can later be
 * placed — and it never moves a grid line, only which lines are marked, so
 * changing the time signature re-accents in place rather than rescaling
 * anything drawn.
 */
function timelineGrid(state: ProjectState): string {
  const pulsesPerBar = state.time_signature[0] * PULSES_PER_BEAT;
  const totalPulses = TIMELINE_BARS * pulsesPerBar;
  const pxPerPulse = TIMELINE_ZOOM_PX_PER_PULSE[timelineZoom];
  const width = totalPulses * pxPerPulse;
  const lines = Array.from({ length: totalPulses + 1 }, (_, pulse) => {
    const accent = pulse % pulsesPerBar === 0;
    return `<line class="timeline-grid__line${accent ? " timeline-grid__line--accent" : ""}" x1="${pulse * pxPerPulse}" y1="0" x2="${pulse * pxPerPulse}" y2="120" />`;
  }).join("");
  return `<svg class="timeline-grid" width="${width}" height="120" viewBox="0 0 ${width} 120" aria-label="Timeline pulse grid">${lines}</svg>`;
}

/**
 * Placements dropped on track 0 (issue #17), drawn as coloured blocks over
 * the pulse grid at `start_pulse * pxPerPulse`. Each placement is an
 * independent copy of a block's notes/name/colour, so this reads only from
 * the placement itself — never back to `state.blocks` — exactly like a
 * deleted block (#23) would leave it.
 */
function timelinePlacements(state: ProjectState): string {
  const pxPerPulse = TIMELINE_ZOOM_PX_PER_PULSE[timelineZoom];
  return state.placements
    .filter((placement) => placement.track === 0)
    .map((placement) => {
      const length = placement.notes.reduce((end, note) => Math.max(end, note.end_pulse), 0);
      const left = placement.start_pulse * pxPerPulse;
      const width = Math.max(length * pxPerPulse, 2);
      const selected = placement.id === selectedPlacementId;
      return `<div class="timeline-placement${selected ? " timeline-placement--selected" : ""}" style="--placement-color: ${placement.color}; left: ${left}px; width: ${width}px;" data-placement="${placement.id}" draggable="true" data-drag-placement="${placement.id}" aria-selected="${selected}">${escapeHtml(placement.name)}</div>`;
    })
    .join("");
}

/** How many pulses the whole arrangement spans, across every track — the
 * same span `Command::PlayTimeline` turns into a note stream, so the
 * playhead's travel distance and duration match exactly what plays. */
function timelineTotalPulses(state: ProjectState): number {
  return state.placements.reduce((end, placement) => {
    const length = placement.notes.reduce((noteEnd, note) => Math.max(noteEnd, note.end_pulse), 0);
    return Math.max(end, placement.start_pulse + length);
  }, 0);
}

/** Mirrors `daw_core::pulse_elapsed_time`: pulses to milliseconds at `bpm`. */
function pulseElapsedMs(pulses: number, bpm: number): number {
  return bpm > 0 ? (pulses * 60_000) / (bpm * PULSES_PER_BEAT) : 0;
}

/**
 * A visible marker of playback position while the timeline plays (issue
 * #18). Its travel and duration are computed here, purely for animation —
 * the core's own timer (mirrored by `schedulePlaybackRefresh`) is still
 * what actually ends playback; this only has to look right for that same
 * span.
 */
function timelinePlayhead(state: ProjectState): string {
  if (!state.is_playing || !timelinePlaybackActive) return "";
  const pxPerPulse = TIMELINE_ZOOM_PX_PER_PULSE[timelineZoom];
  const distance = timelineTotalPulses(state) * pxPerPulse;
  const durationMs = pulseElapsedMs(timelineTotalPulses(state), state.bpm);
  // Looping (issue #19) repeats the same sweep indefinitely rather than
  // holding at the end of one pass.
  const iterationCount = state.loop_enabled ? "infinite" : "1";
  return `<div class="timeline-playhead" style="--playhead-distance: ${distance}px; animation-duration: ${durationMs}ms; animation-iteration-count: ${iterationCount};" aria-hidden="true"></div>`;
}

/** Notes as they're captured mid-recording, from `liveNotes` — same visual
 * language as `takeGrid` but no trim handles, since there is no finished
 * take yet to trim. The grid grows to fit the furthest note played so far. */
function liveTakeGrid(notes: RecordedNote[]): string {
  const end = notes.reduce((max, note) => Math.max(max, note.end_pulse), 0);
  const width = Math.max(end, 1) * 32;
  const lines = Array.from({ length: end + 1 }, (_, pulse) =>
    `<line x1="${pulse * 32}" y1="0" x2="${pulse * 32}" y2="88" />`,
  ).join("");
  const rects = notes.map((note) =>
    `<rect x="${note.start_pulse * 32}" y="${72 - note.pitch % 5 * 12}" width="${Math.max(2, (note.end_pulse - note.start_pulse) * 32)}" height="9" rx="2" />`,
  ).join("");
  return `<div class="take-editor take-editor--live"><svg viewBox="0 0 ${width} 88" aria-label="Notes captured so far"><g class="take-grid">${lines}</g><g class="take-notes take-notes--live">${rects}</g></svg></div>`;
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
        <button type="button" data-new-project>New project</button>
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
        <label class="pulse-toggle pulse-toggle--icon pulse-toggle--transport count-in-toggle" title="One-bar count-in">
          <input type="checkbox" data-count-in aria-label="One-bar count-in"${state.count_in_enabled ? " checked" : ""} />
          <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
            <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.7" />
            <text x="12" y="15.5" text-anchor="middle" fill="currentColor" font-family="system-ui, sans-serif" font-size="10" font-weight="700">123</text>
          </svg>
        </label>
        <label class="pulse-toggle pulse-toggle--icon pulse-toggle--transport metronome-toggle" title="Metronome">
          <input type="checkbox" data-metronome aria-label="Metronome"${state.metronome_enabled ? " checked" : ""} />
          <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><path d="M7.5 19h9L14.4 6H9.6L7.5 19zm4.5-9v5m-2 4h4M11 3h2" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </label>
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
      </div>
    </header>
  `;
}

function libraryBlock(state: ProjectState, block: Block, index: number): string {
  return /* html */ `
    <div class="library-block" style="--block-color: ${block.color}" draggable="true" data-drag-block="${block.id}">
      <span class="library-block__swatch"></span>
      ${block.id === editingBlockId
        ? `<input class="library-block__name-input" data-block-name-input="${block.id}" value="${escapeHtml(block.name)}" aria-label="Block name" />`
        : `<span class="library-block__name" data-block-name="${block.id}">${escapeHtml(block.name)}</span>`}
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

function timelinePane(state: ProjectState): string {
  return /* html */ `
    <section class="timeline" aria-label="Timeline">
      <div class="timeline__header">
        <h2>Timeline</h2>
        <button
          class="timeline__play"
          type="button"
          data-play-timeline
          ${
            (state.is_playing && !timelinePlaybackActive) ||
            (!state.is_playing && state.placements.length === 0)
              ? "disabled"
              : ""
          }
        >
          ${state.is_playing && timelinePlaybackActive ? "Stop (Space)" : "Play timeline (Space)"}
        </button>
        <button class="timeline__export" type="button" data-export-midi>Export .mid</button>
        <label class="pulse-toggle">
          <input type="checkbox" data-loop-enabled${state.loop_enabled ? " checked" : ""} />
          <span>Loop</span>
        </label>
        <div class="timeline__zoom" role="group" aria-label="Timeline zoom">
          <button type="button" data-timeline-zoom="overview" aria-pressed="${timelineZoom === "overview"}">Overview</button>
          <button type="button" data-timeline-zoom="normal" aria-pressed="${timelineZoom === "normal"}">Normal</button>
          <button type="button" data-timeline-zoom="close" aria-pressed="${timelineZoom === "close"}">Close</button>
        </div>
      </div>
      <div class="timeline__scroll" data-timeline-drop>
        <div class="timeline__track">
          ${timelineGrid(state)}
          <div class="timeline__placements">${timelinePlacements(state)}</div>
          ${timelinePlayhead(state)}
        </div>
      </div>
    </section>
  `;
}

function canvasPane(state: ProjectState): string {
  return /* html */ `
    <section class="canvas" aria-label="Recording and arrangement">
      ${timelinePane(state)}

      <hr class="divider" />

      <div class="canvas-head">
        <div>
          <h2>Current take</h2>
          <span class="meta">${(() => {
            const count = state.is_recording ? liveNotes.length : state.take?.notes.length;
            if (count === undefined) return "no take yet";
            return `${count} note${count === 1 ? "" : "s"}`;
          })()}</span>
        </div>
        <div class="canvas-actions">
          <button
            class="icon-button icon-button--lg record-button${state.is_recording ? " record-button--active" : ""}"
            type="button"
            data-record
            aria-pressed="${state.is_recording}"
            aria-label="${state.is_recording ? "Stop recording" : "Record"}"
            title="${state.is_recording ? "Stop (R)" : "Record (R)"}"
            ${state.is_playing ? "disabled" : ""}
          >
            ${state.is_recording
              ? `<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" /></svg>`
              : `<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><circle cx="12" cy="12" r="9" fill="currentColor" /></svg>`}
          </button>
          <button
            class="icon-button icon-button--lg play-icon"
            type="button"
            data-play-take
            aria-label="Play take"
            title="Play take"
            ${!state.take || state.is_playing ? "disabled" : ""}
          >
            <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M7 4l14 8-14 8V4z" fill="currentColor" /></svg>
          </button>
          ${state.take ? /* html */ `
            <select data-quantisation aria-label="Quantisation">
              ${(["off", "whole", "half", "quarter", "eighth"] as Quantisation[]).map((value) => `<option value="${value}"${state.take?.quantisation === value ? " selected" : ""}>${value}</option>`).join("")}
            </select>
            <button
              class="icon-button library-add-button"
              type="button"
              data-add-to-library
              aria-label="Add take to library"
              title="Add take to library"
            >
              <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M5 5.5A1.5 1.5 0 0 1 6.5 4h11A1.5 1.5 0 0 1 19 5.5v13a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 18.5v-13zM12 8v8m-4-4h8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
            </button>
          ` : ""}
        </div>
      </div>

      ${state.is_recording
        ? liveNotes.length
          ? liveTakeGrid(liveNotes)
          : `<span class="take-summary--empty">Listening for notes&hellip;</span>`
        : state.take
          ? takeGrid(state)
          : `<span class="take-summary--empty">Press Record to capture a take</span>`}
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
      </div>

      <hr class="divider" />

      <div class="inspector-section" aria-label="MIDI input">
        <div class="midi-section__header">
          <p class="pane-title">MIDI</p>
          ${midiConnectionMarkup()}
        </div>
        ${midiSelectMarkup()}
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

function midiConnectionMarkup(): string {
  const connected = midiStatus?.connected ?? false;
  const label = connected ? "MIDI input connected" : "MIDI input not connected";
  return `<span class="midi-control__connection${connected ? " is-connected" : ""}" data-midi-status role="status" aria-label="${label}" title="${label}"></span>`;
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
  currentState = state;
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
      <div class="modal-overlay" data-unsaved-changes-overlay>
        <div class="modal" role="alertdialog" aria-modal="true" aria-label="Unsaved changes">
          <p>You have unsaved changes. Save them before continuing?</p>
          <div class="modal__actions">
            <button type="button" data-unsaved-save>Save</button>
            <button type="button" data-unsaved-discard>Discard</button>
            <button type="button" data-unsaved-cancel>Cancel</button>
          </div>
        </div>
      </div>
    ` : ""}
    ${pendingRecovery ? /* html */ `
      <div class="modal-overlay" data-recovery-overlay>
        <div class="modal" role="alertdialog" aria-modal="true" aria-label="Recover interrupted session">
          <p>${
            pendingRecovery.project_name
              ? `An interrupted session of "${escapeHtml(pendingRecovery.project_name)}" was found. Recover it?`
              : "An interrupted session was found. Recover it?"
          }</p>
          <div class="modal__actions">
            <button type="button" data-recovery-accept>Recover</button>
            <button type="button" data-recovery-decline>Discard</button>
          </div>
        </div>
      </div>
    ` : ""}
  `;
  if (editingBlockId !== null) {
    const input = root.querySelector<HTMLInputElement>(
      `[data-block-name-input="${editingBlockId}"]`,
    );
    input?.focus();
    input?.select();
  }
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
    render(root, applied.state);
    if (!window.confirm("Recording will replace the current take. Continue?")) {
      return;
    }
    const forced = await applyCommand({ type: "startRecording", payload: { force: true } });
    render(root, forced.state);
    return;
  }
  liveNotes = [];
  render(root, applied.state);
}

async function endRecording(root: HTMLElement): Promise<void> {
  const applied = await stopRecording();
  liveNotes = [];
  render(root, applied.state);
}

/**
 * Deletes a library block (issue #23) — the application's one destructive
 * path across the two areas. If the block is used on the timeline, the core
 * reports `confirmDeleteBlockInUse` with how many placements would be
 * removed and leaves everything untouched; this asks the user to confirm,
 * stating that count, then resends with `force: true` if they agree.
 * Cancelling changes nothing at all.
 */
async function deleteBlock(root: HTMLElement, id: number): Promise<void> {
  const applied = await applyCommand({ type: "deleteBlock", payload: { id, force: false } });
  const confirmation = applied.effects.find(
    (effect): effect is Extract<Effect, { type: "confirmDeleteBlockInUse" }> =>
      effect.type === "confirmDeleteBlockInUse",
  );
  if (confirmation) {
    render(root, applied.state);
    const uses = confirmation.uses;
    const noun = uses === 1 ? "placement" : "placements";
    if (!window.confirm(`This block is used in ${uses} ${noun}. Delete it and remove them?`)) {
      return;
    }
    const forced = await applyCommand({ type: "deleteBlock", payload: { id, force: true } });
    render(root, forced.state);
    return;
  }
  render(root, applied.state);
}

/**
 * The core's `is_playing` flag reverts to false purely server-side, once
 * the shell's own timer decides a schedule has finished (see
 * `play_take_schedule` in `src-tauri/src/lib.rs`) — nothing pushes that
 * back to the webview. This mirrors that same duration so the UI refreshes
 * itself shortly after, rather than leaving play/record controls (and, for
 * the timeline, the playhead) looking like playback is still running.
 */
function schedulePlaybackRefresh(root: HTMLElement, totalPulses: number, bpm: number): void {
  const durationMs = pulseElapsedMs(totalPulses, bpm);
  window.setTimeout(() => {
    void fetchProjectState().then((state) => {
      if (!state.is_playing) {
        timelinePlaybackActive = false;
        render(root, state);
        return;
      }
      // Still playing: either a loop (issue #19) carried the same pass
      // into another lap, or a newer playback with the same duration
      // started in the meantime. Either way, keep checking at the same
      // cadence rather than assuming this pass is the one still running.
      schedulePlaybackRefresh(root, totalPulses, bpm);
    });
  }, durationMs + 50);
}

interface LiveNoteEvent {
  pitch: number;
  velocity: number;
  pulse: number;
  is_on: boolean;
}

/** Live counterpart to `wireRecordingControls`: listens for the backend's
 * "live-note" event (emitted per note on/off while a session is open, see
 * `src-tauri/src/recording.rs`) and keeps `liveNotes` in sync so the take
 * grid can show notes as they're played, before recording stops and
 * `state.take` exists. A note-on opens a note with `end_pulse` equal to its
 * `start_pulse`; the matching note-off closes the most recent still-open
 * note at that pitch. */
function wireLiveNoteFeed(root: HTMLElement): void {
  listen<LiveNoteEvent>("live-note", (event) => {
    const { pitch, velocity, pulse, is_on } = event.payload;
    if (is_on) {
      liveNotes = [...liveNotes, { pitch, velocity, start_pulse: pulse, end_pulse: pulse }];
    } else {
      let openIndex = -1;
      for (let index = liveNotes.length - 1; index >= 0; index -= 1) {
        const note = liveNotes[index];
        if (note && note.pitch === pitch && note.end_pulse === note.start_pulse) {
          openIndex = index;
          break;
        }
      }
      if (openIndex !== -1) {
        liveNotes = liveNotes.map((note, index) =>
          index === openIndex ? { ...note, end_pulse: Math.max(pulse, note.start_pulse) } : note,
        );
      }
    }
    if (currentState?.is_recording) render(root, currentState);
  }).catch((error: unknown) => console.error("could not listen for live-note:", error));
}

async function playTake(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "playTake" });
  render(root, applied.state);
  if (applied.state.is_playing && applied.state.take) {
    // Playback schedules the take's *resolved* view (trim + quantisation
    // applied), not its raw length — see `Take::notes()` in the core —
    // so the refresh timer has to match that, not `takeEndPulse`'s raw span.
    const length = applied.state.take.notes.reduce((end, note) => Math.max(end, note.end_pulse), 0);
    schedulePlaybackRefresh(root, length, applied.state.bpm);
  }
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
  const block = currentState?.blocks[index];
  const applied = await applyCommand({ type: "playBlock", payload: index });
  render(root, applied.state);
  if (applied.state.is_playing && block) {
    const length = block.notes.reduce((end, note) => Math.max(end, note.end_pulse), 0);
    schedulePlaybackRefresh(root, length, applied.state.bpm);
  }
}

/**
 * Plays the whole arrangement from the beginning (issue #18): the timeline
 * play button, and `Space` when nothing is already playing.
 */
async function playTimeline(root: HTMLElement): Promise<void> {
  const applied = await applyCommand({ type: "playTimeline" });
  timelinePlaybackActive = applied.state.is_playing;
  render(root, applied.state);
  if (applied.state.is_playing) {
    schedulePlaybackRefresh(root, timelineTotalPulses(applied.state), applied.state.bpm);
  }
}

/** `Space` while something is already playing (issue #18): stops it
 * immediately rather than waiting for it to finish on its own. */
async function stopCurrentPlayback(root: HTMLElement): Promise<void> {
  timelinePlaybackActive = false;
  const applied = await stopPlayback();
  render(root, applied.state);
}

/**
 * Exports the timeline as a `.mid` file (issue #24). Doesn't touch
 * `ProjectState` at all — export isn't a musical edit — so there is
 * nothing to render afterwards; the only thing worth telling the user is
 * that an empty timeline had nothing to export, since a native save
 * dialog never even opened for them to notice that on their own.
 */
async function exportTimelineAsMidi(): Promise<void> {
  const outcome = await exportMidi();
  if (outcome.status === "nothingToExport") {
    window.alert("The timeline is empty — there is nothing to export.");
  }
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
  if (!currentState?.is_dirty) return;
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

/**
 * Runs `action` unconditionally: the caller has already established (via
 * `requestProjectAction`, or because the user just chose to save/discard)
 * that it's safe to go ahead.
 */
async function proceedWithProjectAction(
  root: HTMLElement,
  action: PendingProjectAction,
): Promise<void> {
  if (action.kind === "new") {
    const applied = await applyCommand({ type: "newProject", payload: { force: true } });
    activeProjectName = null;
    render(root, applied.state);
    return;
  }
  if (action.kind === "open") {
    const applied = await openProject(action.name, true);
    activeProjectName = action.name;
    render(root, applied.state);
    return;
  }
  // "quit": bypasses `onCloseRequested` entirely, so it can't re-trigger the
  // warning it was itself raised to resolve.
  await getCurrentWindow().destroy();
}

/**
 * The entry point for "New project", switching projects, and quitting: asks
 * for confirmation first if there are unsaved changes, per the spec's
 * warning before discarding work, otherwise proceeds immediately.
 */
async function requestProjectAction(
  root: HTMLElement,
  action: PendingProjectAction,
): Promise<void> {
  const state = currentState ?? (await fetchProjectState());
  if (!state.is_dirty) {
    await proceedWithProjectAction(root, action);
    return;
  }
  pendingProjectAction = action;
  render(root, state);
}

/** The unsaved-changes prompt's "Save" choice: saves, naming the project
 * first through the existing inline form if it has never been saved, then
 * resumes whichever action was pending. */
async function saveThenProceedWithProjectAction(
  root: HTMLElement,
  action: PendingProjectAction,
): Promise<void> {
  if (!activeProjectName) {
    isNamingProject = true;
    pendingActionAfterNaming = action;
    render(root, currentState ?? (await fetchProjectState()));
    root.querySelector<HTMLInputElement>("[data-project-name]")?.focus();
    return;
  }
  await saveCurrentProject(root);
  await proceedWithProjectAction(root, action);
}

/**
 * Checked once at startup: if a crash-recovery snapshot (issue #15) is
 * present, holds it in `pendingRecovery` so `render` shows the "recover?"
 * modal over the otherwise-normal (fresh, empty) initial state, rather than
 * silently applying it — recovery is always the user's decision.
 */
async function checkForRecoverySnapshot(root: HTMLElement): Promise<void> {
  const snapshot = await fetchRecoverySnapshot();
  if (!snapshot) return;
  pendingRecovery = snapshot;
  render(root, currentState!);
}

/** The recovery modal's choice: accept restores the interrupted session
 * (still reporting unsaved changes) under whatever name it had, if any;
 * decline discards the snapshot and leaves the fresh project the app
 * already opened on. Either way the snapshot itself is gone once this
 * returns. */
async function settleRecovery(root: HTMLElement, accept: boolean): Promise<void> {
  const projectName = pendingRecovery?.project_name ?? null;
  pendingRecovery = null;
  const applied = await resolveRecovery(accept);
  activeProjectName = accept ? projectName : null;
  render(root, applied.state);
}

function wireProjectControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-project-menu-toggle]")) {
      projectMenuOpen = !projectMenuOpen;
      if (currentState) render(root, currentState);
    }
    if (target.closest("[data-save-project]")) void saveCurrentProject(root);
    if (target.closest("[data-new-project]")) {
      projectMenuOpen = false;
      void requestProjectAction(root, { kind: "new" });
    }
    if (target.closest("[data-cancel-project-name]")) {
      isNamingProject = false;
      pendingActionAfterNaming = null;
      void fetchProjectState().then((state) => render(root, state));
    }
    const open = target.closest<HTMLButtonElement>("[data-open-project]");
    if (open?.dataset.openProject) {
      projectMenuOpen = false;
      void requestProjectAction(root, { kind: "open", name: open.dataset.openProject });
    }

    if (target.closest("[data-unsaved-cancel]")) {
      pendingProjectAction = null;
      render(root, currentState!);
    }
    if (target.closest("[data-unsaved-discard]")) {
      const action = pendingProjectAction;
      pendingProjectAction = null;
      if (action) void proceedWithProjectAction(root, action);
    }
    if (target.closest("[data-unsaved-save]")) {
      const action = pendingProjectAction;
      pendingProjectAction = null;
      if (action) void saveThenProceedWithProjectAction(root, action);
    }

    if (target.closest("[data-recovery-accept]")) void settleRecovery(root, true);
    if (target.closest("[data-recovery-decline]")) void settleRecovery(root, false);
  });

  root.addEventListener("submit", (event) => {
    const form = event.target as HTMLElement;
    if (!form.matches("[data-project-name-form]")) return;
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>("[data-project-name]");
    if (!input?.value.trim()) return;
    const action = pendingActionAfterNaming;
    pendingActionAfterNaming = null;
    void saveCurrentProject(root, input.value.trim()).then(() => {
      if (action) void proceedWithProjectAction(root, action);
    });
  });

  window.addEventListener("keydown", (event) => {
    if (pendingProjectAction && event.key === "Tab") {
      event.preventDefault();
      root.querySelector<HTMLButtonElement>("[data-unsaved-save]")?.focus();
      return;
    }
    if (pendingProjectAction) return;
    if (!event.metaKey || event.key.toLowerCase() !== "s") return;
    event.preventDefault();
    void saveCurrentProject(root);
  });
}

/**
 * Intercepts the window close request per the spec's "quitting with unsaved
 * changes shows the same warning": with nothing unsaved the window closes
 * normally, otherwise the close is held open and the same confirm-discard
 * overlay `requestProjectAction` uses for New/switch project is shown.
 */
function wireQuitWarning(root: HTMLElement): void {
  void getCurrentWindow().onCloseRequested(async (event) => {
    const state = currentState ?? (await fetchProjectState());
    if (!state.is_dirty) return;
    event.preventDefault();
    pendingProjectAction = { kind: "quit" };
    render(root, state);
  });
}

/** Zoom is a view setting (#16), so this only ever re-renders locally —
 * there is no command to send, and nothing about the change is worth the
 * core knowing. */
function wireTimelineControls(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;

    const zoomButton = target.closest<HTMLButtonElement>("[data-timeline-zoom]");
    if (zoomButton?.dataset.timelineZoom) {
      timelineZoom = zoomButton.dataset.timelineZoom as TimelineZoom;
      render(root, currentState!);
    }

    if (target.closest("[data-play-timeline]")) {
      if (currentState?.is_playing) {
        void stopCurrentPlayback(root);
      } else {
        void playTimeline(root);
      }
    }

    if (target.closest("[data-export-midi]")) {
      void exportTimelineAsMidi();
    }
  });

  root.addEventListener("change", (event) => {
    const input = event.target as HTMLInputElement;
    if (!input.matches("[data-loop-enabled]")) return;
    void applyCommand({ type: "setLoopEnabled", payload: input.checked }).then((applied) =>
      render(root, applied.state),
    );
  });

  window.addEventListener("keydown", (event) => {
    if (event.code !== "Space" && event.key !== " ") return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const active = document.activeElement;
    // Never hijack Space while the user is typing or operating a control
    // that treats Space as its own gesture (a button, a range slider).
    if (
      active instanceof HTMLInputElement ||
      active instanceof HTMLSelectElement ||
      active instanceof HTMLButtonElement
    ) {
      return;
    }
    event.preventDefault();
    if (currentState?.is_playing) {
      void stopCurrentPlayback(root);
    } else {
      void playTimeline(root);
    }
  });
}

/**
 * Dragging a block from the library onto the timeline (issue #17). The drop
 * target only needs the dragged block's id — where it lands is never the
 * drop's x position, since a placement always lands flush after the
 * existing ones on the track (that's the whole ticket; a real "drop
 * here specifically" arrives with #20's insert-with-push) — so a plain
 * `text/plain` payload is enough, no custom drag data type needed.
 */
const DRAG_BLOCK_TYPE = "application/x-daw-block";
const DRAG_PLACEMENT_TYPE = "application/x-daw-placement";

/**
 * Where a drop at `event.clientX` lands within `track`'s ordered
 * placements (issue #20): the index of the first placement whose midpoint
 * the drop is before, or the track's current length if it's past all of
 * them (matching #17's flush-append). The drop's x position is what
 * decides this, not which specific element it happened to land on, so a
 * drop between two placements' edges still resolves sensibly.
 */
function computeDropIndex(event: DragEvent, track: number): number {
  const ordered = (currentState?.placements ?? [])
    .filter((placement) => placement.track === track)
    .sort((a, b) => a.start_pulse - b.start_pulse);
  const trackEl = (event.target as HTMLElement).closest<HTMLElement>(".timeline__track");
  if (!trackEl) return ordered.length;
  const dropX = event.clientX - trackEl.getBoundingClientRect().left;
  const pxPerPulse = TIMELINE_ZOOM_PX_PER_PULSE[timelineZoom];
  for (const [index, placement] of ordered.entries()) {
    const length = placement.notes.reduce((end, note) => Math.max(end, note.end_pulse), 0);
    const midpoint = (placement.start_pulse + length / 2) * pxPerPulse;
    if (dropX < midpoint) return index;
  }
  return ordered.length;
}

/**
 * The deliberate silence (issue #22) a `Cmd`-held drop at `event.clientX`
 * asks for, ahead of whichever placement lands at `index`: the distance
 * between the drop's pulse (snapped to the grid) and where that placement
 * would land with no gap at all — flush after its new predecessor, or the
 * very start of the track if it has none. Never negative; dropping earlier
 * than flush just asks for no gap. `excludeId` leaves out the placement
 * being dragged itself, when reordering, so it doesn't count as its own
 * predecessor.
 */
function computeDropGap(
  event: DragEvent,
  track: number,
  index: number,
  excludeId: number | null,
): number {
  const trackEl = (event.target as HTMLElement).closest<HTMLElement>(".timeline__track");
  if (!trackEl) return 0;
  const pxPerPulse = TIMELINE_ZOOM_PX_PER_PULSE[timelineZoom];
  const dropX = event.clientX - trackEl.getBoundingClientRect().left;
  const dropPulse = Math.max(0, Math.round(dropX / pxPerPulse));

  const ordered = (currentState?.placements ?? [])
    .filter((placement) => placement.track === track && placement.id !== excludeId)
    .sort((a, b) => a.start_pulse - b.start_pulse);
  const predecessor = ordered[index - 1];
  const naturalPulse = predecessor
    ? predecessor.start_pulse +
      predecessor.notes.reduce((end, note) => Math.max(end, note.end_pulse), 0)
    : 0;

  return Math.max(0, dropPulse - naturalPulse);
}

/**
 * Dragging a block from the library onto the timeline inserts it (issue
 * #17 for a plain append, #20 for inserting between two placements, pushing
 * the remainder later); dragging an existing placement reorders it, with
 * the same push behaviour. Holding `Cmd` while dropping leaves a deliberate
 * gap before the placement instead of butting it against its neighbour
 * (issue #22), sent as a follow-up `setPlacementGap` once the insert or
 * reorder itself lands — that also lets a `Cmd`-drop flush against a
 * neighbour explicitly clear a gap the placement already had.
 */
function wireBlockPlacementDragAndDrop(root: HTMLElement): void {
  root.addEventListener("dragstart", (event) => {
    const block = (event.target as HTMLElement).closest<HTMLElement>("[data-drag-block]");
    if (block?.dataset.dragBlock) {
      event.dataTransfer?.setData(DRAG_BLOCK_TYPE, block.dataset.dragBlock);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "copy";
      return;
    }
    const placement = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-drag-placement]",
    );
    if (placement?.dataset.dragPlacement) {
      event.dataTransfer?.setData(DRAG_PLACEMENT_TYPE, placement.dataset.dragPlacement);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
    }
  });

  root.addEventListener("dragover", (event) => {
    if (!(event.target as HTMLElement).closest("[data-timeline-drop]")) return;
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = event.dataTransfer.types.includes(DRAG_PLACEMENT_TYPE)
        ? "move"
        : "copy";
    }
  });

  root.addEventListener("drop", (event) => {
    if (!(event.target as HTMLElement).closest("[data-timeline-drop]")) return;
    event.preventDefault();
    const track = 0;

    const blockId = event.dataTransfer?.getData(DRAG_BLOCK_TYPE);
    if (blockId) {
      const index = computeDropIndex(event, track);
      const gapBefore = event.metaKey ? computeDropGap(event, track, index, null) : null;
      void applyCommand({
        type: "insertPlacementAt",
        payload: { block_id: Number(blockId), track, index },
      }).then((applied) => {
        render(root, applied.state);
        if (gapBefore === null) return;
        // The command just created exactly one placement, so this is its id.
        const insertedId = applied.state.next_placement_id - 1;
        void applyCommand({
          type: "setPlacementGap",
          payload: { id: insertedId, gap_before: gapBefore },
        }).then((withGap) => render(root, withGap.state));
      });
      return;
    }

    const placementId = event.dataTransfer?.getData(DRAG_PLACEMENT_TYPE);
    if (placementId) {
      const id = Number(placementId);
      const ordered = (currentState?.placements ?? [])
        .filter((placement) => placement.track === track)
        .sort((a, b) => a.start_pulse - b.start_pulse);
      const currentIndex = ordered.findIndex((placement) => placement.id === id);
      let index = computeDropIndex(event, track);
      // `Command::ReorderPlacement` computes `new_index` against the track
      // with the dragged placement already removed; `computeDropIndex`
      // doesn't know that, so adjust for a drop past the placement's own
      // current position.
      if (currentIndex !== -1 && index > currentIndex) index -= 1;
      const gapBefore = event.metaKey ? computeDropGap(event, track, index, id) : null;
      void applyCommand({
        type: "reorderPlacement",
        payload: { id, new_index: index },
      }).then((applied) => {
        render(root, applied.state);
        if (gapBefore === null) return;
        void applyCommand({
          type: "setPlacementGap",
          payload: { id, gap_before: gapBefore },
        }).then((withGap) => render(root, withGap.state));
      });
    }
  });
}

/**
 * Selecting a placement on the timeline and removing it with `Delete`
 * (issue #21). Selection is a view concern, like `timelineZoom` — clicking a
 * placement selects it, clicking anywhere else on the timeline clears the
 * selection, and `Delete`/`Backspace` sends `Command::DeletePlacement` for
 * whichever placement is currently selected.
 */
function wireTimelineSelection(root: HTMLElement): void {
  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    const placement = target.closest<HTMLElement>("[data-placement]");
    if (placement?.dataset.placement) {
      selectedPlacementId = Number(placement.dataset.placement);
      render(root, currentState!);
      return;
    }
    if (target.closest("[data-timeline-drop]") && selectedPlacementId !== null) {
      selectedPlacementId = null;
      render(root, currentState!);
    }
  });

  window.addEventListener("keydown", (event) => {
    if (event.key !== "Delete" && event.key !== "Backspace") return;
    if (selectedPlacementId === null) return;
    const active = document.activeElement;
    // Never hijack Delete/Backspace while the user is typing, exactly like
    // the Space-bar transport shortcut above.
    if (
      active instanceof HTMLInputElement ||
      active instanceof HTMLSelectElement ||
      active instanceof HTMLTextAreaElement ||
      active instanceof HTMLButtonElement
    ) {
      return;
    }
    event.preventDefault();
    const id = selectedPlacementId;
    selectedPlacementId = null;
    void applyCommand({ type: "deletePlacement", payload: id }).then((applied) =>
      render(root, applied.state),
    );
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
    && midiStatus.connected === status.connected
    && midiStatus.devices.length === status.devices.length
    && midiStatus.devices.every((device, index) => device.id === status.devices[index]?.id && device.name === status.devices[index]?.name);
  midiStatus = status;
  if (unchanged) return;
  if (currentState) render(root, currentState);
}

function showMidiConnectionStatus(root: HTMLElement, connected: boolean, label: string): void {
  const indicator = root.querySelector<HTMLElement>("[data-midi-status]");
  if (!indicator) return;
  indicator.classList.toggle("is-connected", connected);
  indicator.setAttribute("aria-label", label);
  indicator.title = label;
}

async function refreshMidiDevices(root: HTMLElement): Promise<void> {
  try {
    renderMidiStatus(root, await listMidiDevices());
  } catch (error: unknown) {
    showMidiConnectionStatus(root, false, `Could not check MIDI inputs: ${String(error)}`);
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
        showMidiConnectionStatus(root, false, `Could not select MIDI input: ${String(error)}`);
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
    if (target.closest("[data-play-take]")) {
      void playTake(root);
    }
    if (target.closest("[data-add-to-library]")) {
      void addTakeToLibrary(root);
    }
    const blockButton = target.closest<HTMLButtonElement>("[data-play-block]");
    if (blockButton) void playBlock(root, Number(blockButton.dataset.playBlock));
    const deleteButton = target.closest<HTMLButtonElement>("[data-delete-block]");
    if (deleteButton) void deleteBlock(root, Number(deleteButton.dataset.deleteBlock));
  });

  root.addEventListener("dblclick", (event) => {
    const name = (event.target as HTMLElement).closest<HTMLElement>("[data-block-name]");
    if (!name) return;
    editingBlockId = Number(name.dataset.blockName);
    render(root, currentState!);
  });

  // Escaping cancels the edit by re-rendering, which removes the focused
  // input from the DOM and — in every browser — synchronously fires `blur`
  // on it as a side effect. This flag tells the blur handler below that the
  // blur is that removal, not the user leaving the field, so it doesn't
  // commit a rename the user just cancelled.
  let cancellingBlockNameEdit = false;

  function commitBlockNameEdit(input: HTMLInputElement): void {
    const id = Number(input.dataset.blockNameInput);
    const name = input.value.trim();
    editingBlockId = null;
    if (!name) {
      render(root, currentState!);
      return;
    }
    void applyCommand({ type: "renameBlock", payload: { id, name } }).then((applied) =>
      render(root, applied.state),
    );
  }

  root.addEventListener(
    "blur",
    (event) => {
      const input = event.target as HTMLElement;
      if (!input.matches("[data-block-name-input]")) return;
      if (cancellingBlockNameEdit) {
        cancellingBlockNameEdit = false;
        return;
      }
      commitBlockNameEdit(input as HTMLInputElement);
    },
    true,
  );

  root.addEventListener("keydown", (event) => {
    const input = event.target as HTMLElement;
    if (!input.matches("[data-block-name-input]")) return;
    if (event.key === "Enter") {
      event.preventDefault();
      (input as HTMLInputElement).blur();
    }
    if (event.key === "Escape") {
      cancellingBlockNameEdit = true;
      editingBlockId = null;
      render(root, currentState!);
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

  root.addEventListener("change", (event) => {
    const select = event.target as HTMLSelectElement;
    if (!select.matches("[data-quantisation]")) return;
    void applyTakeQuantisation(root);
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
      if (currentState) render(root, currentState);
    }
    if (target.closest("[data-toggle-inspector]")) {
      inspectorCollapsed = !inspectorCollapsed;
      if (currentState) render(root, currentState);
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
  await checkForRecoverySnapshot(root);
  await refreshProjects(root);
  wireProjectControls(root);
  wireTempoControls(root);
  wireSoundControls(root);
  wireRecordingControls(root);
  wireTestNoteButton(root);
  wireMidiPicker(root);
  wireUndoRedoKeys(root);
  wireQuitWarning(root);
  wireTimelineControls(root);
  wireBlockPlacementDragAndDrop(root);
  wireTimelineSelection(root);
  wireLayoutControls(root);
  wireLiveNoteFeed(root);
  await refreshMidiDevices(root);
  window.setInterval(() => void refreshMidiDevices(root), 1_000);

  fetchAudioStatus()
    .then((status) => {
      if (!status.available) showAudioStatus(root, status.message, true);
    })
    .catch((error: unknown) => showAudioStatus(root, `could not check audio output: ${String(error)}`, true));
}

void main();
