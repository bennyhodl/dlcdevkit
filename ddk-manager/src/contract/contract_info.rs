//! #ContractInfo

use super::AdaptorInfo;
use super::ContractDescriptor;
use crate::error::Error;
use crate::ContractSigner;
use bitcoin::Amount;
use bitcoin::{Script, Transaction};
use ddk_dlc::{OracleInfo, Payout};
use ddk_messages::oracle_msgs;
use ddk_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use ddk_trie::{DlcTrie, RangeInfo};
use secp256k1_zkp::schnorr::Signature as SchnorrSignature;
use secp256k1_zkp::{All, EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey, Verification};
use std::ops::Deref;

pub(super) type OracleIndexAndPrefixLength = Vec<(usize, usize)>;

/// The CET and adaptor signature indexes for an attested outcome, with the
/// oracle signatures that decrypt the adaptor signature: one set per oracle of
/// the matched combination.
pub type RangeInfoAndOracleSignatures = (RangeInfo, Vec<Vec<SchnorrSignature>>);

/// Contains information about the contract conditions and oracles used.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct ContractInfo {
    /// The descriptor for the contract
    pub contract_descriptor: ContractDescriptor,
    /// The oracle announcements used for the contract.
    pub oracle_announcements: Vec<OracleAnnouncement>,
    /// How many oracles are required to provide a compatible outcome to be able
    /// to close the contract.
    pub threshold: usize,
}

impl ContractInfo {
    /// Get the payouts associated with the contract.
    pub fn get_payouts(&self, total_collateral: Amount) -> Result<Vec<Payout>, Error> {
        match &self.contract_descriptor {
            ContractDescriptor::Enum(e) => Ok(e.get_payouts()),
            ContractDescriptor::Numerical(n) => n.get_payouts(total_collateral),
        }
    }

    /// Validate that the descriptor covers all possible outcomes that can be attested
    /// by the oracle(s).
    pub fn validate(&self) -> Result<(), Error> {
        if self.oracle_announcements.is_empty() {
            return Err(Error::InvalidState(
                "ContractInfo doesn't contain any announcement.".to_string(),
            ));
        }

        self.contract_descriptor
            .validate(&self.oracle_announcements)
    }

    /// Utility function returning a set of OracleInfo created using the set
    /// of oracle announcements defined for the contract.
    pub fn get_oracle_infos(&self) -> Vec<OracleInfo> {
        self.oracle_announcements.iter().map(|x| x.into()).collect()
    }

    /// Uses the provided AdaptorInfo and SecretKey to generate the set of
    /// adaptor signatures for the contract.
    pub fn get_adaptor_signatures<S: Deref>(
        &self,
        secp: &Secp256k1<All>,
        adaptor_info: &AdaptorInfo,
        signer: &S,
        funding_witness_script: &Script,
        fund_output_value: Amount,
        cets: &[Transaction],
    ) -> Result<Vec<EcdsaAdaptorSignature>, Error>
    where
        S::Target: ContractSigner,
    {
        let fund_privkey = signer.get_secret_key()?;
        match adaptor_info {
            AdaptorInfo::Enum => match &self.contract_descriptor {
                ContractDescriptor::Enum(e) => e.get_adaptor_signatures(
                    secp,
                    &self.get_oracle_infos(),
                    self.threshold,
                    cets,
                    &fund_privkey,
                    funding_witness_script,
                    fund_output_value,
                ),
                _ => unreachable!(),
            },
            AdaptorInfo::Numerical(trie) => Ok(trie.sign(
                secp,
                &fund_privkey,
                funding_witness_script,
                fund_output_value,
                cets,
                &self.precompute_points(secp)?,
            )?),
            AdaptorInfo::NumericalWithDifference(trie) => Ok(trie.sign(
                secp,
                &fund_privkey,
                funding_witness_script,
                fund_output_value,
                cets,
                &self.precompute_points(secp)?,
            )?),
        }
    }

