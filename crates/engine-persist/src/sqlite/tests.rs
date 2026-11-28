use super::*;

fn temp_db(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "s7-pp-b1-{}-{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    path
}

fn sample_object(payload: &[u8]) -> StoredObject {
    // A well-formed domain-tagged object (the Encoder's
    // length-prefixed string form).
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(7u64).to_le_bytes());
    bytes.extend_from_slice(b"s7/test/");
    bytes.extend_from_slice(payload);
    StoredObject::new(bytes).unwrap()
}

#[test]
fn fresh_open_is_unfounded_and_founds_cleanly() {
    let path = temp_db("fresh");
    let mut medium = SqliteMedium::open(&path).unwrap();
    assert_eq!(medium.head().unwrap(), None);
    assert!(medium.campaign().is_none());

    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-test", 0);
    let outcome = medium.append(&founding, &[], None).unwrap();
    let AppendOutcome::Accepted { tier, .. } = outcome else {
        panic!("the founding append accepts: {outcome:?}")
    };
    assert_eq!(tier, SyncTier::TransactionalWalFull);
    assert_eq!(medium.head().unwrap(), Some(founding.id()));
    assert_eq!(medium.campaign().map(|(f, _, _)| f), Some(founding.id()));

    let summary = medium.validate().unwrap();
    assert_eq!(summary.head, founding.id());
    assert_eq!(summary.height, 0);
    assert_eq!(summary.records, 1);
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn reopen_validates_and_the_authority_row_survives() {
    let path = temp_db("reopen");
    {
        let mut medium = SqliteMedium::open(&path).unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-reopen", 3);
        medium.append(&founding, &[], None).unwrap();
    }
    {
        let mut medium = SqliteMedium::open(&path).unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-reopen", 3);
        assert_eq!(medium.head().unwrap(), Some(founding.id()));
        let summary = medium.validate().unwrap();
        assert_eq!(summary.campaign_id, "pp-b1-reopen");
        assert_eq!(summary.records, 1);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn regular_append_walks_and_rejects_stale_parents() {
    let path = temp_db("regular");
    let mut medium = SqliteMedium::open(&path).unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-regular", 0);
    medium.append(&founding, &[], None).unwrap();
    let founding_id = founding.id();

    let object = sample_object(b"payload-one");
    let regular = RevisionRecord::regular(founding_id, 0, 9).with_content_refs(vec![object.hash()]);
    let AppendOutcome::Accepted { id, .. } = medium
        .append(&regular, &[object], Some(founding_id))
        .unwrap()
    else {
        panic!("the regular append accepts")
    };
    assert_eq!(id, regular.id());

    // A stale writer against the superseded parent.
    let stale_object = sample_object(b"payload-two");
    let stale =
        RevisionRecord::regular(founding_id, 0, 10).with_content_refs(vec![stale_object.hash()]);
    let outcome = medium
        .append(&stale, &[stale_object], Some(founding_id))
        .unwrap();
    assert_eq!(
        outcome,
        AppendOutcome::RejectedStaleParent {
            observed: Some(regular.id())
        }
    );

    let summary = medium.validate().unwrap();
    assert_eq!(summary.records, 2);
    assert_eq!(summary.height, 1);
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_only_audit_validates_and_refuses_appends() {
    let path = temp_db("readonly");
    {
        let mut medium = SqliteMedium::open(&path).unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-audit", 0);
        medium.append(&founding, &[], None).unwrap();
    }
    let mut audit = SqliteMedium::open_read_only(&path).unwrap();
    let summary = audit.validate().unwrap();
    assert_eq!(summary.records, 1);
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pp-b1-audit", 0);
    let refused = audit.append(&founding, &[], None);
    assert!(matches!(refused, Err(MediumError::Sqlite { .. })));
    let _ = std::fs::remove_file(&path);
}
