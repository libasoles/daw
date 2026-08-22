use daw_core::{ports::MemoryStorage, ports::Storage, Command, DawCore, Effect, RecordedNote, Take};

#[test]
fn opening_a_saved_project_restores_every_setting_and_every_block() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));
    core.apply(Command::SetTimeSignature {
        beats_per_bar: 3,
        beat_unit: 4,
    });
    core.apply(Command::SetReverb(42));
    core.apply(Command::SetMetronomeEnabled(true));
    core.apply(Command::SetCountInEnabled(true));
    core.apply(Command::SetLoopEnabled(true));
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    core.apply(Command::AddTakeToLibrary);
    let block_id = core.state().blocks[0].id;
    core.apply(Command::RenameBlock {
        id: block_id,
        name: "Verse".into(),
    });
    core.apply(Command::RecolourBlock {
        id: block_id,
        color: "#60a5fa".into(),
    });
    core.apply(Command::AddPlacement { block_id, track: 0 });

    let saved_document = core.project_document();

    let mut reopened = DawCore::new();
    reopened.apply(Command::OpenProject {
        document: saved_document.clone(),
        force: true,
    });

    assert_eq!(reopened.state(), &saved_document);
    assert!(!reopened.state().is_dirty);
    assert!(!reopened.state().is_recording);
    assert!(!reopened.state().is_playing);
    assert_eq!(reopened.state().bpm, 140);
    assert_eq!(reopened.state().time_signature, (3, 4));
    assert_eq!(reopened.state().reverb, 42);
    assert!(reopened.state().metronome_enabled);
    assert!(reopened.state().count_in_enabled);
    assert!(reopened.state().loop_enabled);
    assert_eq!(reopened.state().blocks.len(), 1);
    assert_eq!(reopened.state().blocks[0].name, "Verse");
    assert_eq!(reopened.state().blocks[0].color, "#60a5fa");
    assert_eq!(reopened.state().placements.len(), 1);
    assert_eq!(reopened.state().placements[0].name, "Verse");

    // A freshly opened project starts clean: further saves stay disabled
    // until something actually changes.
    reopened.apply(Command::ProjectSaved);
    assert!(!reopened.state().is_dirty);
}

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
    core.apply(Command::OpenProject {
        document: loaded,
        force: true,
    });

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
