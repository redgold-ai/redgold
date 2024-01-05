use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use futures::TryFutureExt;
use itertools::{Itertools, max, min};
use log::{error, info};

use redgold_schema::{bytes_data, error_info, ErrorInfoContext, from_hex, from_hex_ref, RgResult, SafeBytesAccess, SafeOption, structs, WithMetadataHashable};
use redgold_schema::structs::{Address, BytesData, ErrorInfo, SupportedCurrency, Hash, InitiateMultipartyKeygenRequest, InitiateMultipartySigningRequest, MultipartyIdentifier, NetworkEnvironment, PublicKey, StandardContractType, SubmitTransactionResponse, Transaction, CurrencyAmount, LiquidityDeposit};
use crate::core::relay::Relay;
use crate::core::stream_handlers::IntervalFold;
use crate::multiparty::initiate_mp;

use serde::{Serialize, Deserialize};
use redgold_keys::transaction_support::{TransactionBuilderSupport, TransactionSupport};
use redgold_schema::transaction_builder::TransactionBuilder;
use redgold_keys::util::btc_wallet::{ExternalTimedTransaction, SingleKeyBitcoinWallet};
use crate::multiparty::initiate_mp::{default_room_id, initiate_mp_keysign};
use crate::node::Node;
use redgold_keys::address_external::{ToBitcoinAddress, ToEthereumAddress};
use crate::util::logging::Loggable;
use redgold_schema::EasyJson;
use redgold_schema::errors::EnhanceErrorInfo;
use redgold_schema::seeds::get_seeds_by_env;
use crate::node_config::NodeConfig;
use crate::scrape::coinbase_btc_spot_latest;
use crate::util::cli::arg_parse_config::ArgTranslate;
use crate::util::cli::args::RgArgs;

