//! Oracles, events and contracts for the DLC test suites.
//!
//! `ddk`'s stateless tests and `ddk-manager`'s manager tests need the same
//! things: oracles that announce an event and later attest it, an enum
//! descriptor over a fixed outcome set, a numeric descriptor over a payout
//! curve, and a contract built from those descriptors. Both suites used to
//! build all of it themselves.
//!
//! What they do not share is the layer they hand the contract to.
//! `ddk-manager` takes a [`ContractInput`], which names oracles by public key
//! and event id and lets the manager collect the announcements.
//! `ddk::contract` takes the wire [`ContractInfo`], which carries the
//! announcements itself. A [`ContractLeg`] produces either form from one
//! descriptor.
//!
//! Policy stays with each suite: which oracles sign, what they sign, and what
//! payout curve a numeric contract runs on. The manager tests randomize all
//! three; the stateless tests fix them.

use bitcoin::{Amount, XOnlyPublicKey};
use ddk::oracle::memory::MemoryOracle;
use ddk_dlc::{EnumerationPayout, Payout};
use ddk_manager::contract::contract_input::{ContractInput, ContractInputInfo, OracleInput};
use ddk_manager::contract::enum_descriptor::EnumDescriptor;
use ddk_manager::contract::numerical_descriptor::{DifferenceParams, NumericalDescriptor};
use ddk_manager::contract::ContractDescriptor;
use ddk_manager::payout_curve::{PayoutFunction, RoundingInterval, RoundingIntervals};
use ddk_manager::Oracle;
use ddk_messages::contract_msgs::{
    ContractInfo, ContractInfoInner, DisjointContractInfo, SingleContractInfo,
};
use ddk_messages::oracle_msgs::{
    MultiOracleInfo, OracleAnnouncement, OracleAttestation, OracleInfo, OracleParams,
    SingleOracleInfo,
};
use ddk_trie::OracleNumericInfo;

/// Oracle event maturity, deliberately in the past.
///
/// CET and refund locktimes are derived from it, and regtest block timestamps
/// track the real wall clock, so both are already spendable the moment the
/// funding transaction confirms.
pub const EVENT_MATURITY: u32 = 1_623_133_104;

/// The base a numeric event decomposes its outcome into.
pub const BASE: u32 = 2;

/// How many digits a numeric event announces.
pub const NB_DIGITS: u16 = 10;

/// The exponent bounding the oracle spread a contract with difference params
/// supports.
pub const MIN_SUPPORT_EXP: usize = 1;

/// The exponent bounding the oracle error such a contract tolerates.
pub const MAX_ERROR_EXP: usize = 2;

/// Payouts are rounded to whole satoshis.
pub const ROUNDING_MOD: u64 = 1;

/// The unit a numeric event reports its outcome in.
const NUMERIC_UNIT: &str = "sats";

/// The outcomes of every enum event these suites announce.
pub fn enum_outcomes() -> Vec<String> {
    vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ]
}

/// The largest outcome an event of `nb_digits` digits can attest.
pub fn max_value_from_digits(nb_digits: usize) -> u64 {
    (BASE as u64).pow(nb_digits as u32) - 1
}

/// The largest outcome an event of [`NB_DIGITS`] digits can attest.
pub fn max_value() -> u64 {
    max_value_from_digits(NB_DIGITS as usize)
}

/// The oracle disagreement a numeric contract can be built to tolerate.
pub fn difference_params() -> DifferenceParams {
    DifferenceParams {
        max_error_exp: MAX_ERROR_EXP,
        min_support_exp: MIN_SUPPORT_EXP,
        maximize_coverage: false,
    }
}

/// `nb_oracles` oracles that all decompose an outcome the same way.
pub fn numeric_infos(nb_oracles: usize) -> OracleNumericInfo {
    variable_numeric_infos(&vec![NB_DIGITS as usize; nb_oracles])
}

/// Oracles that decompose an outcome into a digit count each, which do not
/// have to match.
pub fn variable_numeric_infos(nb_digits: &[usize]) -> OracleNumericInfo {
    OracleNumericInfo {
        base: BASE as usize,
        nb_digits: nb_digits.to_vec(),
    }
}

// --- Oracles ---------------------------------------------------------------

/// Creates `nb_oracles` oracles that have announced nothing.
pub fn new_oracles(nb_oracles: usize) -> Vec<MemoryOracle> {
    (0..nb_oracles).map(|_| MemoryOracle::default()).collect()
}

/// The public keys of `oracles`, in order.
pub fn public_keys(oracles: &[MemoryOracle]) -> Vec<XOnlyPublicKey> {
    oracles
        .iter()
        .map(|oracle| oracle.get_public_key())
        .collect()
}

