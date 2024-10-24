pub mod convert;
pub mod ddkrpc;
pub mod util;

use std::str::FromStr;
use std::sync::Arc;

use ddk::bitcoin::secp256k1::PublicKey;
use ddk::bitcoin::{Address, Amount, FeeRate};
use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::dlc_manager::Oracle as DlcOracle;
use ddk::dlc_manager::Storage as DlcStorage;
use ddk::oracle::KormirOracleClient;
use ddk::storage::SledStorage;
use ddk::transport::lightning::LightningTransport;
use ddk::util::serialize_contract;
use ddk::DlcDevKit;
use ddk::{Oracle, Storage, Transport};
use ddkrpc::ddk_rpc_server::DdkRpc;
use ddkrpc::{
    AcceptOfferRequest, AcceptOfferResponse, ConnectRequest, ConnectResponse,
    GetWalletTransactionsRequest, GetWalletTransactionsResponse, ListContractsRequest,
    ListContractsResponse, ListOffersRequest, ListOffersResponse, ListOraclesRequest,
    ListOraclesResponse, ListPeersRequest, ListPeersResponse, ListUtxosRequest, ListUtxosResponse,
    NewAddressRequest, NewAddressResponse, OracleAnnouncementsRequest, OracleAnnouncementsResponse,
    Peer, SendOfferRequest, SendOfferResponse, SendRequest, SendResponse, WalletBalanceRequest,
    WalletBalanceResponse,
};
use ddkrpc::{InfoRequest, InfoResponse};
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::{async_trait, Code};

type DdkServer = DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

pub struct DdkNode {
    pub inner: Arc<DdkServer>,
}

impl DdkNode {
    pub fn new(ddk: DdkServer) -> Self {
        Self {
            inner: Arc::new(ddk),
        }
    }
}

#[async_trait]
impl DdkRpc for DdkNode {
    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn info(&self, _request: Request<InfoRequest>) -> Result<Response<InfoResponse>, Status> {
        tracing::info!("Request for node info.");
        let pubkey = self.inner.transport.node_id.to_string();
        let transport = self.inner.transport.name();
        let oracle = self.inner.oracle.name();
        let response = InfoResponse {
            pubkey,
            transport,
            oracle,
        };
        Ok(Response::new(response))
    }

    #[tracing::instrument(skip(self, request), name = "grpc_server")]
    async fn send_offer(
        &self,
        request: Request<SendOfferRequest>,
    ) -> Result<Response<SendOfferResponse>, Status> {
        tracing::info!("Request to send offer.");
        let SendOfferRequest {
            contract_input,
            counter_party,
        } = request.into_inner();
        let contract_input: ContractInput =
            serde_json::from_slice(&contract_input).expect("couldn't get bytes correct");
        let mut oracle_announcements = Vec::new();
        for info in &contract_input.contract_infos {
            let announcement = self
                .inner
                .oracle
                .get_announcement(&info.oracles.event_id)
                .await
                .unwrap();
            oracle_announcements.push(announcement)
        }

        let counter_party = PublicKey::from_str(&counter_party).expect("no public key");
        let offer_msg = self
            .inner
            .send_dlc_offer(&contract_input, counter_party, oracle_announcements)
            .map_err(|e| {
                Status::new(
                    Code::Cancelled,
                    format!(
                        "Contract offer could not be sent to counterparty. error={:?}",
                        e
                    ),
                )
            })?;

        let offer_dlc =
            serde_json::to_vec(&offer_msg).expect("OfferDlc could not be converted to vec.");
        Ok(Response::new(SendOfferResponse { offer_dlc }))
    }

