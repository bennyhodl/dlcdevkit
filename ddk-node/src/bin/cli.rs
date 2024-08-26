use clap::{Parser, Subcommand};
use ddk::dlc_manager::contract::offered_contract::OfferedContract;
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;
use ddk_node::ddkrpc::{AcceptOfferRequest, InfoRequest, ListOffersRequest, NewAddressRequest, SendOfferRequest};

#[derive(Debug, Clone, Parser)]
#[clap(author, version, about)]
struct  DdkCliArgs {
    #[clap(subcommand)]
    pub command: CliCommand
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    // Gets information about the DDK instance
    Info,
    // Generate a new, unused address from the wallet.
    NewAddress,
    // Pass a contract input to send an offer
    OfferContract(Offer),
    // Retrieve the offers that ddk-node has received.
    Offers,
    // Accept a DLC offer with the contract id string.
    AcceptOffer(Accept)
}

#[derive(Parser, Clone, Debug)]
struct Offer {
    // Path to a contract input file. Eventually to be a repl asking contract params
    pub contract_input: String,
    // The counterparty for the contract. MUST be already connected.
    pub counter_party: String
}

#[derive(Parser, Clone, Debug)]
struct Accept {
    // The contract id string to accept.
    pub contract_id: String
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = DdkCliArgs::parse();

    let mut client = DdkRpcClient::connect("http://127.0.0.1:3030").await?;

    match args.command {
        CliCommand::Info => {
            let info = client.info(InfoRequest::default()).await?.into_inner();
            println!("{:?}", info);
        }
        CliCommand::NewAddress => {
            let address = client.new_address(NewAddressRequest::default()).await?.into_inner();
            println!("{:?}", address)
        }
        CliCommand::OfferContract(contract) => {
            let input_str = std::fs::read(contract.contract_input).expect("Could not read contract string.");
            client.send_offer(SendOfferRequest {contract_input: input_str, counter_party: contract.counter_party }).await?.into_inner();
        }
        CliCommand::Offers => {
            let offers_request = client.list_offers(ListOffersRequest {}).await?.into_inner();
            let offers: Vec<OfferedContract> = offers_request.offers.iter().map(|offer| serde_json::from_slice(offer).unwrap()).collect();
            for offer in offers {
                println!("{:?}", offer.id);
                println!("Contract: {}", hex::encode(&offer.id));
            }
        }
        CliCommand::AcceptOffer(accept) => {
            let accept = client.accept_offer(AcceptOfferRequest { contract_id: accept.contract_id}).await?.into_inner();
            println!("Contract Accepted w/ node id: {:?}", accept.node_id)
        }
    }

    Ok(())
}
