/**
 * The frontend is a presentation layer only. It turns gestures into commands
 * and renders `ProjectState`; it holds no musical logic, and it never computes
 * anything a test in `daw-core` could have asserted on.
 *
 * Right now it renders one placeholder identifying the app. Issue #3 wires the
 * command bridge; the three real regions follow in H1-H3.
 */

/** Three stacked blocks on a timeline — the shape the whole app is about. */
const mark = /* html */ `
  <svg class="placeholder__mark" viewBox="0 0 48 48" fill="none" aria-hidden="true">
    <rect x="4" y="9" width="26" height="8" rx="3" fill="currentColor" />
    <rect x="4" y="20" width="40" height="8" rx="3" fill="currentColor" opacity="0.66" />
    <rect x="4" y="31" width="16" height="8" rx="3" fill="currentColor" opacity="0.4" />
  </svg>
`;

function render(root: HTMLElement): void {
  root.innerHTML = /* html */ `
    <section class="placeholder">
      ${mark}
      <h1 class="placeholder__name">daw</h1>
      <p class="placeholder__tagline">
        Record takes, keep the good ones, arrange them on a timeline.
      </p>
      <p class="placeholder__status">no project &mdash; nothing to play yet</p>
    </section>
  `;
}

const root = document.querySelector<HTMLElement>("#app");
if (!root) {
  throw new Error("Missing #app mount point in index.html");
}
render(root);
