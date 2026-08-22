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
 */

import { applyCommand, fetchProjectState, type ProjectState } from "./core";

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
      <p class="placeholder__status">no project &mdash; nothing to play yet</p>
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
  wireUndoRedoKeys(root);
}

void main();