/// Announces `event_id` on every oracle as an enum event over
/// [`enum_outcomes`], maturing at `maturity`.
pub async fn announce_enum_event(
    oracles: &[MemoryOracle],
    event_id: &str,
    maturity: u32,
) -> Vec<OracleAnnouncement> {
    let mut announcements = Vec::with_capacity(oracles.len());
    for oracle in oracles {
        announcements.push(
            oracle
                .oracle
                .create_enum_event(event_id.to_string(), enum_outcomes(), maturity)
                .await
                .expect("oracle could not announce the enum event"),
        );
    }
    announcements
}

/// Announces `event_id` on every oracle as a digit decomposition event, sized
/// from that oracle's entry in `nb_digits`.
pub async fn announce_numeric_event(
    oracles: &[MemoryOracle],
    event_id: &str,
    nb_digits: &[usize],
    maturity: u32,
) -> Vec<OracleAnnouncement> {
    assert_eq!(
        oracles.len(),
        nb_digits.len(),
        "every oracle needs a digit count"
    );
    let mut announcements = Vec::with_capacity(oracles.len());
    for (oracle, digits) in oracles.iter().zip(nb_digits) {
        announcements.push(
            oracle
                .oracle
                .create_numeric_event(
                    event_id.to_string(),
                    *digits as u16,
                    false,
                    0,
                    NUMERIC_UNIT.to_string(),
                    maturity,
                )
                .await
                .expect("oracle could not announce the numeric event"),
        );
    }
    announcements
}

/// Signs `outcome` on the oracles `signers` names by index.
///
/// An oracle that already signed this event is left as it is, rather than
/// failing: a caller may offer several candidate outcomes and let the first
/// one stand.
pub async fn sign_enum_event(
    oracles: &[MemoryOracle],
    event_id: &str,
    signers: &[usize],
    outcome: &str,
) {
    for index in signers {
        let _already_signed = oracles[*index]
            .oracle
            .sign_enum_event(event_id.to_string(), outcome.to_string())
            .await;
    }
}

/// Signs `value` on the oracles `signers` names by index, under the same rule
/// as [`sign_enum_event`].
pub async fn sign_numeric_event(
    oracles: &[MemoryOracle],
    event_id: &str,
    signers: &[usize],
    value: i64,
) {
    for index in signers {
        let _already_signed = oracles[*index]
            .oracle
            .sign_numeric_event(event_id.to_string(), value)
            .await;
    }
}

/// The attestations of the oracles `signers` names, paired with their index.
///
/// An oracle that never signed the event has nothing to attest, and asking for
/// its attestation is a test bug rather than a settlement failure, so it is
/// caught here.
pub async fn attestations(
    oracles: &[MemoryOracle],
    event_id: &str,
    signers: &[usize],
) -> Vec<(usize, OracleAttestation)> {
    let mut attestations = Vec::with_capacity(signers.len());
    for index in signers {
        let attestation = oracles[*index]
            .get_attestation(event_id)
            .await
            .expect("oracle could not produce an attestation");
        assert!(
            !attestation.signatures.is_empty(),
            "oracle {index} did not sign {event_id}"
        );
        attestations.push((*index, attestation));
    }
    attestations
}

// --- Contracts -------------------------------------------------------------

/// An enum descriptor over [`enum_outcomes`] that pays the whole of
/// `total_collateral` to alternating parties.
pub fn enum_descriptor(total_collateral: Amount) -> ContractDescriptor {
    let outcome_payouts = enum_outcomes()
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            let payout = if index % 2 == 0 {
                Payout {
                    offer: total_collateral,
                    accept: Amount::ZERO,
                }
            } else {
                Payout {
                    offer: Amount::ZERO,
                    accept: total_collateral,
                }
            };
            EnumerationPayout { outcome, payout }
        })
        .collect();
    ContractDescriptor::Enum(EnumDescriptor { outcome_payouts })
}

/// A numeric descriptor over `payout_function`, rounded to [`ROUNDING_MOD`].
///
/// The curve is the caller's: the two suites build different ones, and which
/// one a contract runs on is part of what those tests cover.
pub fn numeric_descriptor(
    payout_function: PayoutFunction,
    oracle_numeric_infos: OracleNumericInfo,
    difference_params: Option<DifferenceParams>,
) -> ContractDescriptor {
    ContractDescriptor::Numerical(NumericalDescriptor {
        payout_function,
        rounding_intervals: RoundingIntervals {
            intervals: vec![RoundingInterval {
                begin_interval: 0,
                rounding_mod: ROUNDING_MOD,
            }],
        },
        oracle_numeric_infos,
        difference_params,
    })
}

