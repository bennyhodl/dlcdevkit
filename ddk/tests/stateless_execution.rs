//! End-to-end execution tests for the stateless contract API.
//!
//! These are the stateless counterpart to
//! `ddk-manager/tests/manager_execution_tests.rs`. Each test funds real UTXOs
//! on regtest, drives a contract through `create_offer` → `accept_offer` →
//! `sign_accept` → `finalize_sign` using only wire messages, broadcasts the
//! funding transaction to a real node, and then settles the contract with a CET
//! built from real oracle attestations or with the refund transaction. No
//! contract manager, storage backend, or persisted contract state is involved.
//!
//! The matrix covers contract shapes (enum, numeric, numeric with oracle
//! difference tolerance, disjoint, single-funded, spliced), oracle counts and
//! thresholds, both settling parties, and every funding-input signer and DLC
//! funding-key source the API supports.
//!
//! They are `#[ignore]`d because they are slow, not because they need setup:
//! each one boots its own bitcoind and electrs through [`ddk_testenv`] and
//! mines real blocks, so nothing has to be running beforehand.
//!
//! ```sh
//! cargo test --test stateless_execution -- --ignored
//! ```

mod stateless_utils;

use bitcoin::Amount;
use ddk::contract::Party;
use stateless_utils::*;

/// Runs an enum contract to a confirmed funding transaction and settles it with
/// a CET, using whichever oracle set, signers, and key sources are given.
async fn enum_close(
    label: &str,
    nb_oracles: usize,
    threshold: u16,
    closer: Party,
    offer_spec: PartySpec,
    accept_spec: PartySpec,
) {
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::enums(nb_oracles, threshold, label).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&oracles, TOTAL_COLLATERAL),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(&ctx, offer_spec, temporary_contract_id).await,
            TestParty::new(&ctx, accept_spec, temporary_contract_id).await,
        ),
    )
    .await;

    let attestations = oracles.attest_enum(SETTLEMENT_OUTCOME).await;
    close_with_cet(&ctx, &contract, closer, &attestations).await;
}

