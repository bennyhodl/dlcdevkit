//! #Manager a component to create and update DLCs.

use super::{
    Blockchain, CachedContractSignerProvider, ConfirmationStatus, ContractSigner, Oracle, Storage,
    Time, Wallet,
};
use crate::contract::{
    accepted_contract::AcceptedContract, contract_info::ContractInfo,
    contract_input::ContractInput, contract_input::OracleInput, offered_contract::OfferedContract,
    signed_contract::SignedContract, AdaptorInfo, ClosedContract, Contract, FailedAcceptContract,
    FailedSignContract, PreClosedContract,
};
use crate::contract_updater::{accept_contract, verify_accepted_and_sign_contract};
use crate::error::Error;
use crate::utils::get_object_in_state;
use crate::{ContractId, ContractSignerProvider};
use bitcoin::absolute::Height;
use bitcoin::hex::DisplayHex;
use bitcoin::Transaction;
use bitcoin::{Address, Amount};
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use ddk_messages::{AcceptDlc, CloseDlc, Message as DlcMessage, OfferDlc, SignDlc};
use futures::stream;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use lightning::util::logger::Logger;
use lightning::{log_debug, log_error, log_info, log_trace, log_warn};
use once_cell::sync::Lazy;
use secp256k1_zkp::XOnlyPublicKey;
use secp256k1_zkp::{All, PublicKey, Secp256k1};
use std::collections::HashMap;
use std::ops::Deref;
use std::string::ToString;
use std::sync::Arc;

/// The number of confirmations required before moving the the confirmed state.
pub const DEFAULT_NB_CONFIRMATIONS: u32 = 3;

/// The number of confirmations required before moving the the confirmed state.
/// Uses the NB_CONFIRMATIONS environment variable to set the value.
static NB_CONFIRMATIONS: Lazy<u32> = Lazy::new(|| match std::env::var("NB_CONFIRMATIONS") {
    Ok(val) => val.parse().unwrap_or(DEFAULT_NB_CONFIRMATIONS),
    Err(_) => DEFAULT_NB_CONFIRMATIONS,
});

/// Automatically broadcast the refund transaction in `check_confirmed_contracts`
static AUTOMATIC_REFUND: Lazy<bool> = Lazy::new(|| match std::env::var("AUTOMATIC_REFUND") {
    Ok(val) => val.parse().unwrap_or(true),
    Err(_) => true,
});

/// The delay to set the refund value to.
pub const REFUND_DELAY: u32 = 86400 * 7;
/// Tolerance for local clock skew: an oracle event is only considered matured
/// once it is this many seconds past its maturity epoch, so a fast local clock
/// cannot trigger a premature CET broadcast.
pub const MATURITY_SKEW_SECS: u64 = 3600;
type ClosableContractInfo<'a> = Option<(
    &'a ContractInfo,
    &'a AdaptorInfo,
    Vec<(usize, OracleAttestation)>,
)>;

/// Application hook consulted before a counterparty's cooperative close
/// proposal is broadcast.
///
/// A valid signature on a [`CloseDlc`] is not consent: the counterparty
/// chooses `accept_payout` freely, up to the full collateral. The application
/// must check the proposed payout against its own expectation (oracle state,
/// market price, user confirmation, ...) and return `Ok(true)` only when the
/// terms are acceptable. When no approver is set on the [`Manager`], every
/// counterparty close proposal is rejected.
pub trait CooperativeCloseApprover: Send + Sync {
    /// Returns whether the close proposal paying `accept_payout` to the
    /// counterparty of the contract with the given id should be broadcast.
    fn approve_counterparty_close(
        &self,
        contract_id: ContractId,
        accept_payout: Amount,
    ) -> Result<bool, Error>;
}

impl std::fmt::Debug for dyn CooperativeCloseApprover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CooperativeCloseApprover")
    }
}

/// Used to create and update DLCs.
#[derive(Debug)]
pub struct Manager<
    W: Deref,
    SP: Deref,
    B: Deref,
    S: Deref,
    O: Deref,
    T: Deref,
    X: ContractSigner,
    L: Deref,
> where
    W::Target: Wallet,
    SP::Target: ContractSignerProvider<Signer = X>,
    B::Target: Blockchain,
    S::Target: Storage,
    O::Target: Oracle,
    T::Target: Time,
    L::Target: Logger,
{
    oracles: HashMap<XOnlyPublicKey, O>,
    wallet: W,
    signer_provider: SP,
    blockchain: B,
    store: S,
    secp: Secp256k1<All>,
    time: T,
    logger: L,
    close_approver: Option<Arc<dyn CooperativeCloseApprover>>,
}

macro_rules! get_contract_in_state {
    ($manager: ident, $contract_id: expr, $state: ident, $peer_id: expr) => {{
        get_object_in_state!(
            $manager,
            $contract_id,
            $state,
            $peer_id,
            Contract,
            get_contract
        )
    }};
}

impl<W: Deref, SP: Deref, B: Deref, S: Deref, O: Deref, T: Deref, X: ContractSigner, L: Deref>
    Manager<W, Arc<CachedContractSignerProvider<SP, X>>, B, S, O, T, X, L>
