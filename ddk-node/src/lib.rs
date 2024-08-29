pub mod ddkrpc;

use std::str::FromStr;
use std::sync::Arc;

use ddk::bdk::bitcoin::secp256k1::PublicKey;
use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::dlc_manager::Storage;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::DlcDevKit;
use ddk::{DdkOracle, DdkTransport};
use ddkrpc::ddk_rpc_server::DdkRpc;
use ddkrpc::{
    AcceptOfferRequest, AcceptOfferResponse, GetWalletTransactionsRequest, GetWalletTransactionsResponse, ListOffersRequest, ListOffersResponse, ListUtxosRequest, ListUtxosResponse, NewAddressRequest, NewAddressResponse, SendOfferRequest, SendOfferResponse, WalletBalanceRequest, WalletBalanceResponse
};
use ddkrpc::{InfoRequest, InfoResponse};
use tonic::{async_trait, Code};
use tonic::Request;
use tonic::Response;
use tonic::Status;

type DdkServer = DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

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
            let announcement = self.inner.oracle.get_announcement_async(&info.oracles.event_id).await.unwrap();
            oracle_announcements.push(announcement)
        }

        let counter_party = PublicKey::from_str(&counter_party).expect("no public key");
        let offer_msg = self
            .inner
            .send_dlc_offer(&contract_input, counter_party, oracle_announcements).map_err(|e| Status::new(Code::Cancelled, format!("Contract offer could not be sent to counterparty. error={:?}", e)))?;

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
        println!("{:?}", contract_id);
        let (contract_id, counter_party, accept_dlc) = self
            .inner
            .accept_dlc_offer(contract_id).map_err(|_| Status::new(Code::Cancelled, "Contract could not be accepted."))?;

        let accept_dlc = serde_json::to_vec(&accept_dlc).map_err(|_| Status::new(Code::Cancelled, "Accept DLC is malformed to create bytes."))?;

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
            confirmed: wallet_balance.confirmed,
            unconfirmed: wallet_balance.trusted_pending + wallet_balance.untrusted_pending,
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
        let transactions: Vec<ddkrpc::Transaction> = wallet_transactions
            .iter()
            .map(|t| ddkrpc::Transaction {
                transaction: serde_json::to_vec(t).unwrap(),
            })
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
}