    /// Generate the AdaptorInfo for the contract while verifying the provided
    /// set of adaptor signatures.
    #[allow(clippy::too_many_arguments)]
    pub fn verify_and_get_adaptor_info(
        &self,
        secp: &Secp256k1<All>,
        total_collateral: Amount,
        fund_pubkey: &PublicKey,
        funding_witness_script: &Script,
        fund_output_value: Amount,
        cets: &[Transaction],
        adaptor_sigs: &[EcdsaAdaptorSignature],
        adaptor_sig_start: usize,
    ) -> Result<(AdaptorInfo, usize), Error> {
        let oracle_infos = self.get_oracle_infos();
        match &self.contract_descriptor {
            ContractDescriptor::Enum(e) => Ok(e.verify_and_get_adaptor_info(
                secp,
                &oracle_infos,
                self.threshold,
                fund_pubkey,
                funding_witness_script,
                fund_output_value,
                cets,
                adaptor_sigs,
                adaptor_sig_start,
            )?),
            ContractDescriptor::Numerical(n) => Ok(n.verify_and_get_adaptor_info(
                secp,
                total_collateral,
                fund_pubkey,
                funding_witness_script,
                fund_output_value,
                self.threshold,
                &self.precompute_points(secp)?,
                cets,
                adaptor_sigs,
                adaptor_sig_start,
            )?),
        }
    }

    /// Tries to find a match in the given adaptor info for the given outcomes.
    pub fn get_range_info_for_outcome(
        &self,
        adaptor_info: &AdaptorInfo,
        outcomes: &[(usize, &Vec<String>)],
        adaptor_sig_start: usize,
    ) -> Option<(OracleIndexAndPrefixLength, RangeInfo)> {
        match adaptor_info {
            AdaptorInfo::Enum => match &self.contract_descriptor {
                ContractDescriptor::Enum(e) => e.get_range_info_for_outcome(
                    self.oracle_announcements.len(),
                    self.threshold,
                    outcomes,
                    adaptor_sig_start,
                ),
                _ => unreachable!(),
            },
            AdaptorInfo::Numerical(n) => {
                let res = n.look_up(&outcomes_to_digits(outcomes))?;
                Some((
                    res.1.iter().map(|(x, y)| (*x, y.len())).collect(),
                    res.0.clone(),
                ))
            }
            AdaptorInfo::NumericalWithDifference(n) => {
                let res = n.multi_trie.look_up(&outcomes_to_digits(outcomes))?;

                Some((
                    res.1.iter().map(|(x, y)| (*x, y.len())).collect(),
                    res.0.clone(),
                ))
            }
        }
    }

    /// Finds the CET matching the attested outcomes and selects the oracle
    /// signatures that decrypt its adaptor signature.
    ///
    /// The adaptor secret is the sum of one signature set per oracle of the
    /// combination the adaptor point was built for, so the attestations must
    /// bind to that index set: every oracle index must be distinct, and every
    /// oracle of the matched combination must have an attestation with at least
    /// as many signatures as the matched outcome prefix. Attestations from
    /// oracles outside the matched combination are not used, so a
    /// `threshold`-of-`n` contract accepts up to `n` attestations.
    ///
    /// Returns `Ok(None)` when no outcome of this contract info matches the
    /// attestations.
    pub fn get_range_info_and_oracle_signatures(
        &self,
        adaptor_info: &AdaptorInfo,
        attestations: &[(usize, OracleAttestation)],
        adaptor_sig_start: usize,
    ) -> Result<Option<RangeInfoAndOracleSignatures>, Error> {
        ensure_distinct_oracle_indices(attestations)?;
        let outcomes: Vec<(usize, &Vec<String>)> = attestations
            .iter()
            .map(|(index, attestation)| (*index, &attestation.outcomes))
            .collect();
        let Some((signature_infos, range_info)) =
            self.get_range_info_for_outcome(adaptor_info, &outcomes, adaptor_sig_start)
        else {
            return Ok(None);
        };
        let signatures = select_oracle_signatures(&signature_infos, attestations)?;
        Ok(Some((range_info, signatures)))
    }

