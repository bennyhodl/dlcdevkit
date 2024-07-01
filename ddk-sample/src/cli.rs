use std::env::current_dir;
use std::net::SocketAddr;
use std::str::FromStr;

use clap::{Parser, Subcommand};
use ddk::transport::PeerInformation;
// use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::DdkStorage;
use ddk::bdk::bitcoin::Address;
use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::DdkTransport;
use ddk::dlc_manager::Storage;
use ddk::dlc_messages::{Message, OfferDlc};
// use ddk::dlc_messages::contract_msgs::ContractInfo;
// use ddk::dlc_messages::oracle_msgs::OracleAnnouncement;

use crate::ApplicationDdk;
/// CLI defines the overall command-line interface.
#[derive(Parser, Debug)]
#[command(name = "DLC Dev Kit Sample")]
#[command(about = "Test out DDK", long_about = None)]
pub struct DdkCli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    #[arg(short, long)]
    port: Option<u16>
}

/// Commands define the possible subcommands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    Info,
    Listpeers,
    Addpeer {
        #[arg(long)]
        pubkey: String,
        #[arg(long)]
        host: String,
    },
    Wallet {
        #[command(subcommand)]
        wallet_subcommand: WalletCommand,
    },
    Listcontracts,
    Listoffers,
    Closedcontracts,
    Contract {
        #[arg(short, long)]
        contract_id: String,
        // could be a file
    },
    Accept {
        #[arg(short, long)]
        offer: String
    },
    SendOffer {
        // #[arg(short, long)]
        // contract_input: String,
        #[arg(short, long)]
        counterparty: String,
        #[arg(short, long)]
        announcement: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WalletCommand {
    Balance,
    NewAddress,
    Send {
        address: String,
        amount: u64,
        sat_vbyte: u64
    }
}

pub fn match_ddk_command(command: Commands, ddk: &ApplicationDdk, output: &mut Vec<String>) -> anyhow::Result<()> {
    match command {
        Commands::Info => {
            output.push(format!("Node id: {}", ddk.transport.node_id.to_string()));
            output.push(format!("Network: {}", ddk.network()))
        }
        Commands::Listpeers => {
            let peers = ddk.transport.peer_manager().get_peer_node_ids();
            if peers.is_empty() {
                return Ok(output.push(format!("No peers")));
            }
            peers.iter().for_each(|peer| {
                output.push(format!("Checking if peer is saved"));
                let connect_to = PeerInformation {
                    pubkey: peer.0.to_string(),
                    host: peer.clone().1.unwrap().to_string()
                };
                ddk.storage.save_peer(connect_to).unwrap();
                output.push(format!("Peer: {}\t\tHost: {:?}", peer.0, peer.1));
            })
        }
        Commands::Addpeer { pubkey, host } => {
            let peer_manager = ddk.transport.peer_manager().clone();
            let pubkey = ddk::bdk::bitcoin::secp256k1::PublicKey::from_str(&pubkey)?;
            let host = SocketAddr::from_str(&host)?;
            tokio::runtime::Builder::new_current_thread().build().unwrap().spawn(async move {
                let _ = lightning_net_tokio::connect_outbound(peer_manager, pubkey, host).await;
            });
             
            output.push(format!("Add: {:?} {:?}", pubkey, host));
        }
        Commands::Listcontracts => {
            let contracts = ddk.storage.get_contracts()?;
            contracts.iter().for_each(|contract| {
                output.push(format!("{:?}\n", contract))
            })
        }
        Commands::Listoffers => {
            let offers = ddk.storage.get_offered_channels()?;
            offers.iter().for_each(|offer| {
                output.push(format!("{:?}", offer))
            })
        }
        Commands::Closedcontracts => {
            let contracts = ddk.storage.get_preclosed_contracts()?;
            contracts.iter().for_each(|_contract| {
                output.push(format!("Closed contracts"))
            })
        }
        Commands::Accept { offer } => {
            let offer = serde_json::from_str::<OfferDlc>(&offer)?;
            let (contract_id, counterparty, accept_offer) = ddk.manager.lock().unwrap().accept_contract_offer(&offer.temporary_contract_id)?;
            ddk.transport.send_message(counterparty, Message::Accept(accept_offer));
            output.push(format!("Accepted offer: {:?}", contract_id));
        }
        Commands::SendOffer { counterparty, .. } => {
            output.push(format!("{:?}", current_dir().unwrap()));
            let file = current_dir().unwrap().join("numerical_contract_input.json");
            let offer_string = std::fs::read_to_string(file).unwrap();
            let counterparty = ddk::bdk::bitcoin::secp256k1::PublicKey::from_str(&counterparty)?;
            // let announcement = serde_json::from_str::<OracleAnnouncement>(&announcement)?;
            let contract_input = serde_json::from_str::<ContractInput>(&offer_string)?;
            let offer = ddk.manager.lock().unwrap().send_offer(&contract_input, counterparty)?;
            let offer_clone = offer.clone();
            output.push(format!("Offered Contract: {:?}", &offer_clone.clone().temporary_contract_id[0..12]));
            output.push(format!("{:?}", offer_clone));

            ddk.transport.send_message(counterparty, Message::Offer(offer));
        }
        Commands::Contract { contract_id } => {
            let mut buf = [0u8; 32];
            buf.copy_from_slice(contract_id.as_bytes());
            let contract = ddk.storage.get_contract(&buf)?;
            output.push(format!("{:?}", contract))
        }
        Commands::Wallet { wallet_subcommand } => {
            match wallet_subcommand {
                WalletCommand::Balance => {
                    let balance = ddk.wallet.get_balance()?;
                    output.push(format!("{:?}", balance))
                },
                WalletCommand::NewAddress => {
                    let address = ddk.wallet.new_external_address()?;
                    output.push(format!("{}", address))
                }
                WalletCommand::Send { address, amount, sat_vbyte } => {
                    let address = Address::from_str(&address)?.assume_checked();
                    let send = ddk.wallet.send_to_address(address, amount, sat_vbyte)?;
                    output.push(format!("Sent transaction: {:?}", send))
                }
            }
        }
    }
    Ok(())
}
