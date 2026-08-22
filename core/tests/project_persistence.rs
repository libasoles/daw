use daw_core::{Command, DawCore};

#[test]
fn dirty_state_tracks_manual_saves_and_undoes() {
    let mut core = DawCore::new();

    assert!(
        core.state().is_dirty,
        "an unnamed project needs its first save"
    );

    core.apply(Command::ProjectSaved);
    assert!(!core.state().is_dirty);

    core.apply(Command::SetBpm(140));
    assert!(core.state().is_dirty);

    core.apply(Command::Undo);
    assert!(!core.state().is_dirty);

    core.apply(Command::Redo);
    assert!(core.state().is_dirty);
}
