//! Grouping of the flat announcement list passed to `send_dlc_offer`.

use bitcoin::Amount;
use ddk::group_announcements;
use ddk::oracle::memory::MemoryOracle;
use ddk_testenv::dlc::{
    announce_enum_event, contract_input, enum_descriptor, ContractLeg, EVENT_MATURITY,
};

#[tokio::test]
async fn announcements_are_grouped_per_contract_info() {
    let left_oracle = MemoryOracle::default();
    let right_oracle = MemoryOracle::default();
    let left =
        announce_enum_event(std::slice::from_ref(&left_oracle), "left", EVENT_MATURITY).await;
    let right =
        announce_enum_event(std::slice::from_ref(&right_oracle), "right", EVENT_MATURITY).await;
    let total = Amount::from_sat(100_000);
    let input = contract_input(
        &[
            ContractLeg::new(enum_descriptor(total), left.clone(), 1),
            ContractLeg::new(enum_descriptor(total), right.clone(), 1),
        ],
        total,
        Amount::ZERO,
        2,
    );

    let grouped = group_announcements(&input, vec![left[0].clone(), right[0].clone()]).unwrap();
    assert_eq!(grouped, vec![left.clone(), right.clone()]);

    // Too few, too many, and out of order all fail before the manager runs.
    assert!(group_announcements(&input, vec![left[0].clone()]).is_err());
    assert!(group_announcements(
        &input,
        vec![left[0].clone(), right[0].clone(), right[0].clone()]
    )
    .is_err());
    assert!(group_announcements(&input, vec![right[0].clone(), left[0].clone()]).is_err());
}