where
    W::Target: Wallet,
    SP::Target: ContractSignerProvider<Signer = X>,
    B::Target: Blockchain,
    S::Target: Storage,
    O::Target: Oracle,
    T::Target: Time,
    L::Target: Logger,
{
    /// Create a new Manager struct.
    ///
    /// `close_approver` is consulted before a counterparty's cooperative close
    /// proposal is broadcast; with `None`, [`Manager::accept_cooperative_close`]
    /// rejects every proposal.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(skip_all)]
    pub async fn new(
        wallet: W,
        signer_provider: SP,
        blockchain: B,
        store: S,
        oracles: HashMap<XOnlyPublicKey, O>,
        time: T,
        logger: L,
        close_approver: Option<Arc<dyn CooperativeCloseApprover>>,
    ) -> Result<Self, Error> {
        let signer_provider = Arc::new(CachedContractSignerProvider::new(signer_provider));

        log_info!(logger, "Manager initialized");

        Ok(Manager {
            secp: secp256k1_zkp::Secp256k1::new(),
            wallet,
            signer_provider,
            blockchain,
            store,
            oracles,
            time,
            logger,
            close_approver,
        })
    }

    /// Get the store from the Manager to access contracts.
    pub fn get_store(&self) -> &S {
        &self.store
    }

    /// Function called to pass a DlcMessage to the Manager.
    #[tracing::instrument(skip_all)]
    pub async fn on_dlc_message(
        &self,
        msg: &DlcMessage,
        counter_party: PublicKey,
    ) -> Result<Option<DlcMessage>, Error> {
        match msg {
            DlcMessage::Offer(o) => {
                log_debug!(self.logger, "Received offer message");
                self.on_offer_message(o, counter_party).await?;
                Ok(None)
            }
            DlcMessage::Accept(a) => {
                log_debug!(self.logger, "Received accept message");
                Ok(Some(self.on_accept_message(a, &counter_party).await?))
            }
            DlcMessage::Sign(s) => {
                log_debug!(self.logger, "Received sign message");
                self.on_sign_message(s, &counter_party).await?;
                Ok(None)
            }
            DlcMessage::Close(c) => {
                log_debug!(self.logger, "Received close message");
                self.on_close_message(c, &counter_party).await?;
                Ok(None)
            }
        }
    }

    /// Create a new spliced offer
    #[tracing::instrument(skip_all)]
    pub async fn send_splice_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        contract_id: &ContractId,
    ) -> Result<OfferDlc, Error> {
        let oracle_announcements = self.oracle_announcements(contract_input).await?;

        self.send_splice_offer_with_announcements(
            contract_input,
            counter_party,
            contract_id,
            oracle_announcements,
        )
        .await
    }

    /// Creates a new offer DLC using an existing DLC as an input.
    /// The new DLC MUST use a Confirmed contract as an input.
    #[tracing::instrument(skip_all)]
    pub async fn send_splice_offer_with_announcements(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        contract_id: &ContractId,
        oracle_announcements: Vec<Vec<OracleAnnouncement>>,
    ) -> Result<OfferDlc, Error> {
        let confirmed_contract =
            get_contract_in_state!(self, contract_id, Confirmed, Some(counter_party))?;

        let dlc_input = confirmed_contract.get_dlc_input();

        let (offered_contract, offer_msg) = crate::contract_updater::offer_contract(
            &self.secp,
            contract_input,
            oracle_announcements,
            vec![dlc_input],
            REFUND_DELAY,
            &counter_party,
            &self.wallet,
            &self.blockchain,
            &self.signer_provider,
            &self.logger,
        )
        .await?;

        offered_contract.validate()?;

        self.store.create_contract(&offered_contract).await?;

        Ok(offer_msg)
    }

    /// Function called to create a new DLC. The offered contract will be stored
    /// and an OfferDlc message returned. The application may append TLV
    /// records to the returned message and pass it to
    /// [`Manager::commit_offer`] before sending it.
    ///
    /// This function will fetch the oracle announcements from the oracle.
    #[tracing::instrument(skip_all)]
    pub async fn send_offer(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
    ) -> Result<OfferDlc, Error> {
        // If the oracle announcement fails to retrieve, then log and continue.
        let oracle_announcements = self.oracle_announcements(contract_input).await?;

        self.send_offer_with_announcements(contract_input, counter_party, oracle_announcements)
            .await
    }

    /// Function called to create a new DLC. The offered contract will be stored
    /// and an OfferDlc message returned.
    ///
    /// This function allows to pass the oracle announcements directly instead of
    /// fetching them from the oracle.
    #[tracing::instrument(skip_all)]
    pub async fn send_offer_with_announcements(
        &self,
        contract_input: &ContractInput,
        counter_party: PublicKey,
        oracle_announcements: Vec<Vec<OracleAnnouncement>>,
    ) -> Result<OfferDlc, Error> {
        let (offered_contract, offer_msg) = crate::contract_updater::offer_contract(
            &self.secp,
            contract_input,
            oracle_announcements,
            vec![],
            REFUND_DELAY,
            &counter_party,
            &self.wallet,
            &self.blockchain,
            &self.signer_provider,
            &self.logger,
        )
        .await?;

        offered_contract.validate()?;

        self.store.create_contract(&offered_contract).await?;

        Ok(offer_msg)
    }

    /// Function to call to accept a DLC for which an offer was received. The
    /// application may append TLV records to the returned message and pass it
    /// to [`Manager::commit_accept`] before sending it.
    #[tracing::instrument(skip_all)]
    pub async fn accept_contract_offer(
        &self,
        contract_id: &ContractId,
    ) -> Result<(ContractId, PublicKey, AcceptDlc), Error> {
        let offered_contract =
            get_contract_in_state!(self, contract_id, Offered, None as Option<PublicKey>)?;

        // The offer was validated on receipt, but the oracle event may have
        // matured while the offer waited for a decision. The CET locktime is
        // pinned to the closest event maturity.
        let now = self.time.unix_time_now();
        if u64::from(offered_contract.cet_locktime) <= now {
            return Err(Error::InvalidParameters(format!(
                "oracle event has already matured (maturity {}, time {now})",
                offered_contract.cet_locktime
            )));
        }

        let counter_party = offered_contract.counter_party;

        let (accepted_contract, accept_msg) = accept_contract(
            &self.secp,
            &offered_contract,
            &self.wallet,
            &self.signer_provider,
            &self.blockchain,
            &self.logger,
        )
        .await?;

        self.wallet.import_address(&Address::p2wsh(
            &accepted_contract.dlc_transactions.funding_witness_script,
            self.blockchain.get_network()?,
        ))?;

        let contract_id = accepted_contract.get_contract_id();

        self.store
            .update_contract(&Contract::Accepted(accepted_contract))
            .await?;

        log_info!(
            self.logger,
            "Accepted and stored the contract. temp_id={} contract_id={}",
            offered_contract.id.to_lower_hex_string(),
            contract_id.to_lower_hex_string(),
        );

        Ok((contract_id, counter_party, accept_msg))
    }

    /// Function to call with the final offer message before sending it. The
    /// application may have appended TLV records to the message returned by
    /// [`Manager::send_offer`]; this copies the message's stream onto the
    /// stored contract so the store matches what goes out on the wire.
    #[tracing::instrument(skip_all)]
    pub async fn commit_offer(&self, offer_msg: &OfferDlc) -> Result<(), Error> {
        let mut offered_contract = get_contract_in_state!(
            self,
            &offer_msg.temporary_contract_id,
            Offered,
            None as Option<PublicKey>
        )?;
        offered_contract.tlvs = offer_msg.tlvs.clone();
        self.store
            .update_contract(&Contract::Offered(offered_contract))
            .await
    }

    /// Function to call with the final accept message before sending it. See
    /// [`Manager::commit_offer`].
    #[tracing::instrument(skip_all)]
    pub async fn commit_accept(
        &self,
        contract_id: &ContractId,
        accept_msg: &AcceptDlc,
    ) -> Result<(), Error> {
        let mut accepted_contract =
            get_contract_in_state!(self, contract_id, Accepted, None as Option<PublicKey>)?;
        if accepted_contract.offered_contract.id != accept_msg.temporary_contract_id {
            return Err(Error::InvalidParameters(
                "accept message does not match the contract".to_string(),
            ));
        }
        accepted_contract.tlvs = accept_msg.tlvs.clone();
        self.store
            .update_contract(&Contract::Accepted(accepted_contract))
            .await
    }

    /// Function to call with the final sign message before sending it. See
    /// [`Manager::commit_offer`].
    #[tracing::instrument(skip_all)]
    pub async fn commit_sign(&self, sign_msg: &SignDlc) -> Result<(), Error> {
        let mut signed_contract = get_contract_in_state!(
            self,
            &sign_msg.contract_id,
            Signed,
            None as Option<PublicKey>
        )?;
        signed_contract.tlvs = sign_msg.tlvs.clone();
        self.store
            .update_contract(&Contract::Signed(signed_contract))
            .await
    }

    /// Function to call to check the state of the currently executing DLCs and
    /// update them if possible.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn periodic_check(&self) -> Result<(), Error> {
        let signed_contracts = self.check_for_spliced_contract().await?;
        self.check_signed_contracts(&signed_contracts).await?;
        self.check_confirmed_contracts().await?;
        self.check_preclosed_contracts().await?;

        Ok(())
    }

    /// Function to call to offer a DLC.
    #[tracing::instrument(skip_all)]
    pub async fn on_offer_message(
        &self,
        offered_message: &OfferDlc,
        counter_party: PublicKey,
    ) -> Result<(), Error> {
        offered_message.validate(
            &self.secp,
            REFUND_DELAY,
            REFUND_DELAY * 2,
            self.time.unix_time_now(),
        )?;
        let keys_id = self
            .signer_provider
            .derive_signer_key_id(offered_message.temporary_contract_id);
        let contract: OfferedContract =
            OfferedContract::try_from_offer_dlc(offered_message, counter_party, keys_id)?;
        contract.validate()?;

        if self.store.get_contract(&contract.id).await?.is_some() {
            return Err(Error::InvalidParameters(
                "Contract with identical id already exists".to_string(),
            ));
        }

        self.store.create_contract(&contract).await?;
        log_info!(
            self.logger,
            "Created and stored the offered contract. temp_id={}",
            contract.id.to_lower_hex_string(),
        );
        Ok(())
    }

    /// Function to call to close a DLC.
    #[tracing::instrument(skip_all)]
    pub async fn on_close_message(
        &self,
        close_msg: &CloseDlc,
        counter_party: &PublicKey,
    ) -> Result<(), Error> {
        // Validate that the contract exists and is in the correct state
        let signed_contract = get_contract_in_state!(
            self,
            &close_msg.contract_id,
            Confirmed,
            Some(*counter_party)
        )?;

        // Validate the close message by attempting to construct the close transaction
        // This verifies the signature and transaction structure without broadcasting
        let _close_tx = crate::contract_updater::complete_cooperative_close(
            &self.secp,
            &signed_contract,
            close_msg,
            &self.signer_provider,
            &self.logger,
        )?;

        // Message is valid - the application layer should call accept_cooperative_close()
        // if they want to accept the offered terms
        Ok(())
    }

    /// Function to call when an accept message is received. Verifies the
    /// accept, stores the signed contract, and returns the sign message for
    /// the application to finish. The application may append TLV records to
    /// it and pass it to [`Manager::commit_sign`] before sending it.
    #[tracing::instrument(skip_all)]
    pub async fn on_accept_message(
        &self,
        accept_msg: &AcceptDlc,
        counter_party: &PublicKey,
    ) -> Result<DlcMessage, Error> {
        let offered_contract = get_contract_in_state!(
            self,
            &accept_msg.temporary_contract_id,
            Offered,
            Some(*counter_party)
        )?;

        let (signed_contract, signed_msg) = match verify_accepted_and_sign_contract(
            &self.secp,
            &offered_contract,
            accept_msg,
            &self.wallet,
            &self.signer_provider,
            &self.store,
            &self.logger,
        )
        .await
        {
            Ok(contract) => contract,
            Err(e) => {
                log_error!(
                    self.logger,
                    "Error in on_accept_message. tmp_contract_id={} error={}",
                    offered_contract.id.to_lower_hex_string(),
                    e.to_string()
                );
                return self
                    .accept_fail_on_error(offered_contract, accept_msg.clone(), e)
                    .await;
            }
        };

        log_info!(
            self.logger,
            "Verified the accept message and signed the contract. temp_id={} contract_id={}",
            offered_contract.id.to_lower_hex_string(),
            signed_contract.accepted_contract.get_contract_id_string(),
        );

        let contract_id = signed_contract.accepted_contract.get_contract_id_string();

        self.wallet.import_address(&Address::p2wsh(
            &signed_contract
                .accepted_contract
                .dlc_transactions
                .funding_witness_script,
            self.blockchain.get_network()?,
        ))?;

        self.store
            .update_contract(&Contract::Signed(signed_contract))
            .await?;

        log_info!(
            self.logger,
            "Accepted and signed the contract. temp_id={} contract_id={}",
            offered_contract.id.to_lower_hex_string(),
            contract_id,
        );

        Ok(DlcMessage::Sign(signed_msg))
    }

    /// Function to call to sign a DLC for which an accept was received.
    #[tracing::instrument(skip_all)]
    pub async fn on_sign_message(
        &self,
        sign_message: &SignDlc,
        peer_id: &PublicKey,
    ) -> Result<(), Error> {
        let accepted_contract =
            get_contract_in_state!(self, &sign_message.contract_id, Accepted, Some(*peer_id))?;
        let (signed_contract, fund_tx) = match crate::contract_updater::verify_signed_contract(
            &self.secp,
            &accepted_contract,
            sign_message,
            &self.wallet,
            &self.store,
            &self.signer_provider,
            &self.logger,
        )
        .await
        {
            Ok(contract) => contract,
            Err(e) => {
                log_error!(
                    self.logger,
                    "Error in on_sign_message. contract_id={} error={}",
                    accepted_contract.get_contract_id_string(),
                    e.to_string()
                );
                return self
                    .sign_fail_on_error(accepted_contract, sign_message.clone(), e)
                    .await;
            }
        };

        self.store
            .update_contract(&Contract::Signed(signed_contract.clone()))
            .await?;

        self.blockchain.send_transaction(&fund_tx).await?;

        // Check if there are any DLC inputs in the funding inputs of the contract.
        // If there are, mark the contract as pre-closed.
        // The funding transaction was just broadcast, so every previous
        // contract it splices is now closing.
        self.preclose_spliced_contracts(&signed_contract).await?;

        Ok(())
    }

    /// Marks every Confirmed contract that `signed_contract` spends through a
    /// DLC input as PreClosed by the splice funding transaction.
    ///
    /// A splice can spend several previous contracts at once; each of them
    /// must move on, or its watcher keeps trying to settle or refund a spent
    /// output. A previous contract that is not Confirmed (already pre-closed
    /// by an earlier pass, for example) is skipped; other errors propagate.
    async fn preclose_spliced_contracts(
        &self,
        signed_contract: &SignedContract,
    ) -> Result<(), Error> {
        let splice_fund_tx = &signed_contract.accepted_contract.dlc_transactions.fund;
        for funding_input in &signed_contract
            .accepted_contract
            .offered_contract
            .funding_inputs
        {
            let Some(dlc_input) = &funding_input.dlc_input else {
                continue;
            };
            let contract_id = dlc_input.contract_id;

            let previous_contract = match get_contract_in_state!(
                self,
                &contract_id,
                Confirmed,
                None as Option<PublicKey>
            ) {
                Ok(contract) => contract,
                Err(Error::InvalidState(e)) => {
                    log_trace!(self.logger,
                        "The previous contract referenced in a splice transaction is in an unexpected state. contract_id={} error={}",
                        contract_id.to_lower_hex_string(), e.to_string(),
                    );
                    continue;
                }
                Err(e) => {
                    log_debug!(self.logger,
                        "The previous contract referenced in a splice transaction failed to retrieve. contract_id={} error={}",
                        contract_id.to_lower_hex_string(), e.to_string(),
                    );
                    return Err(e);
                }
            };

            let preclosed_contract = PreClosedContract {
                signed_contract: previous_contract.clone(),
                attestations: None,
                signed_cet: splice_fund_tx.clone(),
            };

            log_debug!(self.logger,
                "Contract contains a DLC input. Marking the previous contract as pre-closed. dlc_input_contract_id={} splice_contract_id={}",
                contract_id.to_lower_hex_string(),
                signed_contract.accepted_contract.get_contract_id_string(),
            );

            self.store
                .update_contract(&Contract::PreClosed(preclosed_contract))
                .await?;
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn get_oracle_announcements(
        &self,
        oracle_inputs: &OracleInput,
    ) -> Result<Vec<OracleAnnouncement>, Error> {
        let mut announcements = Vec::new();
        for pubkey in &oracle_inputs.public_keys {
            let oracle = self
                .oracles
                .get(pubkey)
                .ok_or_else(|| Error::InvalidParameters("Unknown oracle public key".to_string()))?;
            let announcement = oracle.get_announcement(&oracle_inputs.event_id).await?;
            announcements.push(announcement);
        }

        Ok(announcements)
    }

    async fn sign_fail_on_error<R>(
        &self,
        accepted_contract: AcceptedContract,
        sign_message: SignDlc,
        e: Error,
    ) -> Result<R, Error> {
        log_error!(
            self.logger,
            "Error in on_sign_message marking contract as failed sign. contract_id={} error={}",
            accepted_contract.get_contract_id_string(),
            e.to_string()
        );
        self.store
            .update_contract(&Contract::FailedSign(FailedSignContract {
                accepted_contract,
                sign_message,
                error_message: e.to_string(),
            }))
            .await?;
        Err(e)
    }

    async fn accept_fail_on_error<R>(
        &self,
        offered_contract: OfferedContract,
        accept_message: AcceptDlc,
        e: Error,
    ) -> Result<R, Error> {
        log_error!(
            self.logger,
            "Error in on_accept_message marking contract as failed accept. contract_id={} error={}",
            offered_contract.id.to_lower_hex_string(),
            e.to_string()
        );
        self.store
            .update_contract(&Contract::FailedAccept(FailedAcceptContract {
                offered_contract,
                accept_message,
                error_message: e.to_string(),
            }))
            .await?;
        Err(e)
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_signed_contract(&self, contract: &SignedContract) -> Result<(), Error> {
        let fund_txid = contract
            .accepted_contract
            .dlc_transactions
            .fund
            .compute_txid();
        let status = self
            .blockchain
            .get_transaction_confirmations(&fund_txid)
            .await?;
        if status == ConfirmationStatus::NotFound {
            log_warn!(self.logger,
                "Funding transaction not found in mempool or on-chain. Not confirming contract. fund_txid={} contract_id={}",
                fund_txid.to_string(),
                contract.accepted_contract.get_contract_id_string(),
            );
            return Ok(());
        }
        let confirmations = status.confirmations();
        if confirmations >= *NB_CONFIRMATIONS {
            log_info!(
                self.logger,
                "Marking signed contract as confirmed. confirmations={} contract_id={}",
                confirmations,
                contract.accepted_contract.get_contract_id_string(),
            );
            self.store
                .update_contract(&Contract::Confirmed(contract.clone()))
                .await?;
        } else {
            log_debug!(self.logger,
                "Not enough confirmations to mark contract as confirmed. confirmations={} required={} contract_id={}", 
                confirmations,
                *NB_CONFIRMATIONS,
                contract.accepted_contract.get_contract_id_string(),
            );
        }
        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_signed_contracts(
        &self,
        signed_contracts: &[SignedContract],
    ) -> Result<(), Error> {
        for c in signed_contracts {
            if let Err(e) = self.check_signed_contract(c).await {
                log_error!(
                    self.logger,
                    "Error checking signed contract. contract_id={} error={}",
                    c.accepted_contract.get_contract_id_string(),
                    e.to_string()
                )
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_for_spliced_contract(&self) -> Result<Vec<SignedContract>, Error> {
        let contracts = self.get_store().get_signed_contracts().await?;
        for contract in &contracts {
            if !contract
                .accepted_contract
                .offered_contract
                .funding_inputs
                .iter()
                .any(|d| d.dlc_input.is_some())
            {
                continue;
            }

            // The funding transaction of the splice contract closes the
            // previous contract. Only pre-close the previous contract when
            // the network knows that transaction. A transaction that was
            // evicted from the mempool, or that was not broadcast, must not
            // close the previous contract.
            let splice_fund_txid = contract
                .accepted_contract
                .dlc_transactions
                .fund
                .compute_txid();
            if self
                .blockchain
                .get_transaction_confirmations(&splice_fund_txid)
                .await?
                == ConfirmationStatus::NotFound
            {
                log_debug!(self.logger,
                    "Splice funding transaction not found in mempool or on-chain. Not pre-closing the previous contracts. splice_fund_txid={} splice_contract_id={}",
                    splice_fund_txid.to_string(),
                    contract.accepted_contract.get_contract_id_string(),
                );
                continue;
            }

            self.preclose_spliced_contracts(contract).await?;
        }
        Ok(contracts)
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_confirmed_contracts(&self) -> Result<(), Error> {
        for c in self.store.get_confirmed_contracts().await? {
            // Confirmed contracts from channel are processed in channel specific methods.
            if c.channel_id.is_some() {
                continue;
            }
            if let Err(e) = self.check_confirmed_contract(&c).await {
                log_error!(
                    self.logger,
                    "Error checking confirmed contract. contract_id={} error={}",
                    c.accepted_contract.get_contract_id_string(),
                    e.to_string()
                )
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn get_closable_contract_info<'a>(
        &'a self,
        contract: &'a SignedContract,
    ) -> ClosableContractInfo<'a> {
        let contract_infos = &contract.accepted_contract.offered_contract.contract_info;
        let adaptor_infos = &contract.accepted_contract.adaptor_infos;
        for (contract_info, adaptor_info) in contract_infos.iter().zip(adaptor_infos.iter()) {
            log_debug!(
                self.logger,
                "Checking contract for oracle maturation. contract_id={}",
                contract.accepted_contract.get_contract_id_string()
            );
            let matured: Vec<_> = contract_info
                .oracle_announcements
                .iter()
                .filter(|x| {
                    (x.oracle_event.event_maturity_epoch as u64) + MATURITY_SKEW_SECS
                        <= self.time.unix_time_now()
                })
                .enumerate()
                .collect();
            if matured.len() >= contract_info.threshold {
                let attestations = stream::iter(matured.iter())
                    .map(|(i, announcement)| async move {
                        log_debug!(self.logger,
                            "Oracle announcement for contract is matured. Getting attestations. contract_id={} event_id={}", 
                            contract.accepted_contract.get_contract_id_string(),
                            announcement.oracle_event.event_id
                        );
                        // First try to get the oracle
                        let oracle = match self.oracles.get(&announcement.oracle_public_key) {
                            Some(oracle) => oracle,
                            None => {
                                log_debug!(self.logger,
                                    "Oracle not found. pubkey={}. contract_id={} event_id={}", 
                                    announcement.oracle_public_key,
                                    contract.accepted_contract.get_contract_id_string(),
                                    announcement.oracle_event.event_id
                                );
                                return None;
                            }
                        };
                        // Then try to get the attestation
                        let attestation = match oracle
                            .get_attestation(&announcement.oracle_event.event_id)
                            .await
                        {
                            Ok(attestation) => attestation,
                            Err(_) => {
                                // log_error!(self.logger,
                                //     "Attestation not found for event. pubkey={} event_id={} error={}",
                                //     announcement.oracle_public_key,
                                //     announcement.oracle_event.event_id,
                                //     e.to_string()
                                // );
                                return None;
                            }
                        };
                        // Validate the attestation
                        if let Err(e) = attestation.validate(&self.secp, announcement) {
                            log_error!(self.logger,
                                "Oracle attestation is not valid. pubkey={} event_id={}, error={}",
                                announcement.oracle_public_key,
                                announcement.oracle_event.event_id,
                                e.to_string()
                            );
                            return None;
                        }
                        log_info!(self.logger,
                            "Retrieved a valid attestation. pubkey={} event_id={} outcomes={:?}", 
                            announcement.oracle_public_key,
                            announcement.oracle_event.event_id,
                            attestation.outcomes
                        );
                        Some((*i, attestation))
                    })
                    .collect::<FuturesUnordered<_>>()
                    .await
                    .filter_map(|result| async move { result }) // Filter out None values
                    .collect::<Vec<_>>()
                    .await;
                if attestations.len() >= contract_info.threshold {
                    log_info!(self.logger,
                        "Found enough attestations to close contract. contract_id={} attestations={}", 
                        contract.accepted_contract.get_contract_id_string(),
                        attestations.len()
                    );
                    return Some((contract_info, adaptor_info, attestations));
                }
            }
        }
        None
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_confirmed_contract(&self, contract: &SignedContract) -> Result<(), Error> {
        // log_debug!(
        //     self.logger,
        //     "Checking confirmed contract. contract_id={}",
        //     contract.accepted_contract.get_contract_id_string()
        // );
        let closable_contract_info = self.get_closable_contract_info(contract).await;
        if let Some((contract_info, adaptor_info, attestations)) = closable_contract_info {
            log_debug!(
                self.logger,
                "Found closable contract info. contract_id={} attestations={}",
                contract.accepted_contract.get_contract_id_string(),
                attestations.len()
            );
            let offer = &contract.accepted_contract.offered_contract;
            let signer = self.signer_provider.derive_contract_signer(offer.keys_id)?;

            //  === WARNING ===
            // This code could potentially be problematic. When running refund tests, it would look for a CET
            // but the CET would be invalid and refund would not pass. By only updating with a valid CET,
            // we then go to update. This way if it fails we can check for refund instead of bailing and getting locked
            // funds.
            if let Ok(cet) = crate::contract_updater::get_signed_cet(
                &self.secp,
                contract,
                contract_info,
                adaptor_info,
                &attestations,
                &signer,
                &self.logger,
            ) {
                log_info!(
                    self.logger,
                    "Found valid CET. Closing contract. contract_id={}",
                    contract.accepted_contract.get_contract_id_string()
                );
                match self
                    .close_contract(
                        contract,
                        cet,
                        attestations.iter().map(|x| x.1.clone()).collect(),
                    )
                    .await
                {
                    Ok(closed_contract) => {
                        log_info!(
                            self.logger,
                            "Updated contract to closed. contract_id={}",
                            contract.accepted_contract.get_contract_id_string()
                        );
                        self.store.update_contract(&closed_contract).await?;
                        return Ok(());
                    }
                    Err(e) => {
                        log_warn!(
                            self.logger,
                            "Failed to close contract. contract_id={} error={}",
                            contract.accepted_contract.get_contract_id_string(),
                            e.to_string()
                        );
                        return Err(e);
                    }
                }
            }
        }

        // Check each pending close transaction
        for pending_close_tx in &contract
            .accepted_contract
            .dlc_transactions
            .pending_close_txs
        {
            let status = self
                .blockchain
                .get_transaction_confirmations(&pending_close_tx.compute_txid())
                .await?;
            if status == ConfirmationStatus::NotFound {
                // The pending close transaction is not in the mempool and not
                // in a block. It was not broadcast, or it was evicted from
                // the mempool. Do not close the contract with it.
                continue;
            }
            let confirmations = status.confirmations();

            log_debug!(
                self.logger,
                "Checking pending close transaction. close_txid={} contract_id={} confirmations={}",
                pending_close_tx.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string(),
                confirmations
            );

            // `Closed` is a terminal state that no periodic check examines
            // again. A transaction that only sits in the mempool can still be
            // evicted, so require at least one confirmation on-chain, also
            // when `NB_CONFIRMATIONS` is zero.
            if confirmations >= (*NB_CONFIRMATIONS).max(1) {
                // Found a fully confirmed pending close - move directly to Closed
                log_info!(self.logger,
                    "Pending close transaction is fully confirmed. Moving to closed. close_txid={} contract_id={}", 
                    pending_close_tx.compute_txid().to_string(),
                    contract.accepted_contract.get_contract_id_string()
                );
                let pnl = contract.accepted_contract.compute_pnl(pending_close_tx);
                let closed_contract = ClosedContract {
                    attestations: None, // Cooperative close has no attestations
                    signed_cet: Some(pending_close_tx.clone()), // Cooperative close doesn't use a CET
                    contract_id: contract.accepted_contract.get_contract_id(),
                    temporary_contract_id: contract.accepted_contract.offered_contract.id,
                    counter_party_id: contract.accepted_contract.offered_contract.counter_party,
                    pnl,
                    funding_txid: contract
                        .accepted_contract
                        .dlc_transactions
                        .fund
                        .compute_txid(),
                    signed_contract: contract.clone(),
                };

                self.store
                    .update_contract(&Contract::Closed(closed_contract))
                    .await?;
                break; // Only one close can be confirmed
            } else if confirmations >= 1 {
                // Found a confirmed but not fully confirmed pending close - move to PreClosed
                log_debug!(self.logger,
                    "Found a confirmed but not fully confirmed pending close. Moving to preclosed. close_txid={} contract_id={}", 
                    pending_close_tx.compute_txid().to_string(),
                    contract.accepted_contract.get_contract_id_string()
                );
                let preclosed_contract = PreClosedContract {
                    signed_contract: contract.clone(),
                    attestations: None, // Cooperative close has no attestations
                    signed_cet: pending_close_tx.clone(),
                };

                self.store
                    .update_contract(&Contract::PreClosed(preclosed_contract))
                    .await?;
                break; // Only one close can be confirmed
            }
        }

        self.check_refund(contract).await?;

        Ok(())
    }

    /// Manually close a contract with the oracle attestations.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn close_confirmed_contract(
        &self,
        contract_id: &ContractId,
        attestations: Vec<(usize, OracleAttestation)>,
    ) -> Result<Contract, Error> {
        log_info!(
            self.logger,
            "Attempting to close confirmed contract manually. contract_id={} outcomes={:?}",
            contract_id.to_lower_hex_string(),
            attestations
                .iter()
                .map(|(_, a)| a.outcomes.clone())
                .collect::<Vec<_>>()
        );
        let contract = get_contract_in_state!(self, contract_id, Confirmed, None::<PublicKey>)?;
        let contract_infos = &contract.accepted_contract.offered_contract.contract_info;
        let adaptor_infos = &contract.accepted_contract.adaptor_infos;

        // Find the contract info whose oracles produced every attestation.
        // Each attestation must be a valid signature from the oracle at the
        // index it claims, so a forged, misindexed, or out-of-range attestation
        // disqualifies the whole set instead of being skipped.
        if let Some((contract_info, adaptor_info)) =
            contract_infos.iter().zip(adaptor_infos).find(|(c, _)| {
                attestations.len() >= c.threshold
                    && attestations.iter().all(|(i, a)| {
                        c.oracle_announcements.get(*i).is_some_and(|announcement| {
                            a.validate(&self.secp, announcement).is_ok()
                        })
                    })
            })
        {
            let offer = &contract.accepted_contract.offered_contract;
            let signer = self.signer_provider.derive_contract_signer(offer.keys_id)?;
            log_debug!(
                self.logger,
                "Getting signed CET. contract_id={}",
                contract.accepted_contract.get_contract_id_string()
            );
            let cet = crate::contract_updater::get_signed_cet(
                &self.secp,
                &contract,
                contract_info,
                adaptor_info,
                &attestations,
                &signer,
                &self.logger,
            )?;

            // Check that the lock time has passed
            let time = bitcoin::absolute::Time::from_consensus(self.time.unix_time_now() as u32)
                .expect("Time is not in valid range. This should never happen.");
            let height =
                Height::from_consensus(self.blockchain.get_blockchain_height().await? as u32)
                    .expect("Height is not in valid range. This should never happen.");
            let locktime = cet.lock_time;

            if !locktime.is_satisfied_by(height, time) {
                return Err(Error::InvalidState(
                    "CET lock time has not passed yet".to_string(),
                ));
            }

            match self
                .close_contract(
                    &contract,
                    cet,
                    attestations.into_iter().map(|x| x.1).collect(),
                )
                .await
            {
                Ok(closed_contract) => {
                    log_info!(
                        self.logger,
                        "Closed contract manually. contract_id={}",
                        contract.accepted_contract.get_contract_id_string()
                    );
                    self.store.update_contract(&closed_contract).await?;
                    Ok(closed_contract)
                }
                Err(e) => {
                    log_error!(
                        self.logger,
                        "Failed to close contract. contract_id={} error={}",
                        contract.accepted_contract.get_contract_id_string(),
                        e.to_string()
                    );
                    Err(e)
                }
            }
        } else {
            Err(Error::InvalidState(
                "Attestations did not match contract infos".to_string(),
            ))
        }
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_preclosed_contracts(&self) -> Result<(), Error> {
        for c in self.store.get_preclosed_contracts().await? {
            if let Err(e) = self.check_preclosed_contract(&c).await {
                log_error!(
                    self.logger,
                    "Error checking pre-closed contract. contract_id={} error={}",
                    c.signed_contract.accepted_contract.get_contract_id_string(),
                    e
                )
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_preclosed_contract(&self, contract: &PreClosedContract) -> Result<(), Error> {
        let broadcasted_txid = contract.signed_cet.compute_txid();
        let status = self
            .blockchain
            .get_transaction_confirmations(&broadcasted_txid)
            .await?;
        if status == ConfirmationStatus::NotFound {
            // The closing transaction is not in the mempool and not in a
            // block. It was evicted from the mempool, or the chain source has
            // not seen it yet. Broadcast it again — the network accepts a
            // transaction it already knows without an error.
            match self.blockchain.send_transaction(&contract.signed_cet).await {
                Ok(()) => {
                    log_warn!(self.logger,
                        "Closing transaction not found in mempool or on-chain. Broadcast it again. close_txid={} contract_id={}",
                        broadcasted_txid.to_string(),
                        contract.signed_contract.accepted_contract.get_contract_id_string(),
                    );
                }
                Err(e) if contract.attestations.is_none() => {
                    // A pre-close without attestations comes from a splice or
                    // a cooperative close. Its closing transaction cannot
                    // enter the mempool again, so the funding output stays
                    // unspent and the contract is still live. Move it back to
                    // confirmed.
                    log_warn!(self.logger,
                        "Closing transaction was dropped and cannot be broadcast again. Moving contract back to confirmed. close_txid={} contract_id={} error={}",
                        broadcasted_txid.to_string(),
                        contract.signed_contract.accepted_contract.get_contract_id_string(),
                        e.to_string(),
                    );
                    self.store
                        .update_contract(&Contract::Confirmed(contract.signed_contract.clone()))
                        .await?;
                }
                Err(e) => return Err(e),
            }
            return Ok(());
        }
        let confirmations = status.confirmations();
        log_debug!(
            self.logger,
            "Checking pre-closed contract. broadcasted_txid={} contract_id={} confirmations={}",
            broadcasted_txid.to_string(),
            contract
                .signed_contract
                .accepted_contract
                .get_contract_id_string(),
            confirmations
        );
        // `Closed` is a terminal state that no periodic check examines again.
        // A transaction that only sits in the mempool can still be evicted,
        // so require at least one confirmation on-chain, also when
        // `NB_CONFIRMATIONS` is zero.
        if confirmations >= (*NB_CONFIRMATIONS).max(1) {
            log_debug!(self.logger,
                "Pre-closed contract is fully confirmed. Moving to closed. broadcasted_txid={} contract_id={}",
                broadcasted_txid.to_string(),
                contract.signed_contract.accepted_contract.get_contract_id_string()
            );
            let pnl = contract
                .signed_contract
                .accepted_contract
                .compute_pnl(&contract.signed_cet);

            let closed_contract = ClosedContract {
                attestations: contract.attestations.clone(),
                signed_cet: Some(contract.signed_cet.clone()),
                contract_id: contract.signed_contract.accepted_contract.get_contract_id(),
                temporary_contract_id: contract
                    .signed_contract
                    .accepted_contract
                    .offered_contract
                    .id,
                counter_party_id: contract
                    .signed_contract
                    .accepted_contract
                    .offered_contract
                    .counter_party,
                funding_txid: contract
                    .signed_contract
                    .accepted_contract
                    .dlc_transactions
                    .fund
                    .compute_txid(),
                pnl,
                signed_contract: contract.signed_contract.clone(),
            };
            self.store
                .update_contract(&Contract::Closed(closed_contract))
                .await?;
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn close_contract(
        &self,
        contract: &SignedContract,
        signed_cet: Transaction,
        attestations: Vec<OracleAttestation>,
    ) -> Result<Contract, Error> {
        // A CET that was not broadcast yet is not found on the network, so
        // count it as zero confirmations.
        let confirmations = self
            .blockchain
            .get_transaction_confirmations(&signed_cet.compute_txid())
            .await?
            .confirmations();

        if confirmations < 1 {
            log_info!(
                self.logger,
                "Broadcasting signed CET. txid={} contract_id={}",
                signed_cet.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            // TODO(tibo): if this fails because another tx is already in
            // mempool or blockchain, we might have been cheated. There is
            // not much to be done apart from possibly extracting a fraud
            // proof but ideally it should be handled.
            self.blockchain.send_transaction(&signed_cet).await?;

            let preclosed_contract = PreClosedContract {
                signed_contract: contract.clone(),
                attestations: Some(attestations),
                signed_cet,
            };

            return Ok(Contract::PreClosed(preclosed_contract));
        } else if confirmations < *NB_CONFIRMATIONS {
            log_debug!(self.logger,
                "Found a confirmed but not fully confirmed pending close. Moving to preclosed. txid={} contract_id={}", 
                signed_cet.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            let preclosed_contract = PreClosedContract {
                signed_contract: contract.clone(),
                attestations: Some(attestations),
                signed_cet,
            };

            return Ok(Contract::PreClosed(preclosed_contract));
        }

        let closed_contract = ClosedContract {
            attestations: Some(attestations.to_vec()),
            pnl: contract.accepted_contract.compute_pnl(&signed_cet),
            signed_cet: Some(signed_cet),
            contract_id: contract.accepted_contract.get_contract_id(),
            temporary_contract_id: contract.accepted_contract.offered_contract.id,
            funding_txid: contract
                .accepted_contract
                .dlc_transactions
                .fund
                .compute_txid(),
            counter_party_id: contract.accepted_contract.offered_contract.counter_party,
            signed_contract: contract.clone(),
        };

        Ok(Contract::Closed(closed_contract))
    }

    /// Check if the refund locktime has passed and broadcast the refund transaction if it has.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn check_and_broadcast_refund(
        &self,
        contract_id: &ContractId,
    ) -> Result<Contract, Error> {
        // Assert the contract is confirmed
        let contract = get_contract_in_state!(self, contract_id, Confirmed, None::<PublicKey>)?;

        if contract
            .accepted_contract
            .dlc_transactions
            .refund
            .lock_time
            .to_consensus_u32() as u64
            <= self.time.unix_time_now()
        {
            log_debug!(self.logger,
                "Refund locktime has passed. Attempting to broadcast refund. refund_txid={} contract_id={}", 
                contract.accepted_contract.dlc_transactions.refund.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            let accepted_contract = &contract.accepted_contract;
            let refund = accepted_contract.dlc_transactions.refund.clone();
            // A refund that was not broadcast yet is not found on the
            // network, so count it as zero confirmations.
            let confirmations = self
                .blockchain
                .get_transaction_confirmations(&refund.compute_txid())
                .await?
                .confirmations();
            if confirmations == 0 {
                log_debug!(self.logger,
                    "Refund transaction has not been broadcast yet. Sending transaction txid={} contract_id={}", 
                    refund.compute_txid().to_string(),
                    contract.accepted_contract.get_contract_id_string()
                );
                let offer = &contract.accepted_contract.offered_contract;
                let signer = self.signer_provider.derive_contract_signer(offer.keys_id)?;
                let refund = crate::contract_updater::get_signed_refund(
                    &self.secp,
                    &contract,
                    &signer,
                    &self.logger,
                )?;
                self.blockchain.send_transaction(&refund).await?;
            }

            let refunded = Contract::Refunded(contract.clone());
            self.store.update_contract(&refunded).await?;
            Ok(refunded)
        } else {
            return Err(Error::InvalidParameters(
                "Contract maturity has not passed to broadcast refund".to_string(),
            ));
        }
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn check_refund(&self, contract: &SignedContract) -> Result<(), Error> {
        if contract
            .accepted_contract
            .dlc_transactions
            .refund
            .lock_time
            .to_consensus_u32() as u64
            <= self.time.unix_time_now()
        {
            let refund_txid = contract
                .accepted_contract
                .dlc_transactions
                .refund
                .compute_txid();
            let confirmations = self
                .blockchain
                .get_transaction_confirmations(&refund_txid)
                .await?
                .confirmations();

            if confirmations > 0 {
                // Counterparty (or we) already broadcast the refund tx. Update state.
                self.store
                    .update_contract(&Contract::Refunded(contract.clone()))
                    .await?;
            } else if *AUTOMATIC_REFUND {
                self.check_and_broadcast_refund(&contract.accepted_contract.get_contract_id())
                    .await?;
            }
        }

        Ok(())
    }

    /// Function to call when we detect that a contract was closed by our counter party.
    /// This will update the state of the contract and return the [`Contract`] object.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn on_counterparty_close(
        &mut self,
        contract: &SignedContract,
        closing_tx: Transaction,
        confirmations: u32,
    ) -> Result<Contract, Error> {
        // check if the closing tx actually spends the funding output
        if !closing_tx.input.iter().any(|i| {
            i.previous_output
                == contract
                    .accepted_contract
                    .dlc_transactions
                    .get_fund_outpoint()
        }) {
            log_error!(
                self.logger,
                "Closing tx does not spend the funding tx. txid={} contract_id={}",
                closing_tx.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            return Err(Error::InvalidParameters(
                "Closing tx does not spend the funding tx".to_string(),
            ));
        }

        // check if it is the refund tx (easy case)
        if contract
            .accepted_contract
            .dlc_transactions
            .refund
            .compute_txid()
            == closing_tx.compute_txid()
        {
            log_debug!(
                self.logger,
                "Closing tx is the refund tx. Moving to refunded. txid={} contract_id={}",
                closing_tx.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            let refunded = Contract::Refunded(contract.clone());
            self.store.update_contract(&refunded).await?;
            return Ok(refunded);
        }

        let contract = if confirmations < *NB_CONFIRMATIONS {
            log_info!(self.logger,
                "Closing transaction is not fully confirmed. Moving to preclosed. txid={} contract_id={}", 
                closing_tx.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            Contract::PreClosed(PreClosedContract {
                signed_contract: contract.clone(),
                attestations: None, // todo in some cases we can get the attestations from the closing tx
                signed_cet: closing_tx,
            })
        } else {
            log_info!(
                self.logger,
                "Closing transaction is fully confirmed. Moving to closed. txid={} contract_id={}",
                closing_tx.compute_txid().to_string(),
                contract.accepted_contract.get_contract_id_string()
            );
            Contract::Closed(ClosedContract {
                attestations: None, // todo in some cases we can get the attestations from the closing tx
                pnl: contract.accepted_contract.compute_pnl(&closing_tx),
                signed_cet: Some(closing_tx),
                contract_id: contract.accepted_contract.get_contract_id(),
                temporary_contract_id: contract.accepted_contract.offered_contract.id,
                counter_party_id: contract.accepted_contract.offered_contract.counter_party,
                funding_txid: contract
                    .accepted_contract
                    .dlc_transactions
                    .fund
                    .compute_txid(),
                signed_contract: contract.clone(),
            })
        };

        self.store.update_contract(&contract).await?;

        Ok(contract)
    }

    /// Initiates a cooperative close of a contract by creating and signing a closing transaction.
    /// Returns a CloseDlc message to be sent to the counter party.
    /// The contract remains in Confirmed state until the close transaction is broadcast.
    ///
    /// # Warning
    ///
    /// The returned [`CloseDlc`] carries a signature over a transaction that
    /// pays `counter_payout` to the counterparty out of the contract's funds.
    /// Nothing binds `counter_payout` to any oracle outcome: the caller is
    /// fully responsible for choosing a payout it is willing to give away. The
    /// counterparty can broadcast the close transaction at any time once it
    /// holds this message.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn cooperative_close_contract(
        &self,
        contract_id: &ContractId,
        counter_payout: Amount,
    ) -> Result<(CloseDlc, PublicKey), Error> {
        let signed_contract =
            get_contract_in_state!(self, contract_id, Confirmed, None as Option<PublicKey>)?;

        let (close_message, close_tx) = crate::contract_updater::create_cooperative_close(
            &self.secp,
            &signed_contract,
            counter_payout,
            &self.signer_provider,
            &self.logger,
        )?;

        // Create updated contract with pending close transaction
        let mut updated_dlc_transactions =
            signed_contract.accepted_contract.dlc_transactions.clone();
        updated_dlc_transactions
            .pending_close_txs
            .push(close_tx.clone());
        log_debug!(
            self.logger,
            "Created updated contract with pending close transaction. close_txid={} contract_id={}",
            close_tx.compute_txid().to_string(),
            contract_id.to_lower_hex_string()
        );

        let updated_accepted_contract = AcceptedContract {
            dlc_transactions: updated_dlc_transactions,
            ..signed_contract.accepted_contract.clone()
        };

        let updated_signed_contract = SignedContract {
            accepted_contract: updated_accepted_contract,
            ..signed_contract.clone()
        };

        // Update contract state to track pending close
        self.store
            .update_contract(&Contract::Confirmed(updated_signed_contract))
            .await?;

        let counter_party = signed_contract
            .accepted_contract
            .offered_contract
            .counter_party;

        Ok((close_message, counter_party))
    }

    /// Accepts a cooperative close request by completing the close transaction
    /// and broadcasting it to the network.
    ///
    /// # Warning
    ///
    /// A valid signature on the [`CloseDlc`] is not consent: the counterparty
    /// chooses `accept_payout` freely, up to the full collateral. The
    /// [`CooperativeCloseApprover`] set at [`Manager::new`] is consulted with
    /// the proposed payout before anything is broadcast; without one, or when
    /// it declines, this returns [`Error::CooperativeCloseRejected`] and the
    /// contract stays in its current state.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn accept_cooperative_close(
        &self,
        contract_id: &ContractId,
        close_message: &CloseDlc,
    ) -> Result<(), Error> {
        let signed_contract =
            get_contract_in_state!(self, contract_id, Confirmed, None as Option<PublicKey>)?;

        let approver = self.close_approver.as_ref().ok_or_else(|| {
            Error::CooperativeCloseRejected(
                "no cooperative close approver is configured".to_string(),
            )
        })?;
        if !approver.approve_counterparty_close(*contract_id, close_message.accept_payout)? {
            log_warn!(
                self.logger,
                "Cooperative close proposal declined by the application. contract_id={} accept_payout={}",
                contract_id.to_lower_hex_string(),
                close_message.accept_payout,
            );
            return Err(Error::CooperativeCloseRejected(format!(
                "close proposal declined by the application. accept_payout={}",
                close_message.accept_payout
            )));
        }

        let close_tx = crate::contract_updater::complete_cooperative_close(
            &self.secp,
            &signed_contract,
            close_message,
            &self.signer_provider,
            &self.logger,
        )?;

        log_debug!(
            self.logger,
            "Completed cooperative close. close_txid={} contract_id={}",
            close_tx.compute_txid().to_string(),
            contract_id.to_lower_hex_string()
        );
        // Broadcast the closing transaction
        self.blockchain.send_transaction(&close_tx).await?;

        // Create PreClosed contract (transaction broadcast but not confirmed yet)
        let preclosed_contract = PreClosedContract {
            signed_contract,
            attestations: None,
            signed_cet: close_tx,
        };

        self.store
            .update_contract(&Contract::PreClosed(preclosed_contract))
            .await?;

        Ok(())
    }
}

impl<W: Deref, SP: Deref, B: Deref, S: Deref, O: Deref, T: Deref, X: ContractSigner, L: Deref>
    Manager<W, Arc<CachedContractSignerProvider<SP, X>>, B, S, O, T, X, L>
where
    W::Target: Wallet,
    SP::Target: ContractSignerProvider<Signer = X>,
    B::Target: Blockchain,
    S::Target: Storage,
    O::Target: Oracle,
    T::Target: Time,
    L::Target: Logger,
{
    async fn oracle_announcements(
        &self,
        contract_input: &ContractInput,
    ) -> Result<Vec<Vec<OracleAnnouncement>>, Error> {
        let announcements = stream::iter(contract_input.contract_infos.iter())
            .map(|x| {
                let future = self.get_oracle_announcements(&x.oracles);
                async move {
                    match future.await {
                        Ok(result) => Ok(result),
                        Err(e) => {
                            log_error!(self.logger, "Failed to get oracle announcements: {}", e);
                            Err(e)
                        }
                    }
                }
            })
            .collect::<FuturesUnordered<_>>()
            .await
            .try_collect::<Vec<_>>()
            .await?;
        Ok(announcements)
    }
}
