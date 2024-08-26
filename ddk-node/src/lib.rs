pub mod ddkrpc;

use std::str::FromStr;
use std::sync::Arc;

use ddk::bdk::bitcoin::secp256k1::PublicKey;
use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::dlc_manager::Storage;
use ddk::dlc_messages::Message;
use ddkrpc::ddk_rpc_server::DdkRpc;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::DlcDevKit;
use ddk::{DdkTransport, DdkOracle};
use ddkrpc::{AcceptOfferRequest, AcceptOfferResponse, ListOffersRequest, ListOffersResponse, NewAddressRequest, NewAddressResponse, SendOfferRequest, SendOfferResponse};
use ddkrpc::{InfoRequest, InfoResponse};
use tonic::async_trait;
use tonic::Request;
use tonic::Response;
use tonic::Status;

type DdkServer = DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

pub struct DdkNode {
    pub inner: Arc<DdkServer>
}

impl DdkNode {
    pub fn new(ddk: DdkServer) -> Self {
        Self { inner: Arc::new(ddk) }
    }
}

#[async_trait]
impl DdkRpc for DdkNode {
    async fn info(&self, _request: Request<InfoRequest>) -> Result<Response<InfoResponse>, Status>{
        let pubkey = self.inner.transport.node_id.to_string();
        let transport = self.inner.transport.name();
        let oracle = self.inner.oracle.name();
        let response = InfoResponse { pubkey, transport, oracle };
        Ok(Response::new(response))
    }

    async fn send_offer(&self, request: Request<SendOfferRequest>) -> Result<Response<SendOfferResponse>, Status> {
        let SendOfferRequest { contract_input, counter_party } = request.into_inner();
        let contract_input: ContractInput = serde_json::from_slice(&contract_input).expect("couldn't get bytes correct");  
        let counter_party = PublicKey::from_str(&counter_party).expect("no public key");
        println!("Worked in server: {}", contract_input.offer_collateral); 
        let offer_msg = self.inner.manager.lock().unwrap().send_offer(&contract_input, counter_party).expect("couldn't send offer");
        let offer_dlc = serde_json::to_vec(&offer_msg).expect("OfferDlc could not be converted to vec.");
        Ok(Response::new(SendOfferResponse { offer_dlc }))
    }

    async fn accept_offer(&self, request: Request<AcceptOfferRequest>) -> Result<Response<AcceptOfferResponse>, Status> {
        let mut contract_id = [0u8; 32];
        let contract_id_bytes = hex::decode(&request.into_inner().contract_id).unwrap();
        contract_id.copy_from_slice(&contract_id_bytes);
        println!("{:?}", contract_id);
        let (_, node_id, accept_dlc) = self.inner.manager.lock().unwrap().accept_contract_offer(&contract_id).unwrap();
        self.inner.transport.send_message(node_id, Message::Accept(accept_dlc.clone()));
        Ok(Response::new(AcceptOfferResponse { node_id: node_id.to_string(), accept_msg: serde_json::to_vec(&accept_dlc).unwrap()}))
    }

    async fn new_address(&self, _request: Request<NewAddressRequest>) -> Result<Response<NewAddressResponse>, Status> {
        let address = self.inner.wallet.new_external_address().unwrap().to_string();
        let response = NewAddressResponse { address };
        Ok(Response::new(response))
    }

    async fn list_offers(&self, _request: Request<ListOffersRequest>) -> Result<Response<ListOffersResponse>, Status> {
        let offers = self.inner.storage.get_contract_offers().unwrap();
        let offers: Vec<Vec<u8>> = offers.iter().map(|offer| serde_json::to_vec(offer).unwrap()).collect();

        Ok(Response::new(ListOffersResponse { offers }))
    }
}
