pub mod ddkrpc;

use std::sync::Arc;

use ddkrpc::ddk_rpc_server::DdkRpc;
use ddk::oracle::KormirOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::DlcDevKit;
use ddkrpc::{AcceptOfferRequest, AcceptOfferResponse, NewAddressRequest, NewAddressResponse, SendOfferRequest, SendOfferResponse};
use ddkrpc::{InfoRequest, InfoResponse};
use tonic::async_trait;
use tonic::Request;
use tonic::Response;
use tonic::Status;

type DdkServer = DlcDevKit<LightningTransport, SledStorageProvider, KormirOracleClient>;

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
        let response = InfoResponse {
            pubkey: "pub".to_string()
        };
        Ok(Response::new(response))
    }

    async fn send_offer(&self, _request: Request<SendOfferRequest>) -> Result<Response<SendOfferResponse>, Status> {
        todo!()
    }

    async fn accept_offer(&self, _request: Request<AcceptOfferRequest>) -> Result<Response<AcceptOfferResponse>, Status> {
        todo!()
    }

    async fn new_address(&self, _request: Request<NewAddressRequest>) -> Result<Response<NewAddressResponse>, Status> {
        let response = NewAddressResponse {
            address: "address".into()
        };
        Ok(Response::new(response))
    }
}
