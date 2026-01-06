# ddk-payouts

[![Crate](https://img.shields.io/crates/v/ddk-payouts.svg?logo=rust)](https://crates.io/crates/ddk-payouts)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-payouts&color=informational)](https://docs.rs/ddk-payouts)

Utilities for building payout curves and contract inputs for DLC contracts.

This crate supports both enumeration-based discrete outcomes and numerical continuous payout functions including options contracts.

## Enumeration Contracts

For discrete outcomes like sports events or binary choices:

```rust
use bitcoin::Amount;
use ddk_dlc::EnumerationPayout;
use ddk_payouts::enumeration;

let outcome_payouts = vec![
    EnumerationPayout {
        outcome: "TeamA_Wins".to_string(),
        payout: Payout { offer: 100_000, accept: 0 },
    },
    EnumerationPayout {
        outcome: "TeamB_Wins".to_string(),
        payout: Payout { offer: 0, accept: 100_000 },
    },
];

let contract = enumeration::create_contract_input(
    outcome_payouts,
    Amount::from_sat(50_000),  // offer_collateral
    Amount::from_sat(50_000),  // accept_collateral
    2,                          // fee_rate
    oracle_pubkey,
    event_id,
);
```

## Numerical Contracts

For price-based continuous payout curves:

```rust
use bitcoin::Amount;
use ddk_payouts::create_contract_input;

let contract = create_contract_input(
    0,                           // min_price
    100_000,                     // max_price
    10,                          // num_steps
    Amount::from_sat(50_000),    // offer_collateral
    Amount::from_sat(50_000),    // accept_collateral
    2,                           // fee_rate
    oracle_pubkey,
    event_id,
);
```

## Options Contracts

For call/put options with strike prices:

```rust
use bitcoin::Amount;
use ddk_payouts::options::{build_option_order_offer, Direction, OptionType};

let contract = build_option_order_offer(
    &oracle_announcement,
    Amount::ONE_BTC,             // contract_size
    50_000,                      // strike_price
    Amount::from_sat(100_000),   // premium
    2,                           // fee_per_byte
    100,                         // rounding
    OptionType::Call,
    Direction::Long,
    Amount::from_sat(1_000_000), // total_collateral
    20,                          // nb_oracle_digits
)?;
```

## Key Functions

| Function | Description |
|----------|-------------|
| `create_contract_input` | Build numerical contract with linear payout curve |
| `generate_payout_curve` | Create a `PayoutFunction` with custom parameters |
| `enumeration::create_contract_input` | Build enumeration contract from discrete outcomes |
| `options::build_option_order_offer` | Build options contract (call/put, long/short) |

## License

This project is licensed under the MIT License.
