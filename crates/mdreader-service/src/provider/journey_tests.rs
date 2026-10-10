use super::*;

#[test]
fn documents_show_cold_recent_rows_and_exact_final_page_without_fetching_bodies() {
    let (mut c, descriptor) = recent_fixture();
    coordinator::initialize_client(&mut c).unwrap();
    staged(
        &mut c,
        &descriptor,
        vec![
            recent_row("old-a", Some(1)),
            recent_row("new-z", Some(9)),
            recent_row("tie-b", Some(5)),
            recent_row("tie-a", Some(5)),
        ],
        false,
    );
    let first = coordinator::documents(&mut c, descriptor.binding(), 0, 2).unwrap();
    assert_eq!(first["documents"][0]["name"], "new-z.md");
    assert_eq!(first["documents"][1]["name"], "tie-a.md");
    assert_eq!(first["next_offset"], 2);
    let second = coordinator::documents(&mut c, descriptor.binding(), 2, 2).unwrap();
    assert_eq!(second["documents"][0]["name"], "tie-b.md");
    assert_eq!(second["documents"][1]["name"], "old-a.md");
    assert!(second["next_offset"].is_null());
    assert!(
        first["documents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["cached"] == false && row["admitted"] == false)
    );
    let state = listing::observe(&c, descriptor.binding()).unwrap();
    assert_eq!(state.known_documents, 4);
    assert!(matches!(state.completeness, ListingCompleteness::Partial));
    assert!(state.refresh_pending);
    disable(&mut c, descriptor.binding(), &UnsupportedProjection).unwrap();
    assert!(coordinator::documents(&mut c, descriptor.binding(), 0, 2).is_err());
}

#[test]
fn metadata_census_reopens_and_retains_old_complete_observation_during_refresh() {
    let (mut original, descriptor) = recent_fixture();
    coordinator::initialize_client(&mut original).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("public-provider.sqlite");
    let mut stored = Connection::open(&path).unwrap();
    rusqlite::backup::Backup::new(&original, &mut stored)
        .unwrap()
        .run_to_completion(16, std::time::Duration::ZERO, None)
        .unwrap();
    drop(original);
    let snap = staged(
        &mut stored,
        &descriptor,
        vec![recent_row("old-a", Some(1)), recent_row("new-z", Some(9))],
        true,
    );
    publish_snapshot(&mut stored, &snap, &UnsupportedProjection).unwrap();
    let before = listing::observe(&stored, descriptor.binding()).unwrap();
    assert!(matches!(
        before.completeness,
        ListingCompleteness::CompleteSelected
    ));
    assert_eq!(before.known_documents, 2);
    assert!(!before.refresh_pending);
    staged(
        &mut stored,
        &descriptor,
        vec![recent_row("latest", Some(99))],
        false,
    );
    drop(stored);
    let mut reopened = Connection::open(&path).unwrap();
    let retained = listing::observe(&reopened, descriptor.binding()).unwrap();
    assert_eq!(retained.known_documents, 2);
    assert_eq!(retained.observed_at_ms, before.observed_at_ms);
    assert!(matches!(
        retained.completeness,
        ListingCompleteness::CompleteSelected
    ));
    assert!(retained.refresh_pending);
    let page = coordinator::documents(&mut reopened, descriptor.binding(), 0, 10).unwrap();
    assert_eq!(page["documents"].as_array().unwrap().len(), 2);
    assert_eq!(page["documents"][0]["name"], "new-z.md");
    // A pre-ancestry serialized row is still readable on the maintained schema.
    let row = registry::metadata_in(
        &reopened,
        &snap.id().to_string(),
        &RecordKey::new("old-a").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(row.membership_ancestors.is_none());
    let count: u32 = reopened
        .query_row(
            "SELECT count(*) FROM mdreader_provider_registry_cache",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
