use super::c5b_sqlite_tests::{genuine_campaign, temp_db};
use crate::range_installation::{
    DiscoveredRangeContext, RangeInstallationLimits, RangeInstallationObservation,
    observe_discovered_range_terminal_under_coordination,
};
use crate::recovery_discovery::DiscoveryLimits;

#[test]
fn the_composite_over_sqlite_observes_the_genuine_campaign() {
    // The provider parity: the same three-stage composite over
    // the SQLite medium — the discovery through the raw
    // connection's reads, the derivation reproducing the
    // campaign's own expected context, and the coordinated
    // observation under the one-DEFERRED-tx scope.
    let (path, mut medium) = temp_db("c5d-positive");
    let campaign = genuine_campaign(&mut medium, true);
    let result = observe_discovered_range_terminal_under_coordination(
        &mut medium,
        &campaign.head,
        &DiscoveryLimits::default(),
        &RangeInstallationLimits::default(),
    )
    .unwrap();
    match &result.derivation {
        DiscoveredRangeContext::V2Context {
            context,
            covering_mk,
            ..
        } => {
            assert_eq!(*context, campaign.expected);
            assert_eq!(*covering_mk, campaign.mk);
        }
        other => panic!("the sqlite campaign must derive (got: {other:?})"),
    }
    let Some(coordinated) = &result.observation else {
        panic!("the derived V2 context must carry the observation");
    };
    assert_eq!(coordinated.coordinated_head, campaign.head);
    assert!(matches!(
        &coordinated.observation,
        RangeInstallationObservation::ObservedOnAncestry { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_composite_over_sqlite_reports_the_absent_terminus() {
    // The in-flight parity: the terminal unpublished, the
    // derivation still derives, the observation reports the
    // absence bound to the coordinated head.
    let (path, mut medium) = temp_db("c5d-absent");
    let campaign = genuine_campaign(&mut medium, false);
    let result = observe_discovered_range_terminal_under_coordination(
        &mut medium,
        &campaign.head,
        &DiscoveryLimits::default(),
        &RangeInstallationLimits::default(),
    )
    .unwrap();
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
        RangeInstallationObservation::NotObservedOnAncestry { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_file(&path);
}