    /// Verifies the given adaptor signatures are valid with respect to the given
    /// adaptor info.
    #[allow(clippy::too_many_arguments)]
    pub fn verify_adaptor_info(
        &self,
        secp: &Secp256k1<All>,
        fund_pubkey: &PublicKey,
        funding_witness_script: &Script,
        fund_output_value: Amount,
        cets: &[Transaction],
        adaptor_sigs: &[EcdsaAdaptorSignature],
        adaptor_sig_start: usize,
        adaptor_info: &AdaptorInfo,
    ) -> Result<usize, Error> {
        let oracle_infos = self.get_oracle_infos();
        match &self.contract_descriptor {
            ContractDescriptor::Enum(e) => Ok(e.verify_adaptor_info(
                secp,
                &oracle_infos,
                self.threshold,
                fund_pubkey,
                funding_witness_script,
                fund_output_value,
                cets,
                adaptor_sigs,
                adaptor_sig_start,
            )?),
            ContractDescriptor::Numerical(_) => match adaptor_info {
                AdaptorInfo::Enum => unreachable!(),
                AdaptorInfo::Numerical(trie) => Ok(trie.verify(
                    secp,
                    fund_pubkey,
                    funding_witness_script,
                    fund_output_value,
                    adaptor_sigs,
                    cets,
                    &self.precompute_points(secp)?,
                )?),
                AdaptorInfo::NumericalWithDifference(trie) => Ok(trie.verify(
                    secp,
                    fund_pubkey,
                    funding_witness_script,
                    fund_output_value,
                    adaptor_sigs,
                    cets,
                    &self.precompute_points(secp)?,
                )?),
            },
        }
    }

    /// Generate the adaptor info and adaptor signatures for the contract.
    #[allow(clippy::too_many_arguments)]
    pub fn get_adaptor_info(
        &self,
        secp: &Secp256k1<All>,
        total_collateral: Amount,
        fund_priv_key: &SecretKey,
        funding_witness_script: &Script,
        fund_output_value: Amount,
        cets: &[Transaction],
        adaptor_index_start: usize,
    ) -> Result<(AdaptorInfo, Vec<EcdsaAdaptorSignature>), Error> {
        match &self.contract_descriptor {
            ContractDescriptor::Enum(e) => {
                let oracle_infos = self.get_oracle_infos();
                Ok(e.get_adaptor_info(
                    secp,
                    &oracle_infos,
                    self.threshold,
                    fund_priv_key,
                    funding_witness_script,
                    fund_output_value,
                    cets,
                )?)
            }
            ContractDescriptor::Numerical(n) => Ok(n.get_adaptor_info(
                secp,
                total_collateral,
                fund_priv_key,
                funding_witness_script,
                fund_output_value,
                self.threshold,
                &self.precompute_points(secp)?,
                cets,
                adaptor_index_start,
            )?),
        }
    }

    fn precompute_points<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
    ) -> Result<Vec<Vec<Vec<PublicKey>>>, Error> {
        self.oracle_announcements
            .iter()
            .map(|x| {
                let pubkey = &x.oracle_public_key;
                let nonces = &x.oracle_event.oracle_nonces;
                match &x.oracle_event.event_descriptor {
                    EventDescriptor::DigitDecompositionEvent(d) => {
                        let base = d.base as usize;
                        let nb_digits = d.nb_digits as usize;
                        if nb_digits != nonces.len() {
                            return Err(Error::InvalidParameters(
                                "Number of digits and nonces must be equal".to_string(),
                            ));
                        }
                        let mut d_points = Vec::with_capacity(nb_digits);
                        for nonce in nonces {
                            let mut points = Vec::with_capacity(base);
                            for j in 0..base {
                                let msg = oracle_msgs::tagged_attestation_msg(&j.to_string());
                                let sig_point = ddk_dlc::secp_utils::schnorrsig_compute_sig_point(
                                    secp, pubkey, nonce, &msg,
                                )?;
                                points.push(sig_point);
                            }
                            d_points.push(points);
                        }
                        Ok(d_points)
                    }
                    _ => Err(Error::InvalidParameters(
                        "Expected digit decomposition event.".to_string(),
                    )),
                }
            })
            .collect::<Result<Vec<Vec<Vec<PublicKey>>>, Error>>()
    }
}

