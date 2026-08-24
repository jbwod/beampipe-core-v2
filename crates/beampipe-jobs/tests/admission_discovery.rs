use beampipe_adapters::{all_reachable, TapEndpointStatus, TapHealthReport};
use beampipe_domain::SkipReason;
use std::collections::BTreeMap;

#[test]
fn tap_unreachable_maps_to_skip_reason() {
    let report = TapHealthReport {
        endpoints: BTreeMap::from([(
            "catalog".into(),
            TapEndpointStatus {
                configured: true,
                reachable: false,
            },
        )]),
    };
    assert!(!all_reachable(&report, &["catalog".into()]));
    assert_eq!(SkipReason::TapUnreachable.as_str(), "tap_unreachable");
}

#[test]
fn max_batches_per_tick_reason_label() {
    assert_eq!(
        SkipReason::MaxBatchesPerTick.as_str(),
        "max_batches_per_tick"
    );
}