/// Runs a numeric contract and settles it with a CET.
async fn numeric_close(
    label: &str,
    nb_oracles: usize,
    threshold: u16,
    with_difference: bool,
    closer: Party,
) {
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::numerics(nb_oracles, threshold, label).await;
    let difference = with_difference.then(difference_params);

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            numeric_contract_info(&oracles, OFFER_COLLATERAL, ACCEPT_COLLATERAL, difference),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 11, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 12, 2),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    let attestations = oracles
        .attest_numeric(SETTLEMENT_VALUE, with_difference)
        .await;
    close_with_cet(&ctx, &contract, closer, &attestations).await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_close() {
    enum_close(
        "enum_single_oracle_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 1, 1),
        PartySpec::new(Party::Accept, 2, 2),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_close_by_accept_party() {
    enum_close(
        "enum_single_oracle_close_by_accept_party",
        1,
        1,
        Party::Accept,
        PartySpec::new(Party::Offer, 3, 1),
        PartySpec::new(Party::Accept, 4, 2),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_three_of_three_oracles_close() {
    enum_close(
        "enum_three_of_three_oracles_close",
        3,
        3,
        Party::Offer,
        PartySpec::new(Party::Offer, 5, 1),
        PartySpec::new(Party::Accept, 6, 2),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_three_of_five_oracles_close() {
    enum_close(
        "enum_three_of_five_oracles_close",
        5,
        3,
        Party::Accept,
        PartySpec::new(Party::Offer, 7, 1),
        PartySpec::new(Party::Accept, 8, 2),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn numeric_single_oracle_close() {
    numeric_close("numeric_single_oracle_close", 1, 1, false, Party::Offer).await;
}

#[tokio::test]
#[ignore]
async fn numeric_three_of_three_oracles_close() {
    numeric_close(
        "numeric_three_of_three_oracles_close",
        3,
        3,
        false,
        Party::Accept,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn numeric_two_of_five_oracles_close() {
    numeric_close(
        "numeric_two_of_five_oracles_close",
        5,
        2,
        false,
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn numeric_with_difference_three_of_three_oracles_close() {
    numeric_close(
        "numeric_with_difference_three_of_three_oracles_close",
        3,
        3,
        true,
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn numeric_with_difference_three_of_five_oracles_close() {
    numeric_close(
        "numeric_with_difference_three_of_five_oracles_close",
        5,
        3,
        true,
        Party::Accept,
    )
    .await;
}

/// A disjoint contract settles from whichever of its two events attests first.
#[tokio::test]
#[ignore]
async fn disjoint_contract_closes_on_the_enum_event() {
    let label = "disjoint_contract_closes_on_the_enum_event";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let enum_oracles = TestOracles::enums(1, 1, &format!("{label}-enum")).await;
    let numeric_oracles = TestOracles::numerics(1, 1, &format!("{label}-numeric")).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            disjoint_contract_info(
                &enum_oracles,
                &numeric_oracles,
                OFFER_COLLATERAL,
                ACCEPT_COLLATERAL,
            ),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 21, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 22, 2),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    let attestations = enum_oracles.attest_enum("b").await;
    close_with_cet(&ctx, &contract, Party::Offer, &attestations).await;
}

#[tokio::test]
#[ignore]
async fn disjoint_contract_closes_on_the_numeric_event() {
    let label = "disjoint_contract_closes_on_the_numeric_event";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let enum_oracles = TestOracles::enums(1, 1, &format!("{label}-enum")).await;
    let numeric_oracles = TestOracles::numerics(1, 1, &format!("{label}-numeric")).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            disjoint_contract_info(
                &enum_oracles,
                &numeric_oracles,
                OFFER_COLLATERAL,
                ACCEPT_COLLATERAL,
            ),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 23, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 24, 2),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    let attestations = numeric_oracles.attest_numeric(700, false).await;
    close_with_cet(&ctx, &contract, Party::Accept, &attestations).await;
}

/// The offering party funds the whole contract; the accepting party contributes
/// no inputs and no collateral.
#[tokio::test]
#[ignore]
async fn single_funded_contract_closes() {
    let label = "single_funded_contract_closes";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::enums(1, 1, label).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&oracles, TOTAL_COLLATERAL),
            TOTAL_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 31, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::unfunded(Party::Accept, 32),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    assert_eq!(contract.accept.accept_collateral, Amount::ZERO);
    assert!(contract.accept.funding_inputs.is_empty());
    assert_eq!(contract.funding_transaction.input.len(), 1);

    let attestations = oracles.attest_enum("c").await;
    close_with_cet(&ctx, &contract, Party::Offer, &attestations).await;
}

/// Both parties agree to walk away: neither event is attested and the refund
/// transaction, signed during the offer/accept exchange, is broadcast.
#[tokio::test]
#[ignore]
async fn refund_closes_the_contract() {
    let label = "refund_closes_the_contract";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::enums(1, 1, label).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&oracles, TOTAL_COLLATERAL),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 33, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 34, 2),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    let refund = close_with_refund(&ctx, &contract, Party::Offer).await;
    assert_eq!(refund.output.len(), 2);
}

#[tokio::test]
#[ignore]
async fn refund_closes_the_contract_from_the_accept_party() {
    let label = "refund_closes_the_contract_from_the_accept_party";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::enums(1, 1, label).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&oracles, TOTAL_COLLATERAL),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 35, 1),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 36, 2),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    close_with_refund(&ctx, &contract, Party::Accept).await;
}

/// Several inputs per party, with serial ids that interleave across parties so
/// the witness-to-input mapping cannot rely on ordering.
#[tokio::test]
#[ignore]
async fn multiple_inputs_with_interleaved_serial_ids_close() {
    let label = "multiple_inputs_with_interleaved_serial_ids_close";
    let ctx = ChainContext::new(label).await;
    let temporary_contract_id = temporary_contract_id(label);
    let oracles = TestOracles::enums(1, 1, label).await;

    let contract = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&oracles, TOTAL_COLLATERAL),
            OFFER_COLLATERAL,
            temporary_contract_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 37, 0)
                    .with_utxos(vec![(UTXO_VALUE, 900), (UTXO_VALUE, 5)]),
                temporary_contract_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 38, 0)
                    .with_utxos(vec![(UTXO_VALUE, 37), (UTXO_VALUE, 1_200)]),
                temporary_contract_id,
            )
            .await,
        ),
    )
    .await;

    assert_eq!(contract.funding_transaction.input.len(), 4);
    let attestations = oracles.attest_enum("d").await;
    close_with_cet(&ctx, &contract, Party::Offer, &attestations).await;
}