/// One event of a contract: what it pays out, and the oracles that settle it.
///
/// The announcements carry what both contract forms need — the oracle public
/// keys and the event id for a [`ContractInput`], the announcements themselves
/// for the wire [`ContractInfo`].
pub struct ContractLeg {
    pub descriptor: ContractDescriptor,
    pub announcements: Vec<OracleAnnouncement>,
    pub threshold: u16,
}

impl ContractLeg {
    pub fn new(
        descriptor: ContractDescriptor,
        announcements: Vec<OracleAnnouncement>,
        threshold: u16,
    ) -> Self {
        assert!(
            !announcements.is_empty(),
            "a contract leg needs at least one oracle"
        );
        assert!(
            threshold as usize <= announcements.len(),
            "a threshold cannot ask for more oracles than the leg has"
        );
        Self {
            descriptor,
            announcements,
            threshold,
        }
    }

    /// The event every oracle in this leg announced.
    pub fn event_id(&self) -> &str {
        &self.announcements[0].oracle_event.event_id
    }

    /// The oracle spread this leg tolerates, if it is a numeric one built to
    /// tolerate any.
    fn difference_params(&self) -> Option<&DifferenceParams> {
        match &self.descriptor {
            ContractDescriptor::Numerical(numeric) => numeric.difference_params.as_ref(),
            ContractDescriptor::Enum(_) => None,
        }
    }

    /// The wire oracle info: a single announcement, or several with the
    /// threshold and the spread the contract tolerates.
    pub fn oracle_info(&self) -> OracleInfo {
        let params = self.difference_params().map(|params| OracleParams {
            max_error_exp: params.max_error_exp as u16,
            min_fail_exp: params.min_support_exp as u16,
            maximize_coverage: params.maximize_coverage,
        });
        if self.announcements.len() == 1 && params.is_none() {
            OracleInfo::Single(SingleOracleInfo {
                oracle_announcement: self.announcements[0].clone(),
            })
        } else {
            OracleInfo::Multi(MultiOracleInfo {
                threshold: self.threshold,
                oracle_announcements: self.announcements.clone(),
                oracle_params: params,
            })
        }
    }

    /// This leg in wire form.
    pub fn contract_info_inner(&self) -> ContractInfoInner {
        ContractInfoInner {
            contract_descriptor: (&self.descriptor).into(),
            oracle_info: self.oracle_info(),
        }
    }

    /// This leg as the manager takes it.
    pub fn contract_input_info(&self) -> ContractInputInfo {
        ContractInputInfo {
            contract_descriptor: self.descriptor.clone(),
            oracles: OracleInput {
                public_keys: self
                    .announcements
                    .iter()
                    .map(|announcement| announcement.oracle_public_key)
                    .collect(),
                event_id: self.event_id().to_string(),
                threshold: self.threshold,
            },
        }
    }
}

/// A contract over `legs`, locking `total_collateral`, in wire form.
///
/// One leg makes a single contract; several make a disjoint one, which any of
/// them can settle.
pub fn contract_info(legs: &[ContractLeg], total_collateral: Amount) -> ContractInfo {
    let mut contract_infos: Vec<ContractInfoInner> =
        legs.iter().map(ContractLeg::contract_info_inner).collect();
    if contract_infos.len() == 1 {
        ContractInfo::SingleContractInfo(SingleContractInfo {
            total_collateral,
            contract_info: contract_infos.remove(0),
        })
    } else {
        ContractInfo::DisjointContractInfo(DisjointContractInfo {
            total_collateral,
            contract_infos,
        })
    }
}

/// The manager's input for a contract over `legs`.
pub fn contract_input(
    legs: &[ContractLeg],
    offer_collateral: Amount,
    accept_collateral: Amount,
    fee_rate: u64,
) -> ContractInput {
    ContractInput {
        offer_collateral,
        accept_collateral,
        fee_rate,
        contract_flags: 0,
        contract_infos: legs.iter().map(ContractLeg::contract_input_info).collect(),
    }
}

// --- Splices ---------------------------------------------------------------

/// How one splice round changes the collateral locked in a contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpliceDelta {
    /// Lock this much more, paid in from the splicing party's wallet.
    In(Amount),
    /// Release this much, paid back to the splicing party.
    Out(Amount),
}

impl SpliceDelta {
    /// The total collateral this round produces from a contract holding
    /// `total`.
    pub fn apply(self, total: Amount) -> Amount {
        match self {
            SpliceDelta::In(amount) => total + amount,
            SpliceDelta::Out(amount) => total - amount,
        }
    }

    /// Whether this round adds collateral.
    pub fn is_in(self) -> bool {
        matches!(self, SpliceDelta::In(_))
    }
}