fn get_digits_outcome(input: &[String]) -> Result<Vec<usize>, crate::error::Error> {
    input
        .iter()
        .map(|x| {
            x.parse::<usize>().map_err(|_| {
                crate::error::Error::InvalidParameters(
                    "Invalid outcome, {} is not a valid number.".to_string(),
                )
            })
        })
        .collect::<Result<Vec<usize>, crate::error::Error>>()
}

/// Checks that no oracle index appears more than once in `attestations`.
///
/// A repeated oracle would count twice towards the threshold and its
/// signatures would be summed twice into the adaptor secret.
pub fn ensure_distinct_oracle_indices(
    attestations: &[(usize, OracleAttestation)],
) -> Result<(), Error> {
    let mut indices: Vec<usize> = attestations.iter().map(|(index, _)| *index).collect();
    indices.sort_unstable();
    match indices.windows(2).find(|pair| pair[0] == pair[1]) {
        Some(pair) => Err(Error::InvalidParameters(format!(
            "Oracle {} has more than one attestation",
            pair[0]
        ))),
        None => Ok(()),
    }
}

/// Selects, for each `(oracle index, prefix length)` pair of a matched outcome,
/// the first `prefix length` signatures of that oracle's attestation.
///
/// Each oracle in `signature_infos` must have exactly one attestation carrying
/// at least `prefix length` signatures. A missing oracle or a short attestation
/// is an error, not a silently shorter signature set.
pub fn select_oracle_signatures(
    signature_infos: &[(usize, usize)],
    attestations: &[(usize, OracleAttestation)],
) -> Result<Vec<Vec<SchnorrSignature>>, Error> {
    ensure_distinct_oracle_indices(attestations)?;
    signature_infos
        .iter()
        .map(|(oracle_index, prefix_length)| {
            let (_, attestation) = attestations
                .iter()
                .find(|(index, _)| index == oracle_index)
                .ok_or_else(|| {
                    Error::InvalidParameters(format!(
                        "No attestation for oracle {oracle_index}, which the matched outcome requires"
                    ))
                })?;
            if attestation.signatures.len() < *prefix_length {
                return Err(Error::InvalidParameters(format!(
                    "Attestation from oracle {oracle_index} has {} signatures but the matched outcome needs {prefix_length}",
                    attestation.signatures.len()
                )));
            }
            Ok(attestation.signatures[..*prefix_length].to_vec())
        })
        .collect()
}

