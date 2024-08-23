pub mod ddkrpc;

use std::sync::Arc;

use ddkrpc::ddk_rpc_server::DdkRpc;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::DlcDevKit;
use ddk::{DdkTransport, DdkOracle};
use ddkrpc::{AcceptOfferRequest, AcceptOfferResponse, NewAddressRequest, NewAddressResponse, SendOfferRequest, SendOfferResponse};
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

    async fn send_offer(&self, _request: Request<SendOfferRequest>) -> Result<Response<SendOfferResponse>, Status> {
        todo!()
    }

    async fn accept_offer(&self, _request: Request<AcceptOfferRequest>) -> Result<Response<AcceptOfferResponse>, Status> {
        todo!()
    }

    async fn new_address(&self, _request: Request<NewAddressRequest>) -> Result<Response<NewAddressResponse>, Status> {
        let address = self.inner.wallet.new_external_address().unwrap().to_string();
        let response = NewAddressResponse { address };
        Ok(Response::new(response))
    }
}