// --- Splicing -------------------------------------------------------------
//
// A splice spends the previous contract's 2-of-2 funding output as an input to
// the new contract. Both parties' previous funding keys are recomputed from the
// previous contract's temporary id, never stored.
//
// The matrix mirrors the splice paths of
// `ddk-manager/tests/manager_execution_tests.rs`: both contract shapes and a
// disjoint one, one and several oracles, thresholds below the oracle count,
// either party offering the splice, either party settling what it produced,
// collateral going in and out, and chains of several splices before
// settlement.

/// The collateral each round adds to or removes from the funding output.
const SPLICE_AMOUNT: Amount = Amount::from_sat(100_000);

/// One round of a splice chain: who offers the replacement contract, and which
/// way the collateral moves.
#[derive(Clone, Copy)]
struct SpliceRound {
    /// The side of the contract being spliced that offers the replacement.
    ///
    /// Both sides can splice: the party that offers the replacement is credited
    /// with the whole of the previous funding output, whichever side of that
    /// contract it was, so it also puts up the whole of the new collateral.
    splicer: Party,
    delta: SpliceDelta,
}

impl SpliceRound {
    fn splice_in(splicer: Party) -> Self {
        Self {
            splicer,
            delta: SpliceDelta::In(SPLICE_AMOUNT),
        }
    }

    fn splice_out(splicer: Party) -> Self {
        Self {
            splicer,
            delta: SpliceDelta::Out(SPLICE_AMOUNT),
        }
    }
}

/// Funds a contract of `shape`, replaces it once per round by splicing its
/// funding output into the next one, and settles whatever the last round
/// produced from `closer`.
///
/// Every contract in the chain has the shape the scenario named, over oracles
/// and an event of its own, so each round settles on its own attestation. Each
/// replacement is single-funded by the party that offers it: the splice input
/// carries the previous contract's whole funded amount, with
/// [`SPLICE_AMOUNT`] added from that party's wallet or returned to it.
async fn splice_chain_and_close(
    label: &str,
    shape: ContractShape,
    rounds: &[SpliceRound],
    closer: Party,
) {
    let ctx = ChainContext::new(label).await;

    // The contract the first round splices.
    let first_label = format!("{label}-0");
    let first_id = temporary_contract_id(&first_label);
    let first = ShapedContract::new(shape, &first_label, OFFER_COLLATERAL, ACCEPT_COLLATERAL).await;
    let mut previous = fund_contract(
        &ctx,
        ContractSetup::new(
            first.contract_info.clone(),
            OFFER_COLLATERAL,
            first_id,
            TestParty::new(&ctx, PartySpec::new(Party::Offer, 41, 1), first_id).await,
            TestParty::new(&ctx, PartySpec::new(Party::Accept, 42, 2), first_id).await,
        ),
    )
    .await;

    let mut settled = None;
    for (index, round) in rounds.iter().enumerate() {
        let number = index + 1;
        let round_label = format!("{label}-{number}");
        let collateral = round.delta.apply(previous.fund_value());
        // Only a numeric payout curve reads the split between the two sides;
        // every shape locks their sum.
        let offer_share = collateral / 2;
        let spliced_shape =
            ShapedContract::new(shape, &round_label, offer_share, collateral - offer_share).await;

        let spliced_id = temporary_contract_id(&round_label);
        let seed_byte = 43 + index as u8 * 2;
        let offer_spec = if round.delta.is_in() {
            PartySpec::new(Party::Offer, seed_byte, 20 + number as u64)
        } else {
            PartySpec::unfunded(Party::Offer, seed_byte)
        };
        let spliced = fund_contract(
            &ctx,
            ContractSetup::new(
                spliced_shape.contract_info.clone(),
                collateral,
                spliced_id,
                TestParty::new(&ctx, offer_spec, spliced_id).await,
                TestParty::new(
                    &ctx,
                    PartySpec::unfunded(Party::Accept, seed_byte + 1),
                    spliced_id,
                )
                .await,
            )
            .with_splice(previous.splice_setup_by(round.splicer, 900 + number as u64)),
        )
        .await;

        assert_splice(&ctx, &previous, &spliced, round.delta.is_in()).await;

        previous = spliced;
        settled = Some(spliced_shape);
    }

    let attestations = settled
        .expect("a splice chain to have run a round")
        .attest()
        .await;
    close_with_cet(&ctx, &previous, closer, &attestations).await;
}