    #[tracing::instrument(skip(self, request), name = "grpc_server")]
    async fn accept_offer(
        &self,
        request: Request<AcceptOfferRequest>,
    ) -> Result<Response<AcceptOfferResponse>, Status> {
        tracing::info!("Request to accept offer.");
        let mut contract_id = [0u8; 32];
        let contract_id_bytes = hex::decode(&request.into_inner().contract_id).unwrap();
        contract_id.copy_from_slice(&contract_id_bytes);
        let (contract_id, counter_party, accept_dlc) = self
            .inner
            .accept_dlc_offer(contract_id)
            .map_err(|_| Status::new(Code::Cancelled, "Contract could not be accepted."))?;

        let accept_dlc = serde_json::to_vec(&accept_dlc).map_err(|_| {
            Status::new(Code::Cancelled, "Accept DLC is malformed to create bytes.")
        })?;

        Ok(Response::new(AcceptOfferResponse {
            contract_id,
            counter_party,
            accept_dlc,
        }))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn new_address(
        &self,
        _request: Request<NewAddressRequest>,
    ) -> Result<Response<NewAddressResponse>, Status> {
        tracing::info!("Request for new wallet address");
        let address = self
            .inner
            .wallet
            .new_external_address()
            .unwrap()
            .to_string();
        let response = NewAddressResponse { address };
        Ok(Response::new(response))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn list_offers(
        &self,
        _request: Request<ListOffersRequest>,
    ) -> Result<Response<ListOffersResponse>, Status> {
        tracing::info!("Request for offers to the node.");
        let offers = self.inner.storage.get_contract_offers().unwrap();
        let offers: Vec<Vec<u8>> = offers
            .iter()
            .map(|offer| serde_json::to_vec(offer).unwrap())
            .collect();

        Ok(Response::new(ListOffersResponse { offers }))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn wallet_balance(
        &self,
        _request: Request<WalletBalanceRequest>,
    ) -> Result<Response<WalletBalanceResponse>, Status> {
        tracing::info!("Request for wallet balance.");
        let wallet_balance = self.inner.wallet.get_balance().unwrap();

        let response = WalletBalanceResponse {
            confirmed: wallet_balance.confirmed.to_sat(),
            unconfirmed: (wallet_balance.trusted_pending + wallet_balance.untrusted_pending)
                .to_sat(),
        };
        Ok(Response::new(response))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn get_wallet_transactions(
        &self,
        _request: Request<GetWalletTransactionsRequest>,
    ) -> Result<Response<GetWalletTransactionsResponse>, Status> {
        tracing::info!("Request for all wallet transactions.");
        let wallet_transactions = self.inner.wallet.get_transactions().unwrap();
        let transactions: Vec<Vec<u8>> = wallet_transactions
            .iter()
            .map(|t| serde_json::to_vec(&t).unwrap())
            .collect();
        Ok(Response::new(GetWalletTransactionsResponse {
            transactions,
        }))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn list_utxos(
        &self,
        _request: Request<ListUtxosRequest>,
    ) -> Result<Response<ListUtxosResponse>, Status> {
        tracing::info!("Request to list all wallet utxos");
        let utxos = self.inner.wallet.list_utxos().unwrap();
        let utxos: Vec<Vec<u8>> = utxos
            .iter()
            .map(|utxo| serde_json::to_vec(utxo).unwrap())
            .collect();
        Ok(Response::new(ListUtxosResponse { utxos }))
    }

    #[tracing::instrument(skip(self, _request), name = "grpc_server")]
    async fn list_peers(
        &self,
        _request: Request<ListPeersRequest>,
    ) -> Result<Response<ListPeersResponse>, Status> {
        tracing::info!("List peers request");
        let peers = self.inner.transport.peer_manager.list_peers();
        let peers = peers
            .iter()
            .map(|peer| {
                let host = match &peer.socket_address {
                    Some(h) => h.to_string(),
                    None => "".to_string(),
                };
                let pubkey = peer.counterparty_node_id.to_string();
                Peer { pubkey, host }
            })
            .collect::<Vec<Peer>>();

        Ok(Response::new(ListPeersResponse { peers }))
    }

    #[tracing::instrument(skip(self, request), name = "grpc_server")]
    async fn connect_peer(
        &self,
        request: Request<ConnectRequest>,
    ) -> Result<Response<ConnectResponse>, Status> {
        let ConnectRequest { pubkey, host } = request.into_inner();
        let pubkey = PublicKey::from_str(&pubkey).unwrap();
        self.inner.transport.connect_outbound(pubkey, &host).await;
        Ok(Response::new(ConnectResponse {}))
    }

    async fn list_oracles(
        &self,
        _request: Request<ListOraclesRequest>,
    ) -> Result<Response<ListOraclesResponse>, Status> {
        let pubkey = self.inner.oracle.get_pubkey().await.unwrap().to_string();
        let name = self.inner.oracle.name();
        Ok(Response::new(ListOraclesResponse { name, pubkey }))
    }

    async fn list_contracts(
        &self,
        _request: Request<ListContractsRequest>,
    ) -> Result<Response<ListContractsResponse>, Status> {
        let contracts = self
            .inner
            .storage
            .get_contracts()
            .map_err(|e| Status::new(Code::Cancelled, e.to_string()))?;
        let contract_bytes: Vec<Vec<u8>> = contracts
            .iter()
            .map(|contract| serialize_contract(contract).unwrap())
            .collect();
        Ok(Response::new(ListContractsResponse {
            contracts: contract_bytes,
        }))
    }

    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let SendRequest {
            address,
            amount,
            fee_rate,
        } = request.into_inner();
        let address = Address::from_str(&address).unwrap().assume_checked();
        let amount = Amount::from_sat(amount);
        let fee_rate = match FeeRate::from_sat_per_vb(fee_rate) {
            Some(f) => f,
            None => return Err(Status::new(Code::InvalidArgument, "Invalid fee rate.")),
        };
        let txn = self.inner.wallet.send_to_address(address, amount, fee_rate);
        if let Ok(tx) = txn {
            Ok(Response::new(SendResponse {
                txid: tx.to_string(),
            }))
        } else {
            Err(Status::new(Code::Internal, "Transaction sending failed."))
        }
    }

    async fn oracle_announcements(
        &self,
        _request: Request<OracleAnnouncementsRequest>,
    ) -> Result<Response<OracleAnnouncementsResponse>, Status> {
        let announcements: Vec<Vec<u8>> = self
            .inner
            .storage
            .get_marketplace_announcements()
            // TODO: fails if no announcements
            .unwrap()
            .iter()
            .map(|ann| serde_json::to_vec(ann).unwrap())
            .collect();
        Ok(Response::new(OracleAnnouncementsResponse { announcements }))
    }
}
