//! Test conventions for this project, established by issue #2.
//!
//! Every test lives in `core/tests/`, an *integration* test target. That is
//! deliberate: an integration test can only reach `daw-core`'s public API, so
//! it is impossible to write a test that reaches into internals. Tests drive
//! `DawCore` through public commands and assert on `ProjectState` or on the
//! note stream the core produces — never on private helpers, and never
//! requiring a MIDI keyboard, an audio device or a filesystem.
//!
//! Name tests as sentences describing observable behaviour, not as the name of
//! the function under test.

use daw_core::DawCore;

#[test]
fn a_new_core_opens_on_an_empty_project() {
    let core = DawCore::new();
    let state = core.state();

    assert_eq!(state.bpm, 120);
    assert_eq!(state.time_signature, (3, 4));
    assert_eq!(state.instrument, 0);
    assert_eq!(state.reverb, 0);
}
