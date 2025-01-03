use crate::cli_opts::{CliCommand, OracleCommand, WalletCommand};
use crate::convert::*;
use crate::ddkrpc::ddk_rpc_client::DdkRpcClient;
use crate::ddkrpc::{
    AcceptOfferRequest, ConnectRequest, GetWalletTransactionsRequest, InfoRequest,
    ListContractsRequest, ListOffersRequest, ListOraclesRequest, ListPeersRequest,
    ListUtxosRequest, NewAddressRequest, OracleAnnouncementsRequest, SendOfferRequest, SendRequest,
    WalletBalanceRequest,
};
use anyhow::anyhow;
use bitcoin::Transaction;
use ddk::util;
use ddk::wallet::LocalOutput;
use ddk_manager::contract::contract_input::ContractInput;
use ddk_manager::contract::offered_contract::OfferedContract;
use ddk_manager::contract::Contract;
use dlc::{EnumerationPayout, Payout};
use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement};
use dlc_messages::{AcceptDlc, OfferDlc};
use inquire::{Select, Text};
use serde_json::Value;
use tonic::transport::Channel;

pub async fn cli_command(
    arg: CliCommand,
    client: &mut DdkRpcClient<Channel>,
) -> anyhow::Result<()> {
    match arg {
        CliCommand::Info => {
            let info = client.info(InfoRequest::default()).await?.into_inner();
            print!("{}", serde_json::to_string_pretty(&info)?);
        }
        CliCommand::OfferContract(arg) => {
            let contract_input = if let Some(file) = arg.contract_input_file {
                let contract_string = std::fs::read_to_string(file)?;
                serde_json::from_str::<ContractInput>(&contract_string)?
            } else {
                let contract_type =
                    Select::new("Select type of contract.", vec!["enum", "numerical"]).prompt()?;
                // TODO: support multiple oracles
                let oracle = client
                    .list_oracles(ListOraclesRequest::default())
                    .await?
                    .into_inner();

                let announcements = client
                    .oracle_announcements(OracleAnnouncementsRequest {})
                    .await?
                    .into_inner()
                    .announcements
                    .iter()
                    .map(|ann| serde_json::from_slice(ann).unwrap())
                    .collect::<Vec<OracleAnnouncement>>()
                    .into_iter()
                    .filter(|ann| ann.oracle_public_key.to_string() == oracle.pubkey)
                    .collect::<Vec<OracleAnnouncement>>()
                    .into_iter()
                    .filter(|ann| {
                        if contract_type == "enum" {
                            matches!(
                                ann.oracle_event.event_descriptor,
                                EventDescriptor::EnumEvent(_)
                            )
                        } else {
                            matches!(
                                ann.oracle_event.event_descriptor,
                                EventDescriptor::DigitDecompositionEvent(_)
                            )
                        }
                    })
                    .collect::<Vec<OracleAnnouncement>>();

                if announcements.is_empty() {
                    println!("There aren't any oracle announcements to choose from.");
                    return Ok(());
                }

                let ann_selection = announcements
                    .clone()
                    .iter()
                    .map(|ann| ann.oracle_event.event_id.clone())
                    .collect::<Vec<String>>();

                let announcement =
                    Select::new("Select known announcement:", ann_selection).prompt()?;

                let selected_announcement = match announcements
                    .iter()
                    .find(|ann| ann.oracle_event.event_id == announcement)
                {
                    Some(a) => a,
                    None => {
                        println!("Couldn't get selected announcement.");
                        return Ok(());
                    }
                };

                match contract_type {
                    "numerical" => {
                        let offer_collateral: u64 =
                            Text::new("Collateral from you (sats):").prompt()?.parse()?;
                        let accept_collateral: u64 =
                            Text::new("Collateral from counterparty (sats):")
                                .prompt()?
                                .parse()?;
                        let fee_rate: u64 =
                            Text::new("Fee rate (sats/vbyte):").prompt()?.parse()?;
                        let min_price: u64 =
                            Text::new("Minimum Bitcoin price:").prompt()?.parse()?;
                        let max_price: u64 =
                            Text::new("Maximum Bitcoin price:").prompt()?.parse()?;
                        let num_steps: u64 =
                            Text::new("Number of rounding steps:").prompt()?.parse()?;
                        let oracle_pubkey = Text::new("Oracle public key:").prompt()?;
                        let event_id = Text::new("Oracle event id:").prompt()?;
                        ddk_payouts::create_contract_input(
                            min_price,
                            max_price,
                            num_steps,
                            offer_collateral,
                            accept_collateral,
                            fee_rate,
                            oracle_pubkey,
                            event_id,
                        )
                    }
                    "enum" => {
                        let offer_collateral: u64 =
                            Text::new("Collateral from you (sats):").prompt()?.parse()?;
                        let accept_collateral: u64 =
                            Text::new("Collateral from your counterparty (sats):")
                                .prompt()?
                                .parse()?;
                        let outcomes = match &selected_announcement.oracle_event.event_descriptor {
                            EventDescriptor::EnumEvent(e) => e.outcomes.clone(),
                            _ => return Err(anyhow!("Not an enum event from announcement.")),
                        };
                        let mut outcome_payouts = Vec::with_capacity(outcomes.len());
                        println!("Specify the payouts for each outcome.");
                        for outcome in outcomes {
                            println!("> Event outcome: {}", outcome);
                            let offer: u64 = Text::new("Your payout:").prompt()?.parse()?;
                            let accept: u64 =
                                Text::new("Counterparty payout:").prompt()?.parse()?;
                            let outcome_payout = EnumerationPayout {
                                outcome,
                                payout: Payout { offer, accept },
                            };
                            outcome_payouts.push(outcome_payout);
                        }
                        let fee_rate: u64 =
                            Text::new("Fee rate (sats/vbyte):").prompt()?.parse()?;
                        // TODO: list possible events.
                        ddk_payouts::enumeration::create_contract_input(
                            outcome_payouts,
                            offer_collateral,
                            accept_collateral,
                            fee_rate,
                            selected_announcement.oracle_public_key.to_string(),
                            selected_announcement.oracle_event.event_id.clone(),
                        )
                    }
                    _ => panic!("Invalid contract type."),
                }
            };

            let contract_input = serde_json::to_vec(&contract_input)?;
            let offer = client
                .send_offer(SendOfferRequest {
                    contract_input,
                    counter_party: arg.counter_party,
                })
                .await?
                .into_inner();
            let offer_hex = hex::encode(&offer.offer_dlc);
            println!("{}", offer_hex);
            let offer_dlc: OfferDlc = serde_json::from_slice(&offer.offer_dlc)?;
            let offer = serde_json::to_string_pretty(&offer_dlc)?;
            print!("{}", offer);
        }
        CliCommand::Offers => {
            let offers_request = client.list_offers(ListOffersRequest {}).await?.into_inner();
            let offers: Vec<OfferedContract> = offers_request
                .offers
                .iter()
                .map(|offer| serde_json::from_slice(offer).unwrap())
                .collect();
            print!("{}", offered_contract_to_string_pretty(offers).unwrap());
        }
        CliCommand::AcceptOffer(accept) => {
            let accept = client
                .accept_offer(AcceptOfferRequest {
                    contract_id: accept.contract_id,
                })
                .await?
                .into_inner();
            let accept_dlc: AcceptDlc = serde_json::from_slice(&accept.accept_dlc)?;
            let accept_hex = hex::encode(&accept.accept_dlc);
            println!("{}", accept_hex);
            let accept_dlc = serde_json::to_string_pretty(&accept_dlc)?;
            print!("{}", accept_dlc)
        }
        CliCommand::Contracts => {
            let contracts = client
                .list_contracts(ListContractsRequest {})
                .await?
                .into_inner()
                .contracts;
            let contract_values = contracts
                .iter()
                .map(|c| {
                    let contract: Contract = util::deserialize_contract(c).unwrap();
                    contract_to_value(&contract)
                })
                .collect::<Vec<Value>>();
            print!("{}", serde_json::to_string_pretty(&contract_values)?)
        }
        CliCommand::Balance => {
            let balance = client
                .wallet_balance(WalletBalanceRequest::default())
                .await?
                .into_inner();
            let pretty_string = serde_json::to_string_pretty(&balance)?;
            println!("{}", pretty_string);
        }
        CliCommand::Wallet(wallet) => match wallet {
            WalletCommand::NewAddress => {
                let address = client
                    .new_address(NewAddressRequest::default())
                    .await?
                    .into_inner();
                let pretty_string = serde_json::to_string_pretty(&address)?;
                println!("{}", pretty_string);
            }
            WalletCommand::Transactions => {
                let transactions = client
                    .get_wallet_transactions(GetWalletTransactionsRequest::default())
                    .await?
                    .into_inner();
                let txns = transactions
                    .transactions
                    .iter()
                    .map(|txn| serde_json::from_slice(txn).unwrap())
                    .collect::<Vec<Transaction>>();
                let txns = serde_json::to_string_pretty(&txns)?;
                print!("{}", txns)
            }
            WalletCommand::Utxos => {
                let utxos = client
                    .list_utxos(ListUtxosRequest::default())
                    .await?
                    .into_inner();
                let local_outputs = utxos
                    .utxos
                    .iter()
                    .map(|utxo| serde_json::from_slice(utxo).unwrap())
                    .collect::<Vec<LocalOutput>>();

                print!("{}", serde_json::to_string_pretty(&local_outputs).unwrap())
            }
            WalletCommand::Send {
                address,
                amount,
                fee_rate,
            } => {
                let txid = client
                    .send(SendRequest {
                        address,
                        amount,
                        fee_rate,
                    })
                    .await?
                    .into_inner();
                print!("{}", serde_json::to_string_pretty(&txid)?)
            }
        },
        CliCommand::Oracle(command) => match command {
            OracleCommand::Announcements => {
                let announcements = client
                    .oracle_announcements(OracleAnnouncementsRequest {})
                    .await?
                    .into_inner()
                    .announcements
                    .iter()
                    .map(|ann| serde_json::from_slice(ann).unwrap())
                    .collect::<Vec<OracleAnnouncement>>();
                print!("{}", serde_json::to_string_pretty(&announcements).unwrap())
            }
        },
        CliCommand::Peers => {
            let peers_response = client
                .list_peers(ListPeersRequest::default())
                .await?
                .into_inner();
            let peers = serde_json::to_string_pretty(&peers_response.peers)?;
            print!("{}", peers)
        }
        CliCommand::Connect { connect_string } => {
            let parts = connect_string.split("@").collect::<Vec<&str>>();
            client
                .connect_peer(ConnectRequest {
                    pubkey: parts[0].to_string(),
                    host: parts[1].to_string(),
                })
                .await?;
            println!("Connected to {}", parts[0])
        }
    }

    Ok(())
}
