use daw_core::{ports::MemoryStorage, ports::Storage, Command, DawCore, Effect};

#[test]
fn dirty_state_tracks_manual_saves_and_undoes() {
    let mut core = DawCore::new();

    assert!(
        core.state().is_dirty,
        "an unnamed project needs its first save"
    );

    core.apply(Command::ProjectSaved(core.project_document()));
    assert!(!core.state().is_dirty);

    core.apply(Command::SetBpm(140));
    assert!(core.state().is_dirty);

    core.apply(Command::Undo);
    assert!(!core.state().is_dirty);

    core.apply(Command::Redo);
    assert!(core.state().is_dirty);
}

#[test]
fn opening_a_saved_document_restores_it_and_resets_its_dirty_state() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));

    let save = core.apply(Command::SaveProject("first-song".into()));
    let Effect::SaveProject { name, document } = save.effects.as_slice()[0].clone() else {
        panic!("saving must ask the shell to persist a document");
    };
    let mut storage = MemoryStorage::default();
    storage
        .save(&name, serde_json::to_string(&document).unwrap())
        .unwrap();
    core.apply(Command::ProjectSaved(document.clone()));

    core.apply(Command::SetBpm(90));
    let loaded = serde_json::from_str(&storage.load("first-song").unwrap().unwrap()).unwrap();
    core.apply(Command::OpenProject(loaded));

    assert_eq!(core.project_document(), document);
    assert!(!core.state().is_dirty);
}

#[test]
fn a_change_made_while_saving_stays_dirty_after_the_write_finishes() {
    let mut core = DawCore::new();
    let save = core.apply(Command::SaveProject("first-song".into()));
    let Effect::SaveProject { document, .. } = save.effects.as_slice()[0].clone() else {
        panic!("saving must ask the shell to persist a document");
    };

    core.apply(Command::SetBpm(140));
    core.apply(Command::ProjectSaved(document));

    assert!(core.state().is_dirty);
}
