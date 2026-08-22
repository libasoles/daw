use daw_core::ports::{MemoryStorage, Storage};
use daw_core::ProjectState;

#[test]
fn memory_storage_round_trips_a_project_document_by_name() {
    let mut storage = MemoryStorage::default();
    let document = serde_json::to_string(&ProjectState::default()).unwrap();
    storage.save("first-song", document.clone()).unwrap();
    assert_eq!(storage.list().unwrap(), vec!["first-song"]);
    assert_eq!(storage.load("first-song").unwrap(), Some(document));
}
