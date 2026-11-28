use super::c5b_local_tests::{a_root, genuine_campaign, inventory, plain_object};
use super::*;
use crate::range_installation::{
    DiscoveredRangeContext, DiscoveredRangeObservation, NotObservedTerminus,
    RangeInstallationLimits, RangeInstallationObservation,
    observe_discovered_range_terminal_under_coordination,
};
use crate::recovery_discovery::DiscoveryLimits;

fn composite(
    medium: &mut LocalFileMedium,
    head: &Hash,
) -> Result<DiscoveredRangeObservation, crate::range_installation::DiscoveredObservationError> {
    observe_discovered_range_terminal_under_coordination(
        medium,
        head,
        &DiscoveryLimits::default(),
        &RangeInstallationLimits::default(),
    )
}

#[test]
fn the_composite_observes_the_genuine_campaign() {
    // The full three-stage stack over a REAL local store: the
    // discovery walks the direct-written chain, the derivation
    // reproduces the campaign's own expected context, and the
    // coordinated observation verifies the terminal under the
    // provider's HEAD.lock scope.
    let root = a_root("c5d-positive");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    let result = composite(&mut medium, &campaign.head).unwrap();
    match &result.derivation {
        DiscoveredRangeContext::V2Context {
            context,
            covering_mk,
            ..
        } => {
            assert_eq!(*context, campaign.expected);
            assert_eq!(*covering_mk, campaign.mk);
        }
        other => panic!("the genuine campaign must derive (got: {other:?})"),
    }
    let Some(coordinated) = &result.observation else {
        panic!("the derived V2 context must carry the observation");
    };
    assert_eq!(coordinated.coordinated_head, campaign.head);
    let RangeInstallationObservation::ObservedOnAncestry {
        observed_head,
        candidate,
        covering_mk,
        plan_record,
        begin_record,
        batch_markers,
        ..
    } = &coordinated.observation
    else {
        panic!("the terminal-carrying campaign observes positively");
    };
    assert_eq!(*observed_head, campaign.head);
    assert_eq!(candidate.revision, campaign.terminal.unwrap());
    assert_eq!(*covering_mk, campaign.mk);
    assert_eq!(*plan_record, campaign.plan_record);
    assert_eq!(*begin_record, campaign.begin_record);
    assert_eq!(*batch_markers, campaign.marker_records);
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_composite_without_a_terminal_reports_the_absent_terminus() {
    // The startup's in-flight case: the derivation still
    // derives (the terminal is never a derivation input), and
    // the coordinated observation reports the absence bound to
    // the coordinated head — never an invented terminal.
    let root = a_root("c5d-absent");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), false);
    let result = composite(&mut medium, &campaign.head).unwrap();
    assert!(matches!(
        &result.derivation,
        DiscoveredRangeContext::V2Context { context, .. } if *context == campaign.expected
    ));
    let Some(coordinated) = &result.observation else {
        panic!("the in-flight range must still observe");
    };
    assert_eq!(coordinated.coordinated_head, campaign.head);
    assert!(matches!(
        &coordinated.observation,
        RangeInstallationObservation::NotObservedOnAncestry {
            observed_head,
            terminus: NotObservedTerminus::ExpectedCoveringCheckpoint { mk },
            ..
        } if *observed_head == campaign.head && *mk == campaign.mk
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_composite_is_read_only_on_localfile() {
    // The store's complete file inventory is byte-identical
    // before and after the full three-stage composite.
    let root = a_root("c5d-readonly");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    let before = inventory(&root);
    let _ = composite(&mut medium, &campaign.head).unwrap();
    let after = inventory(&root);
    assert_eq!(before, after, "the composite mutated nothing");
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_stale_seed_two_provenance_contract_holds() {
    // The discovery seed is an OLDER head while the provider's
    // HEAD has moved: the derivation is bound to the seed's
    // segment (the discovery's own contract), and the
    // observation answers for the DERIVED context on the
    // coordinated head's ancestry — the two provenances never
    // conflated, never a claim that the discovered range is THE
    // active range.
    let root = a_root("c5d-stale");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    // The later publication, direct-written (the A3 lesson: the
    // open medium's cursor never learns of the direct-written
    // chain, and the REAL append path would refuse the CAS
    // against its stale in-memory state).
    let object = StoredObject::new(plain_object(b"c5d-later")).unwrap();
    let record = RevisionRecord::regular(campaign.head, campaign.head_epoch, 99)
        .with_content_refs(vec![object.hash()]);
    let later_head = record.id();
    std::fs::write(
        medium.object_path_pub(&later_head),
        record.canonical_bytes(),
    )
    .unwrap();
    std::fs::write(medium.object_path_pub(&object.hash()), object.bytes()).unwrap();
    {
        let bytes = {
            let mut enc = engine_core::Encoder::new();
            enc.write_str(HEAD_DOMAIN);
            enc.write_u32(HEAD_VERSION);
            enc.write_bytes(later_head.as_bytes());
            enc.into_bytes()
        };
        std::fs::write(root.join(HEAD_FILE), bytes).unwrap();
    }
    // The composite from the OLD seed: the derivation derives
    // the seed-relative context; the observation runs under the
    // provider's CURRENT head.
    let result = composite(&mut medium, &campaign.head).unwrap();
    assert!(matches!(
        &result.derivation,
        DiscoveredRangeContext::V2Context { context, .. } if *context == campaign.expected
    ));
    let Some(coordinated) = &result.observation else {
        panic!("the derived context must still observe");
    };
    // The observation's coordinated head is the PROVIDER's new
    // head, and the terminal below it still verifies FOR THE
    // DERIVED CONTEXT.
    assert_eq!(coordinated.coordinated_head, later_head);
    assert!(matches!(
        &coordinated.observation,
        RangeInstallationObservation::ObservedOnAncestry { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_missing_head_record_refuses_the_discovery_stage() {
    // A bogus seed head: the discovery stage's typed refusal,
    // propagated under the composite's own error arm — never
    // collapsed into the observation stage's error model.
    let root = a_root("c5d-bogus");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let _campaign = genuine_campaign(&mut medium, root.clone(), true);
    let bogus = hash_object(b"c5d-not-a-record");
    match composite(&mut medium, &bogus) {
        Err(crate::range_installation::DiscoveredObservationError::Discovery(
            crate::recovery_discovery::DiscoveryError::HeadRecordMissing { head },
        )) => assert_eq!(head, bogus),
        other => panic!("the bogus seed must refuse the discovery (got: {other:?})"),
    }
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