fn outcomes_to_digits(outcomes: &[(usize, &Vec<String>)]) -> Vec<(usize, Vec<usize>)> {
    outcomes
        .iter()
        .filter_map(|(x, path)| Some((*x, get_digits_outcome(path).ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::enum_descriptor::EnumDescriptor;
    use ddk_dlc::{EnumerationPayout, Payout};
    use ddk_messages::oracle_msgs::{EnumEventDescriptor, OracleEvent};
    use secp256k1_zkp::XOnlyPublicKey;
    use std::str::FromStr;

    const NB_ORACLES: usize = 3;
    const THRESHOLD: usize = 2;

    fn oracle_public_key() -> XOnlyPublicKey {
        XOnlyPublicKey::from_str("e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443")
            .unwrap()
    }

    fn signature() -> SchnorrSignature {
        SchnorrSignature::from_str("6470FD1303DDA4FDA717B9837153C24A6EAB377183FC438F939E0ED2B620E9EE5077C4A8B8DCA28963D772A94F5F0DDF598E1C47C137F91933274C7C3EDADCE8").unwrap()
    }

    /// An enum contract on `NB_ORACLES` oracles with a `THRESHOLD`. The
    /// signature selection never verifies signatures, so placeholder keys and
    /// signatures are enough.
    fn enum_contract_info() -> ContractInfo {
        let oracle_announcements = (0..NB_ORACLES)
            .map(|_| OracleAnnouncement {
                announcement_signature: signature(),
                oracle_public_key: oracle_public_key(),
                oracle_event: OracleEvent {
                    oracle_nonces: vec![oracle_public_key()],
                    event_maturity_epoch: 0,
                    event_descriptor: EventDescriptor::EnumEvent(EnumEventDescriptor {
                        outcomes: vec!["a".to_string(), "b".to_string()],
                    }),
                    event_id: "test".to_string(),
                },
            })
            .collect();
        ContractInfo {
            contract_descriptor: ContractDescriptor::Enum(EnumDescriptor {
                outcome_payouts: vec![
                    EnumerationPayout {
                        outcome: "a".to_string(),
                        payout: Payout {
                            offer: Amount::from_sat(100),
                            accept: Amount::ZERO,
                        },
                    },
                    EnumerationPayout {
                        outcome: "b".to_string(),
                        payout: Payout {
                            offer: Amount::ZERO,
                            accept: Amount::from_sat(100),
                        },
                    },
                ],
            }),
            oracle_announcements,
            threshold: THRESHOLD,
        }
    }

    fn attestation(outcome: &str) -> OracleAttestation {
        OracleAttestation {
            event_id: "test".to_string(),
            oracle_public_key: oracle_public_key(),
            signatures: vec![signature()],
            outcomes: vec![outcome.to_string()],
        }
    }

    fn lookup(
        attestations: &[(usize, OracleAttestation)],
    ) -> Result<Option<RangeInfoAndOracleSignatures>, Error> {
        enum_contract_info().get_range_info_and_oracle_signatures(
            &AdaptorInfo::Enum,
            attestations,
            0,
        )
    }

    #[test]
    fn exact_threshold_attestations_select_one_signature_set_per_oracle() {
        let (range_info, signatures) = lookup(&[(1, attestation("a")), (2, attestation("a"))])
            .unwrap()
            .expect("outcome a has a CET");
        assert_eq!(range_info.cet_index, 0);
        assert_eq!(signatures.len(), THRESHOLD);
        assert!(signatures.iter().all(|set| set.len() == 1));
    }

    #[test]
    fn attestations_beyond_the_threshold_are_not_summed_into_the_secret() {
        let (_, signatures) = lookup(&[
            (0, attestation("a")),
            (1, attestation("a")),
            (2, attestation("a")),
        ])
        .unwrap()
        .expect("outcome a has a CET");
        assert_eq!(signatures.len(), THRESHOLD);
    }

    #[test]
    fn a_duplicated_oracle_index_is_rejected() {
        let error = lookup(&[(0, attestation("a")), (0, attestation("a"))]).unwrap_err();
        assert!(matches!(error, Error::InvalidParameters(_)), "{error}");
    }

    #[test]
    fn an_attestation_with_too_few_signatures_is_rejected() {
        let mut unsigned = attestation("a");
        unsigned.signatures.clear();
        let error = lookup(&[(0, unsigned), (1, attestation("a"))]).unwrap_err();
        assert!(matches!(error, Error::InvalidParameters(_)), "{error}");
    }

    #[test]
    fn an_unknown_outcome_has_no_range_info() {
        assert!(lookup(&[(0, attestation("c")), (1, attestation("c"))])
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_missing_oracle_in_the_matched_combination_is_rejected() {
        let error =
            select_oracle_signatures(&[(0, 1), (1, 1)], &[(0, attestation("a"))]).unwrap_err();
        assert!(matches!(error, Error::InvalidParameters(_)), "{error}");
    }
}