#[derive(Serialize, Deserialize, Clone)]
pub struct DepositKeyAllocation {
    pub key: PublicKey,
    pub allocation: f64,
    pub initiate: InitiateMultipartyKeygenRequest,
    pub balance_btc: u64,
    pub balance_rdg: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PriceVolume {
    pub price: f64, // RDG/BTC (in satoshis for both) for now
    pub volume: u64, // Volume of RDG available
}

impl PriceVolume {
    pub fn generate(
        available_volume: u64,
        center_price: f64,
        divisions: i32,
        price_width: f64,
        scale: f64
    ) -> Vec<PriceVolume> {
        let divisions_f64 = divisions as f64;

        // Calculate the common ratio
        let ratio = (1.0 / scale).powf(1.0 / (divisions_f64 - 1.0));

        // Calculate the first term
        let first_term = available_volume as f64 * scale / (1.0 - ratio.powf(divisions_f64));

        let mut price_volumes = Vec::new();

        for i in 0..divisions {
            let price_offset = (i+1) as f64;
            let price = center_price + (price_offset * (price_width/divisions_f64));
            let volume = (first_term * ratio.powi(divisions-i)) as u64;
            price_volumes.push(PriceVolume { price, volume });
        }
        let total_volume = price_volumes.iter().map(|v| v.volume).sum::<u64>();

        // Normalize the volumes so their sum equals available_volume
        for pv in &mut price_volumes {
            pv.volume = ((pv.volume as f64 / total_volume as f64) * available_volume as f64) as u64;
        }

        if total_volume != available_volume {
            let delta = total_volume as i64 - available_volume as i64;
            let last = price_volumes.last_mut().unwrap();
            if delta > 0 && (last.volume as u64) > delta as u64 {
                last.volume = ((last.volume as i64) - delta) as u64;
            } else if delta < 0 {
                last.volume = ((last.volume as i64) - delta) as u64;
            }
        }
        price_volumes
    }
}

#[test]
fn inspect_price_volume() {
    let pv = PriceVolume::generate(1_000_000, 1., 25, -0.5, 10.0);
    for p in pv.iter() {
        println!("{}, {}", p.price, p.volume);
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BidAsk{
    pub bids: Vec<PriceVolume>,
    pub asks: Vec<PriceVolume>,
    pub center_price: f64
}

impl BidAsk {

    pub fn asking_price(&self) -> f64 {
        self.asks.get(0).map(|v| v.price).unwrap_or(0.)
    }

    pub fn sum_bid_volume(&self) -> u64 {
        self.bids.iter().map(|v| v.volume).sum::<u64>()
    }

    pub fn sum_ask_volume(&self) -> u64 {
        self.asks.iter().map(|v| v.volume).sum::<u64>()
    }

    pub fn volume_empty(&self) -> bool {
        self.bids.iter().find(|v| v.volume == 0).is_some() ||
        self.asks.iter().find(|v| v.volume == 0).is_some()
    }

    pub fn regenerate(&self, price: f64, min_ask: f64) -> BidAsk {
        BidAsk::generate_default(
            self.sum_ask_volume() as i64,
            self.sum_bid_volume(),
            price,
            min_ask
        )
    }

    pub fn generate_default(
        available_balance: i64,
        pair_balance: u64,
        last_exchange_price: f64,
        min_ask: f64,
    ) -> BidAsk {
        BidAsk::generate(
            available_balance,
            pair_balance,
            last_exchange_price,
            50,
            30.,
            min_ask
        )
    }

    pub fn generate(
        available_balance_rdg: i64,
        pair_balance_btc: u64,
        last_exchange_price: f64, // this is for available type / pair type
        divisions: i32,
        scale: f64,
        // BTC / RDG
        min_ask: f64
    ) -> BidAsk {

        // A bid is an offer to buy RDG with BTC
        // The volume should be denominated in BTC because this is how much is staked natively
        let bids = PriceVolume::generate(
            pair_balance_btc,
            last_exchange_price, // Price here is RDG/BTC
            divisions,
            -(last_exchange_price*0.9),
            scale
        );


        // An ask price in the inverse of a bid price, since we want to denominate in RDG
        // since the volume is in RDG.
        // Here it is now BTC / RDG
        let ask_price_expected = 1.0 / last_exchange_price;

        // Apply a max to ask price.
        let ask_price = f64::max(ask_price_expected, min_ask);

        // An ask is how much BTC is being asked for each RDG
        // Volume is denominated in RDG because this is what the contract is holding for resale
        let asks = PriceVolume::generate(
            available_balance_rdg as u64,
            ask_price,
            divisions,
            ask_price*3.0,
            scale
        );
        BidAsk {
            bids,
            asks,
            center_price: last_exchange_price,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OrderFulfillment {
    pub order_amount: u64,
    pub fulfilled_amount: u64,
    pub updated_curve: Vec<PriceVolume>
}

impl OrderFulfillment {
    pub fn fulfillment_price(&self) -> f64 {
        self.fulfilled_amount as f64 / self.order_amount as f64
    }
}


#[derive(Clone, Serialize, Deserialize)]
pub struct WithdrawalBitcoin {
    outputs: Vec<(String, u64)>,
    updated_bidask: BidAsk,
    used_tx: Vec<Transaction>
}


impl BidAsk {
    pub fn fulfill_taker_order(&self, order_amount: u64, is_ask: bool) -> OrderFulfillment {
        let mut remaining_order_amount = order_amount.clone();
        let mut fulfilled_amount: u64 = 0;
        let mut updated_curve = if is_ask {
            self.asks.clone()
        } else {
            // Reverse the bids and invert the price so we can pop them off in order
            let mut b = self.bids.clone();
            b.reverse();
            for b in b.iter_mut() {
                b.price = 1.0 / b.price;
            }
            b
        };
        for b in updated_curve.iter_mut() {
            // Price is in RDG / BTC (satoshi)
            // Amount is in BTC satoshis, this gives RDG
            let rdg_amount = (b.price * (remaining_order_amount as f64)) as u64;
            if rdg_amount > b.volume {
                // We have more RDG than this ask can fulfill, so we take it all and move on.
                fulfilled_amount += b.volume;
                remaining_order_amount -= (b.volume as f64 / b.price) as u64;
                b.volume = 0;
            } else {
                // We have less RDG than this ask can fulfill, so we take it and stop
                b.volume -= rdg_amount;
                remaining_order_amount = 0;
                break
            }
        };
        // Return bids to the original order
        if !is_ask {
            updated_curve.reverse();
            for b in updated_curve.iter_mut() {
                b.price = 1.0 / b.price;
            }
        }
        OrderFulfillment {
            order_amount,
            fulfilled_amount,
            updated_curve,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DepositWatcherConfig {
    pub deposit_allocations: Vec<DepositKeyAllocation>,
    // TODO: Make this a map over currency type
    pub bid_ask: BidAsk,
    pub last_btc_timestamp: u64,
    pub ask_bid_code_reset: Option<bool>
}

#[derive(Clone)]
pub struct DepositWatcher {
    relay: Relay,
    wallet: Vec<Arc<Mutex<SingleKeyBitcoinWallet>>>
}

impl DepositWatcher {

    // Need to update this to the current test address?
    pub async fn genesis_funding(&self, destination: &Address) -> RgResult<()> {
        let (_, utxos) = Node::genesis_from(self.relay.node_config.clone());
        let u = utxos.get(14).safe_get_msg("Missing utxo")?.clone();
        let a = u.key_pair.address_typed();
        let a_str = a.render_string()?;
        let res = self.relay.ds.transaction_store.query_utxo_id_valid(
            &u.utxo_entry.utxo_id()?.transaction_hash.clone().expect("hash"),
            u.utxo_entry.utxo_id()?.output_index.clone()
        ).await?;
        let uu = u.utxo_entry.clone().json_or();
        if res {
            info!("Sending genesis funding to multiparty address from origin {a_str} using utxo {uu}");
            let mut tb = TransactionBuilder::new();
            tb.with_utxo(&u.utxo_entry)?;
            tb.with_output(&destination, &CurrencyAmount::from(u.utxo_entry.amount() as i64));
            tb.with_stake(100f64, 1000f64, &a);
            let mut tx = tb.build()?;
            tx.sign(&u.key_pair)?;
            self.relay.submit_transaction_sync(&tx).await?;
        } else {
            info!("No genesis funding possible to send");
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CurveUpdateResult {
    updated_bid_ask: BidAsk,
    updated_btc_timestamp: u64,
    updated_allocation: DepositKeyAllocation
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StakeDepositInfo {
    amount: CurrencyAmount,
    deposit: LiquidityDeposit,
    tx_hash: Hash
}

impl DepositWatcher {
    pub fn new(relay: Relay) -> Self {
        Self {
            relay,
            wallet: vec![],
        }
    }
    // pub fn establish_first_allocation(&self) -> RgResult<()> {
    //
    // }

    // TODO: From oracle or api
    pub async fn convert_btc_amount_usd(_timestamp: u64, _amount: u64) -> f64 {
        0.
    }

    pub async fn get_btc_deposits(&mut self, last_timestamp: u64, w: &Arc<Mutex<SingleKeyBitcoinWallet>>) -> Result<(u64, Vec<ExternalTimedTransaction>), ErrorInfo>{
        let pk_hex = w.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
            .public_key.hex_or();

        let mut sourced_tx = w.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
            .get_sourced_tx()?;

        info!("public key: {} Got {} sourced tx raw: {}", pk_hex, sourced_tx.len(), sourced_tx.json_or());

        let mut max_ts: u64 = last_timestamp;
        sourced_tx.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        let mut res = vec![];
        for tx in sourced_tx.iter() {
            if tx.timestamp <= last_timestamp {
                continue;
            }
            if tx.timestamp > max_ts {
                max_ts = tx.timestamp;
            }
            let used = self.relay.ds.multiparty_store.check_bridge_txid_used(&from_hex(tx.tx_id.clone())?).await?;
            if used {
                continue
            }
            res.push(tx.clone())
        }
        Ok((max_ts, res.clone()))
    }

    pub async fn build_rdg_ask_swap_tx(&self,
                                       btc_deposits: Vec<ExternalTimedTransaction>,
                                       bid_ask: BidAsk,
                                       key_address: &structs::Address,
        min_ask: f64
    )
        -> RgResult<(Option<Transaction>, BidAsk)> {

        let mut bid_ask_latest = bid_ask.clone();

        let utxos =
            self.relay.ds.transaction_store.query_utxo_address(&key_address)
                .await?;
        // We're building a transaction FROM some stored input balance we have
        // for our pubkey multisig address
        let mut tb = TransactionBuilder::new();
        for u in &utxos {
            // Check contract type here
            // let o = u.output.safe_get_msg("Missing output on UTXO")?;
            // if let Some(o) = &o.contract.as_ref().and_then(|c| c.standard_contract_type) {
            //     if o == StandardContractType::Swap as i32
            // }
            tb.with_maybe_currency_utxo(u)?;
        }

        for tx in btc_deposits.iter() {
            let destination = tx.other_address.clone();
            let destination_address = structs::Address::from_bitcoin(&destination);
            let ask_fulfillment = bid_ask_latest.fulfill_taker_order(tx.amount, true);
            let destination_amount = ask_fulfillment.fulfilled_amount;

            tb.with_output(&destination_address,
                           &CurrencyAmount::from(destination_amount as i64)
            );
            tb.with_last_output_deposit_swap(tx.tx_id.clone());

            let price = ask_fulfillment.fulfillment_price() * 1.05;
            bid_ask_latest = bid_ask_latest.regenerate(price, min_ask)

        }
        let mut tx_ret = None;
        if !btc_deposits.is_empty() {
            tx_ret = Some(tb.build()?);
        }
        Ok((tx_ret, bid_ask_latest))
    }

    pub async fn send_ask_fulfillment_transaction(&self, tx: &mut Transaction, identifier: MultipartyIdentifier) -> RgResult<SubmitTransactionResponse> {

        let hash = tx.signable_hash();
        let result = initiate_mp_keysign(self.relay.clone(), identifier.clone(),
                                         hash.bytes.safe_get()?.clone(), identifier.party_keys.clone(), None
        ).await?;
        tx.add_proof_per_input(&result.proof);
        self.relay.submit_transaction_sync(tx).await
    }

    pub async fn fulfill_btc_bids(&self, w_arc: &Arc<Mutex<SingleKeyBitcoinWallet>>,
                                  identifier: MultipartyIdentifier, outputs: Vec<(String, u64)>) -> RgResult<String> {
        w_arc.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
            .create_transaction_output_batch(outputs)?;
        let hashes = w_arc.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
            .signable_hashes()?.clone();
        for (i, (hash, hash_type)) in hashes.iter().enumerate() {
            let result = initiate_mp_keysign(self.relay.clone(), identifier.clone(),
                                             BytesData::from(hash.clone()),
                                             identifier.party_keys.clone(), None
            ).await?;
            w_arc.lock()
                .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
                .affix_input_signature(i, &result.proof, hash_type);
        }
        let mut w = w_arc.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?;
        w.sign()?;
        w.broadcast_tx()?;
        Ok(w.txid()?)
    }

    pub async fn update_withdrawal_datastore(&self, withdrawals: WithdrawalBitcoin, txid: String, key_address: &structs::Address) -> RgResult<()> {
        for t in withdrawals.used_tx.iter() {
            let h = t.hash_or();
            let first_input_addr = t.first_input_address();
            let source_address = first_input_addr.safe_get_msg("Missing address")?;
            let input_pk_btc_addr = t.first_input_proof_public_key().as_ref()
                .and_then(|&pk| pk.to_bitcoin_address_network(self.relay.node_config.network.clone()).ok());
            let opt_btc_addr = t.output_bitcoin_address_of(key_address)
                .cloned()
                .and_then(|a| a.render_string().ok())
                .or(input_pk_btc_addr);
            let destination_address_string_btc = opt_btc_addr.safe_get_msg("Missing destination address")?.clone();
            let dest = structs::Address::from_bitcoin(&destination_address_string_btc);
            let amount_rdg = t.output_swap_amount_of(key_address);

            self.relay.ds.multiparty_store.insert_bridge_tx(
                &h.safe_bytes()?.clone(),
                &from_hex_ref(&txid)?,
                // Since the origin of this is on the network through a Redgold transaction,
                // and we're generating a bitcoin transaction from that, then its outgoing
                // To an external network
                true,
                SupportedCurrency::Bitcoin,
                source_address,
                &dest,
                t.time()?.clone(),
                amount_rdg
            ).await?;

        }
        Ok(())
    }


    pub async fn get_rdg_withdrawals_bids(&self, bid_ask: BidAsk, key_address: &structs::Address, min_ask: f64) -> RgResult<WithdrawalBitcoin> {
        let mut bid_ask_latest = bid_ask.clone();
        // These are all transactions that have been sent as RDG to this deposit address,
        // We need to filter out the ones that have already been paid.
        let tx: Vec<structs::Transaction> = self.relay.ds.transaction_store
            .get_filter_tx_for_address(&key_address, 10000, 0, true).await?;

        let mut btc_outputs: Vec<(String, u64)> = vec![];
        let mut tx_res: Vec<Transaction> = vec![];

        for t in tx.iter() {
            let h = t.hash_or();
            let used = self.relay.ds.multiparty_store.check_bridge_txid_used(&h.safe_bytes()?.clone()).await?;
            if !used {
                let input_pk_btc_addr = t.first_input_proof_public_key().as_ref()
                    .and_then(|&pk| pk.to_bitcoin_address_network(self.relay.node_config.network.clone()).ok());
                let opt_btc_addr = t.output_bitcoin_address_of(&key_address).cloned()
                    .and_then(|a| a.render_string().ok())
                    .or(input_pk_btc_addr);
                let amount_rdg = t.output_swap_amount_of(&key_address);
                let destination_address_string_btc = opt_btc_addr.safe_get_msg("Missing destination address")?.clone();

                if amount_rdg > 0 && opt_btc_addr.is_some() {

                    let fulfillment = bid_ask_latest.fulfill_taker_order(amount_rdg as u64, false);
                    btc_outputs.push((destination_address_string_btc.clone(), fulfillment.fulfilled_amount));
                    tx_res.push(t.clone());
                    // In case of failure or error, we need to keep track of the last price that was used so
                    // we can recover the partial state that was updated instead of the full.
                    if bid_ask_latest.volume_empty() {
                        let price = fulfillment.fulfillment_price() * 0.98;
                        bid_ask_latest = bid_ask_latest.regenerate(price, min_ask);
                    }

                }

            }
        }

        Ok(WithdrawalBitcoin {
            outputs: btc_outputs,
            updated_bidask: bid_ask_latest,
            used_tx: tx_res,
        })
    }


    pub async fn process_requests(
        &mut self,
        alloc: &DepositKeyAllocation,
        bid_ask_original: BidAsk,
        last_timestamp: u64,
        w: &Arc<Mutex<SingleKeyBitcoinWallet>>,
    ) -> Result<CurveUpdateResult, ErrorInfo> {
        let key = &alloc.key;
        let key_address = key.address()?;
        let identifier = alloc.initiate.identifier.safe_get().cloned()?;

        let btc_starting_balance = w.lock()
            .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
            .get_wallet_balance()?.confirmed;
        let balance = self.relay.ds.transaction_store.get_balance(&key_address).await?;
        let rdg_starting_balance: i64 = balance.safe_get_msg("Missing balance")?.clone();

        // BTC / RDG
        let min_ask = 1f64 / self.get_starting_center_price_rdg_btc().await?;

        // Grab all recent transactions associated with this key address for on-network
        // transactions
        let tx: Vec<structs::Transaction> = self.relay.ds.transaction_store
            .get_filter_tx_for_address(&key_address, 10000, 0, true).await?;

        let mut staking_deposits = vec![];
        // TODO: Add withdrawal support
        // let mut staking_withdrawals = vec![];

        for t in &tx {
            if let Some((amount, liquidity_request)) = t.liquidity_of(&key_address) {
                if let Some(d) = &liquidity_request.deposit {
                    let d = StakeDepositInfo {
                        amount: amount.clone(),
                        deposit: d.clone(),
                        tx_hash: t.hash_or(),
                    };
                    staking_deposits.push(d);
                }
            }
        }

        let bid_ask = if bid_ask_original.asks.is_empty() && bid_ask_original.bids.is_empty() {
            BidAsk::generate_default(
                rdg_starting_balance,
                btc_starting_balance,
                bid_ask_original.center_price,
                min_ask
            )
        } else {
            bid_ask_original
        };



        info!("Starting watcher process request with balances: RDG:{}, BTC:{} bid_ask: {}", rdg_starting_balance, btc_starting_balance, bid_ask.json_or());


        let mut bid_ask_latest = bid_ask.clone();

        // Prepare Fulfill Asks RDG Transaction from BTC deposits to this multiparty address,
        // but don't yet broadcast the transaction.
        let (updated_last_ts, deposit_txs) = self.get_btc_deposits(last_timestamp, w).await?;

        info!("Found {} new deposits last_updated {} updated_last_ts {} deposit_txs {}",
            deposit_txs.len(), last_timestamp, updated_last_ts, deposit_txs.json_or());

        let (tx, bid_ask_updated_ask_side) = self.build_rdg_ask_swap_tx(
            deposit_txs,
            bid_ask_latest, &key_address.clone(), min_ask).await?;
        // info!("Built RDG ask swap tx: {} bid_ask_updated {}", tx.json_or(), bid_ask_updated_ask_side.json_or());
        if let Some(tx) = tx {
            info!("Sending RDG ask swap tx: {}", tx.json_or());
            self.send_ask_fulfillment_transaction(&mut tx.clone(), identifier.clone()).await?;
        }

        bid_ask_latest = bid_ask_updated_ask_side;

        let withdrawals = self.get_rdg_withdrawals_bids(bid_ask_latest, &key_address, min_ask).await?;
        bid_ask_latest = withdrawals.updated_bidask.clone();

        info!("Found {} new withdrawals {}",
            withdrawals.outputs.len(), withdrawals.json_or());

        if withdrawals.outputs.len() > 0 {
            let txid = self.fulfill_btc_bids(w, identifier, withdrawals.outputs.clone()).await?;
            info!("Fullfilled btc Txid: {}", txid);
            // On failure here really need to handle this somehow?
            self.update_withdrawal_datastore(withdrawals, txid, &key_address).await?;
        }

        let mut updated_allocation = alloc.clone();
        updated_allocation.balance_btc = bid_ask_latest.sum_bid_volume();
        updated_allocation.balance_rdg = bid_ask_latest.sum_ask_volume();
        let update = CurveUpdateResult{
            updated_bid_ask: bid_ask_latest,
            updated_btc_timestamp: updated_last_ts,
            updated_allocation,
        };

        info!("Updated Curve Result {}", update.json_or());

        Ok(update)
    }

    // Returns price in RDG/BTC, i.e. ~300 for USD/RDG 100 and BTC 30k
    pub async fn get_starting_center_price_rdg_btc(&self) -> RgResult<f64> {
        let usd_btc = coinbase_btc_spot_latest().await?.usd_btc()?;
        let starting_usd = 100.0;
        let rdg_btc = usd_btc / starting_usd;
        Ok(rdg_btc)
    }
}


#[async_trait]
impl IntervalFold for DepositWatcher {

    #[tracing::instrument(skip(self))]
    async fn interval_fold(&mut self) -> RgResult<()> {

        info!("Deposit watcher interval fold complete");

        if self.relay.node_config.is_local_debug() {
            return Ok(())
        }

        let usd_btc = coinbase_btc_spot_latest().await.log_error().and_then(|t| t.usd_btc())
            .unwrap_or(30000.0f64);

        let ds = self.relay.ds.clone();
        // TODO: Change to query to include trust information re: deposit score
        // How best to represent this to user? As trustData?
        let _nodes = ds.peer_store.active_nodes(None).await?;

        // Fund from genesis for test purposes
        // self.genesis_funding().await?;

        // let kp = initiate_mp::find_multiparty_key_pairs(self.relay.clone()).await;
        // match kp {
        //     Ok(_) => {}
        //     Err(_) => {}
        // }
        let cfg = ds.config_store.get_json::<DepositWatcherConfig>("deposit_watcher_config").await?;
        //.ok.andthen?
        if let Some(mut cfg) = cfg {
            // if cfg.ask_bid_code_reset.is_none() {
            //     info!("Regenerating starting price due to code reset");
            //     cfg.bid_ask = cfg.bid_ask.regenerate(self.get_starting_center_price_rdg_btc().await);
            //     cfg.ask_bid_code_reset = Some(true);
            //     ds.config_store.insert_update_json("deposit_watcher_config", cfg.clone()).await?;
            // }

            // Check to see if other nodes are dead / not responding, if so, move the thing.
            // Also check bitcoin transaction balances? Find the address they came from.
            // we'll need a guide saying to send from a single account
            if let Some(d) = cfg.deposit_allocations.get(0) {
                info!("Watcher checking deposit allocation pubkey hex: {}", d.key.hex()?);
                if self.wallet.get(0).is_none() {
                    let key = &d.key;
                    let w = SingleKeyBitcoinWallet::new_wallet(key.clone(), self.relay.node_config.network, true)?;
                    self.wallet.push(Arc::new(Mutex::new(w)));
                }
                let w = self.wallet.get(0).cloned();
                if let Some(w) = w {
                    let btc_starting_balance = w.lock()
                        .map_err(|e| error_info(format!("Failed to lock wallet: {}", e).as_str()))?
                        .get_wallet_balance()?.confirmed;

                    let balance = self.relay.ds.transaction_store.get_balance(&d.key.address()?).await?;
                    if balance.map(|x| x > 0).unwrap_or(false) { // && btc_starting_balance > 3500 {
                        let update_result = self.process_requests(
                            d, cfg.bid_ask.clone(), cfg.last_btc_timestamp, &w
                        ).await;
                        if let Ok(update_result) = &update_result {
                            let mut cfg2 = cfg.clone();
                            cfg2.last_btc_timestamp = update_result.updated_btc_timestamp;
                            cfg2.bid_ask = update_result.updated_bid_ask.clone();
                            cfg2.deposit_allocations = vec![update_result.updated_allocation.clone()];
                            ds.config_store.insert_update_json("deposit_watcher_config", cfg2).await?;
                        } else if let Err(e) = update_result {
                            error!("Error processing requests: {}", e.json_or());
                        }
                    } else {
                        info!("No balance found for key: {} or insufficient bitcoin balance of {}", d.key.address()?.render_string()?, btc_starting_balance);
                    }
                }
            }
        } else {
            info!("Attempting to start MP watcher keygen round");
            // Initiate MP keysign etc. gather public key and original proof and params
            let seeds = self.relay.node_config.seeds.clone();
            if seeds.len() < 3 {
                error!("Not enough seeds to initiate MP keygen");
                return Ok(())
            }

            let pks = seeds.iter().flat_map(|s| s.public_key.clone()).collect_vec();

            let res = initiate_mp::initiate_mp_keygen(
                self.relay.clone(),
                None,
                true,
                Some(pks)
            ).await.log_error();
            // TODO: Get this from local share instead of from a second keysign round.
            if let Ok(r) = res {
                let test_sign = r.identifier.uuid.clone();
                let h = Hash::from_string_calculate(&test_sign);
                let bd = h.bytes.safe_get_msg("Missing bytes in immediate hash calculation")?;
                let ksr = initiate_mp::initiate_mp_keysign(
                    self.relay.clone(), r.identifier.clone(),
                    bd.clone(),
                    r.identifier.party_keys.clone(),
                    None
                ).await.log_error();
                if let Ok(ksr) = ksr {
                    // TODO: if not successful, attempt some retries and then delete the operation
                    // and begin again from keygen.
                    // or just delete it immediately.
                    let pk = ksr.proof.public_key.safe_get_msg("Missing public key on key sign result")?;
                    let cfg = DepositWatcherConfig {
                        deposit_allocations: vec![DepositKeyAllocation{
                            key: pk.clone(),
                            allocation: 1.0,
                            initiate: r.request.clone(),
                            balance_btc: 0,
                            balance_rdg: 0,
                        }],
                        bid_ask: BidAsk { bids: vec![], asks: vec![], center_price: self.get_starting_center_price_rdg_btc().await? },
                        last_btc_timestamp: 0,
                        ask_bid_code_reset: None,
                    };
                    self.genesis_funding(&pk.address()?)
                        .await.add("Genesis watcher funding error").log_error().ok();
                    ds.config_store.insert_update_json("deposit_watcher_config", cfg).await?;
                }
            }
            // self.relay.broadcast_async(nodes, req)
        }

        Ok(())
    }
}

#[ignore]
#[tokio::test]
async fn debug_local_ds_utxo_balance() {
    let mut opts = RgArgs::default();
    opts.network = Some("dev".to_string());
    let node_config = NodeConfig::default();
    let mut arg_translate = ArgTranslate::new(&opts, &node_config.clone());
    arg_translate.translate_args().await.unwrap();
    let nc = arg_translate.node_config;
    let r = Relay::new(nc.clone()).await;
    let a = Address::parse("cf4989701946ae307efdb902efd73c13d933efda0ef04bcbc3eef2146850534a").expect("");
    let utxos = r.ds.transaction_store.query_utxo_address(&a).await.unwrap();
    println!("UTXOS: {}", utxos.json_or());
    println!("{}", nc.mnemonic_words.clone());
    let (tx, _gutxos) = Node::genesis_from(nc.clone());
    // let res = r.ds.transaction_store.query_utxo_output_index(&tx.hash_or()).await.unwrap();
    // println!("UTXO: {}", res.json_or());
    println!("Genesis hash {}", tx.hash_or().hex());
    //
    // // Node::prelim_setup(r);
    // for (i,utxo) in gutxos.iter().enumerate() {
    //     let res = r.ds.transaction_store.query_utxo_id_valid(&tx.hash_or(), i as i64).await.unwrap();
    //     if res {
    //         println!("UTXO {i}: {}", utxo.utxo_entry.json_or());
    //     }
    // }
}