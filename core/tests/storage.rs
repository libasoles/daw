use daw_core::ports::{MemoryStorage, Storage};
use daw_core::{Command, DawCore, ProjectState, RecordedNote, Take};

#[test]
fn memory_storage_round_trips_every_persisted_project_setting_by_name() {
    let mut storage = MemoryStorage::default();
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));
    core.apply(Command::SetTimeSignature {
        beats_per_bar: 4,
        beat_unit: 4,
    });
    core.apply(Command::SetInstrument(1));
    core.apply(Command::SetReverb(60));
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 64,
            velocity: 100,
            start_pulse: 2,
            end_pulse: 6,
        },
    ]))));
    core.apply(Command::AddTakeToLibrary);
    core.apply(Command::RenameBlock {
        id: 1,
        name: "Chorus".into(),
    });
    core.apply(Command::RecolourBlock {
        id: 1,
        color: "#60a5fa".into(),
    });
    core.apply(Command::AddPlacement {
        block_id: 1,
        track: 0,
    });

    let document = serde_json::to_string(&core.project_document()).unwrap();
    storage.save("first-song", document.clone()).unwrap();
    assert_eq!(storage.list().unwrap(), vec!["first-song"]);
    assert_eq!(storage.load("first-song").unwrap(), Some(document));

    let restored: ProjectState =
        serde_json::from_str(&storage.load("first-song").unwrap().unwrap()).unwrap();
    assert_eq!(restored, core.project_document());
}

#[test]
fn the_crash_recovery_snapshot_is_a_separate_document_from_every_named_project() {
    let mut storage = MemoryStorage::default();
    assert_eq!(storage.load_snapshot().unwrap(), None);

    storage
        .save("verse-ideas", "a project document".into())
        .unwrap();
    storage.save_snapshot("a recovery snapshot".into()).unwrap();

    // The snapshot never shows up as, or alongside, a named project...
    assert_eq!(storage.list().unwrap(), vec!["verse-ideas"]);
    assert_eq!(
        storage.load("verse-ideas").unwrap().unwrap(),
        "a project document"
    );
    assert_eq!(
        storage.load_snapshot().unwrap().unwrap(),
        "a recovery snapshot"
    );

    // ...and deleting it never touches a saved project file.
    storage.delete_snapshot().unwrap();
    assert_eq!(storage.load_snapshot().unwrap(), None);
    assert_eq!(
        storage.load("verse-ideas").unwrap().unwrap(),
        "a project document"
    );
}