/// A single splice of a contract of `shape`, settled by `closer`.
async fn splice_and_close(label: &str, shape: ContractShape, round: SpliceRound, closer: Party) {
    splice_chain_and_close(label, shape, &[round], closer).await;
}

/// Asserts that `spliced` replaced `previous`: it spends the previous funding
/// output with a complete 2-of-2 witness, and moves the funded amount the way
/// the splice asked for.
async fn assert_splice(
    ctx: &ChainContext,
    previous: &FundedContract,
    spliced: &FundedContract,
    splice_in: bool,
) {
    ctx.assert_spent_by(previous.fund_outpoint(), &spliced.funding_transaction)
        .await;
    let splice_input = spliced
        .funding_transaction
        .input
        .iter()
        .find(|input| input.previous_output == previous.fund_outpoint())
        .expect("the spliced funding transaction must spend the previous funding output");
    assert_eq!(splice_input.witness.len(), 4, "expected a 2-of-2 witness");
    assert!(splice_input.witness[0].is_empty());

    if splice_in {
        assert!(
            spliced.fund_value() > previous.fund_value(),
            "a splice-in must increase the funded amount"
        );
    } else {
        assert!(
            spliced.fund_value() < previous.fund_value(),
            "a splice-out must decrease the funded amount"
        );
    }
}

// Enum contracts, one and several oracles.

