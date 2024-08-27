use clap::{Parser, Subcommand};
use ddk::bdk::bitcoin::{Address, Transaction};
use ddk::bdk::LocalOutput;
use ddk::dlc_manager::contract::offered_contract::OfferedContract;
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;
use ddk_node::ddkrpc::{
    AcceptOfferRequest, GetWalletTransactionsRequest, InfoRequest, ListOffersRequest,
    ListUtxosRequest, NewAddressRequest, SendOfferRequest, WalletBalanceRequest,
};

#[derive(Debug, Clone, Parser)]
#[clap(name = "ddk")]
#[command(about = "A CLI tool for DDK", version = "1.0")]
struct DdkCliArgs {
    #[clap(subcommand)]
    pub command: CliCommand,
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    // Gets information about the DDK instance
    Info,
    // Pass a contract input to send an offer
    OfferContract(Offer),
    // Retrieve the offers that ddk-node has received.
    Offers,
    // Accept a DLC offer with the contract id string.
    AcceptOffer(Accept),
    // Wallet commands
    #[clap(subcommand)]
    Wallet(WalletCommand),
}

#[derive(Parser, Clone, Debug)]
struct Offer {
    // Path to a contract input file. Eventually to be a repl asking contract params
    pub contract_input: String,
    // The counterparty for the contract. MUST be already connected.
    pub counter_party: String,
}

#[derive(Clone, Debug, Subcommand)]
enum WalletCommand {
    #[command(about = "Get the wallet balance.")]
    Balance,
    #[command(about = "Generate a new, unused address from the wallet.")]
    NewAddress,
    #[command(about = "Get the wallet transactions.")]
    Transactions,
    #[command(about = "Get the wallet utxos.")]
    Utxos,
}

#[derive(Parser, Clone, Debug)]
struct Accept {
    // The contract id string to accept.
    pub contract_id: String,
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
        CliCommand::OfferContract(contract) => {
            let input_str =
                std::fs::read(contract.contract_input).expect("Could not read contract string.");
            client
                .send_offer(SendOfferRequest {
                    contract_input: input_str,
                    counter_party: contract.counter_party,
                })
                .await?
                .into_inner();
        }
        CliCommand::Offers => {
            let offers_request = client.list_offers(ListOffersRequest {}).await?.into_inner();
            let offers: Vec<OfferedContract> = offers_request
                .offers
                .iter()
                .map(|offer| serde_json::from_slice(offer).unwrap())
                .collect();
            for offer in offers {
                println!("{:?}", offer.id);
                println!("Contract: {}", hex::encode(&offer.id));
            }
        }
        CliCommand::AcceptOffer(accept) => {
            let accept = client
                .accept_offer(AcceptOfferRequest {
                    contract_id: accept.contract_id,
                })
                .await?
                .into_inner();
            println!("Contract Accepted w/ node id: {:?}", accept.node_id)
        }
        CliCommand::Wallet(wallet) => match wallet {
            WalletCommand::Balance => {
                let balance = client
                    .wallet_balance(WalletBalanceRequest::default())
                    .await?
                    .into_inner();
                println!("Balance: {:?}", balance);
            }
            WalletCommand::NewAddress => {
                let address = client
                    .new_address(NewAddressRequest::default())
                    .await?
                    .into_inner();
                println!("{:?}", address)
            }
            WalletCommand::Transactions => {
                let transactions = client
                    .get_wallet_transactions(GetWalletTransactionsRequest::default())
                    .await?
                    .into_inner();
                for tx in transactions.transactions {
                    let transaction: Transaction = serde_json::from_slice(&tx.transaction)?;
                    println!("TxId: {:?}", transaction.txid().to_string());
                    for output in transaction.output {
                        println!(
                            "\t\tValue: {:?}\tAddress: {:?}",
                            output.value,
                            Address::from_script(
                                &output.script_pubkey,
                                ddk::bdk::bitcoin::Network::Regtest
                            )
                        )
                    }
                }
            }
            WalletCommand::Utxos => {
                let utxos = client
                    .list_utxos(ListUtxosRequest::default())
                    .await?
                    .into_inner();
                for utxo in utxos.utxos {
                    let utxo: LocalOutput = serde_json::from_slice(&utxo)?;
                    println!(
                        "TxId: {:?} Index: {:?}",
                        utxo.outpoint.txid, utxo.outpoint.vout
                    );
                    println!(
                        "\t\tAddress: {:?}",
                        Address::from_script(
                            &utxo.txout.script_pubkey,
                            ddk::bdk::bitcoin::Network::Regtest
                        )
                    );
                    println!("\t\tValue: {:?}", utxo.txout.value);
                }
            }
        },
    }

    Ok(())
}