#[tokio::test]
#[ignore]
async fn splice_in_enum_single_oracle_closes() {
    splice_and_close(
        "splice_in_enum_single_oracle_closes",
        ContractShape::enums(1, 1),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_single_oracle_closes() {
    splice_and_close(
        "splice_out_enum_single_oracle_closes",
        ContractShape::enums(1, 1),
        SpliceRound::splice_out(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_three_of_three_oracles_closes() {
    splice_and_close(
        "splice_in_enum_three_of_three_oracles_closes",
        ContractShape::enums(3, 3),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

/// A threshold below the oracle count, settled by the accepting party.
#[tokio::test]
#[ignore]
async fn splice_out_enum_three_of_five_oracles_closes() {
    splice_and_close(
        "splice_out_enum_three_of_five_oracles_closes",
        ContractShape::enums(5, 3),
        SpliceRound::splice_out(Party::Offer),
        Party::Accept,
    )
    .await;
}

// Numeric contracts, with and without a tolerated oracle spread.

#[tokio::test]
#[ignore]
async fn splice_in_numeric_single_oracle_closes() {
    splice_and_close(
        "splice_in_numeric_single_oracle_closes",
        ContractShape::numeric(1, 1),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numeric_single_oracle_closes() {
    splice_and_close(
        "splice_out_numeric_single_oracle_closes",
        ContractShape::numeric(1, 1),
        SpliceRound::splice_out(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numeric_three_of_three_oracles_closes() {
    splice_and_close(
        "splice_in_numeric_three_of_three_oracles_closes",
        ContractShape::numeric(3, 3),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numeric_with_difference_three_of_five_oracles_closes() {
    splice_and_close(
        "splice_in_numeric_with_difference_three_of_five_oracles_closes",
        ContractShape::numeric_with_difference(5, 3),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numeric_with_difference_three_of_five_oracles_closes() {
    splice_and_close(
        "splice_out_numeric_with_difference_three_of_five_oracles_closes",
        ContractShape::numeric_with_difference(5, 3),
        SpliceRound::splice_out(Party::Offer),
        Party::Accept,
    )
    .await;
}

// Disjoint contracts: the spliced contract settles on whichever of its two
// events is attested.

#[tokio::test]
#[ignore]
async fn splice_in_disjoint_three_of_five_oracles_closes_on_the_enum_event() {
    splice_and_close(
        "splice_in_disjoint_three_of_five_oracles_closes_on_the_enum_event",
        ContractShape::disjoint(5, 3, DisjointEvent::Enum),
        SpliceRound::splice_in(Party::Offer),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_disjoint_three_of_five_oracles_closes_on_the_numeric_event() {
    splice_and_close(
        "splice_out_disjoint_three_of_five_oracles_closes_on_the_numeric_event",
        ContractShape::disjoint(5, 3, DisjointEvent::Numeric),
        SpliceRound::splice_out(Party::Offer),
        Party::Accept,
    )
    .await;
}

// Splices offered by the accepting party.
//
// This is the case the stateful manager got wrong: the two keys in a
// [`DlcInput`] are ordered by who offers the splice, not by who offered the
// contract being spliced, and the party that reads them has to agree.

#[tokio::test]
#[ignore]
async fn splice_in_by_accept_party_closes() {
    splice_and_close(
        "splice_in_by_accept_party_closes",
        ContractShape::enums(1, 1),
        SpliceRound::splice_in(Party::Accept),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_by_accept_party_closes() {
    splice_and_close(
        "splice_out_by_accept_party_closes",
        ContractShape::enums(1, 1),
        SpliceRound::splice_out(Party::Accept),
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_three_of_five_oracles_by_accept_party_closes() {
    splice_and_close(
        "splice_in_enum_three_of_five_oracles_by_accept_party_closes",
        ContractShape::enums(5, 3),
        SpliceRound::splice_in(Party::Accept),
        Party::Accept,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numeric_with_difference_three_of_five_oracles_by_accept_party_closes() {
    splice_and_close(
        "splice_out_numeric_with_difference_three_of_five_oracles_by_accept_party_closes",
        ContractShape::numeric_with_difference(5, 3),
        SpliceRound::splice_out(Party::Accept),
        Party::Accept,
    )
    .await;
}

// Chains: each round splices the contract the previous round produced, so
// every round after the first proves a spliced contract is itself spliceable.

#[tokio::test]
#[ignore]
async fn splice_chain_in_out_in_enum_closes() {
    splice_chain_and_close(
        "splice_chain_in_out_in_enum_closes",
        ContractShape::enums(1, 1),
        &[
            SpliceRound::splice_in(Party::Offer),
            SpliceRound::splice_out(Party::Offer),
            SpliceRound::splice_in(Party::Offer),
        ],
        Party::Offer,
    )
    .await;
}

/// A chain where the two sides take turns splicing.
#[tokio::test]
#[ignore]
async fn splice_chain_alternating_parties_closes() {
    splice_chain_and_close(
        "splice_chain_alternating_parties_closes",
        ContractShape::enums(1, 1),
        &[
            SpliceRound::splice_in(Party::Offer),
            SpliceRound::splice_out(Party::Accept),
            SpliceRound::splice_in(Party::Accept),
        ],
        Party::Offer,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_chain_multi_oracle_enum_closes() {
    splice_chain_and_close(
        "splice_chain_multi_oracle_enum_closes",
        ContractShape::enums(3, 3),
        &[
            SpliceRound::splice_in(Party::Offer),
            SpliceRound::splice_out(Party::Accept),
        ],
        Party::Accept,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_chain_numeric_closes() {
    splice_chain_and_close(
        "splice_chain_numeric_closes",
        ContractShape::numeric(1, 1),
        &[
            SpliceRound::splice_in(Party::Offer),
            SpliceRound::splice_out(Party::Accept),
        ],
        Party::Accept,
    )
    .await;
}

// --- Funding input signers ------------------------------------------------
//
// The same lifecycle, with each party's wallet UTXOs signed by a different
// source. The contract logic is identical in every case; only who produces the
// funding witnesses changes.

#[tokio::test]
#[ignore]
async fn xpriv_signer_funds_and_closes() {
    enum_close(
        "xpriv_signer_funds_and_closes",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 51, 1).with_input_source(InputSource::Xpriv),
        PartySpec::new(Party::Accept, 52, 2).with_input_source(InputSource::Xpriv),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn descriptor_signer_funds_and_closes() {
    enum_close(
        "descriptor_signer_funds_and_closes",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 53, 1).with_input_source(InputSource::Descriptor),
        PartySpec::new(Party::Accept, 54, 2).with_input_source(InputSource::Descriptor),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn ddk_wallet_signer_funds_and_closes() {
    enum_close(
        "ddk_wallet_signer_funds_and_closes",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 55, 1).with_input_source(InputSource::DdkWallet),
        PartySpec::new(Party::Accept, 56, 2).with_input_source(InputSource::DdkWallet),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn external_signer_funds_and_closes() {
    enum_close(
        "external_signer_funds_and_closes",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 57, 1).with_input_source(InputSource::ExternalSigner),
        PartySpec::new(Party::Accept, 58, 2).with_input_source(InputSource::ExternalSigner),
    )
    .await;
}

/// The two parties do not have to use the same signer, and a hardware-style
/// external signer interoperates with a DDK wallet.
#[tokio::test]
#[ignore]
async fn mixed_signers_fund_and_close() {
    enum_close(
        "mixed_signers_fund_and_close",
        1,
        1,
        Party::Accept,
        PartySpec::new(Party::Offer, 59, 1).with_input_source(InputSource::ExternalSigner),
        PartySpec::new(Party::Accept, 60, 2).with_input_source(InputSource::DdkWallet),
    )
    .await;
}

// --- DLC funding key sources ----------------------------------------------
//
// The key controlling the 2-of-2 output, the CET adaptor signatures, and the
// refund signature. Every provider variant derives it from the contract's
// temporary id, so it is recomputable rather than stored.

#[tokio::test]
#[ignore]
async fn raw_funding_keys_fund_and_close() {
    enum_close(
        "raw_funding_keys_fund_and_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 61, 1).with_funding_key_source(FundingKeySource::RawSecretKey),
        PartySpec::new(Party::Accept, 62, 2)
            .with_funding_key_source(FundingKeySource::RawSecretKey),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn xprv_derived_funding_keys_fund_and_close() {
    enum_close(
        "xprv_derived_funding_keys_fund_and_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 63, 1).with_funding_key_source(FundingKeySource::Xprv),
        PartySpec::new(Party::Accept, 64, 2).with_funding_key_source(FundingKeySource::Xprv),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn seed_derived_funding_keys_fund_and_close() {
    enum_close(
        "seed_derived_funding_keys_fund_and_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 65, 1).with_funding_key_source(FundingKeySource::Seed),
        PartySpec::new(Party::Accept, 66, 2).with_funding_key_source(FundingKeySource::Seed),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn mnemonic_derived_funding_keys_fund_and_close() {
    enum_close(
        "mnemonic_derived_funding_keys_fund_and_close",
        1,
        1,
        Party::Accept,
        PartySpec::new(Party::Offer, 67, 1).with_funding_key_source(FundingKeySource::Mnemonic),
        PartySpec::new(Party::Accept, 68, 2).with_funding_key_source(FundingKeySource::Mnemonic),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn descriptor_derived_funding_keys_fund_and_close() {
    enum_close(
        "descriptor_derived_funding_keys_fund_and_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 69, 1).with_funding_key_source(FundingKeySource::Descriptor),
        PartySpec::new(Party::Accept, 70, 2).with_funding_key_source(FundingKeySource::Descriptor),
    )
    .await;
}

/// Each party can pick its own key source independently.
#[tokio::test]
#[ignore]
async fn mixed_funding_key_sources_fund_and_close() {
    enum_close(
        "mixed_funding_key_sources_fund_and_close",
        1,
        1,
        Party::Offer,
        PartySpec::new(Party::Offer, 71, 1)
            .with_funding_key_source(FundingKeySource::Mnemonic)
            .with_input_source(InputSource::Descriptor),
        PartySpec::new(Party::Accept, 72, 2)
            .with_funding_key_source(FundingKeySource::RawSecretKey)
            .with_input_source(InputSource::DdkWallet),
    )
    .await;
}

/// Splicing needs the *previous* contract's funding key, which a
/// [`ddk::contract::ContractKeyProvider`] recomputes from the previous
/// temporary contract id. This runs the splice with mnemonic-backed providers
/// on both sides to exercise that recovery path.
#[tokio::test]
#[ignore]
async fn splice_recovers_prior_keys_from_a_mnemonic() {
    let label = "splice_recovers_prior_keys_from_a_mnemonic";
    let ctx = ChainContext::new(label).await;

    let previous_id = temporary_contract_id(&format!("{label}-previous"));
    let previous_oracles = TestOracles::enums(1, 1, &format!("{label}-previous")).await;
    let previous = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&previous_oracles, TOTAL_COLLATERAL),
            OFFER_COLLATERAL,
            previous_id,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Offer, 81, 1)
                    .with_funding_key_source(FundingKeySource::Mnemonic),
                previous_id,
            )
            .await,
            TestParty::new(
                &ctx,
                PartySpec::new(Party::Accept, 82, 2)
                    .with_funding_key_source(FundingKeySource::Mnemonic),
                previous_id,
            )
            .await,
        ),
    )
    .await;

    // Both parties are rebuilt from scratch for the new contract: their new
    // funding keys come from the new temporary id, and the keys for the
    // previous 2-of-2 are recomputed from the previous one.
    let collateral = previous.fund_value() - Amount::from_sat(100_000);
    let spliced_id = temporary_contract_id(&format!("{label}-spliced"));
    let spliced_oracles = TestOracles::enums(1, 1, &format!("{label}-spliced")).await;
    let offerer = TestParty::new(
        &ctx,
        PartySpec::unfunded(Party::Offer, 81).with_funding_key_source(FundingKeySource::Mnemonic),
        spliced_id,
    )
    .await;
    let accepter = TestParty::new(
        &ctx,
        PartySpec::unfunded(Party::Accept, 82).with_funding_key_source(FundingKeySource::Mnemonic),
        spliced_id,
    )
    .await;
    assert_ne!(
        offerer.funding_pubkey(),
        previous.offerer.funding_pubkey(),
        "the spliced contract must use a fresh funding key"
    );
    let splice = splice_from(&previous, Party::Offer, &offerer, &accepter, 900);

    let spliced = fund_contract(
        &ctx,
        ContractSetup::new(
            enum_contract_info(&spliced_oracles, collateral),
            collateral,
            spliced_id,
            offerer,
            accepter,
        )
        .with_splice(splice),
    )
    .await;

    ctx.assert_spent_by(previous.fund_outpoint(), &spliced.funding_transaction)
        .await;

    let attestations = spliced_oracles.attest_enum("a").await;
    close_with_cet(&ctx, &spliced, Party::Accept, &attestations).await;
}
