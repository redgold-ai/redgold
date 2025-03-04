use std::future::Future;
use std::time::Instant;
use eframe::egui;
use eframe::egui::{Color32, ComboBox, Context, RichText, ScrollArea, TextStyle, Ui, Widget};
use flume::Sender;
use itertools::Itertools;
use serde::Deserialize;
use crate::gui::app_loop::LocalState;

use strum::IntoEnumIterator;
// 0.17.1
use strum_macros::{EnumIter, EnumString};
use tracing::{error, info};
use redgold_keys::address_external::ToBitcoinAddress;
use redgold_keys::TestConstants;
use redgold_keys::transaction_support::TransactionSupport;
use redgold_keys::util::btc_wallet::SingleKeyBitcoinWallet;
use redgold_schema::{EasyJsonDeser, error_info, ErrorInfoContext, RgResult, WithMetadataHashable};
use redgold_schema::structs::{Address, AddressInfo, CurrencyAmount, ErrorInfo, NetworkEnvironment, PublicKey, SubmitTransactionResponse, SupportedCurrency, Transaction};
use crate::hardware::trezor;
use crate::hardware::trezor::trezor_list_devices;
use redgold_schema::EasyJson;
use redgold_schema::transaction::rounded_balance_i64;
use crate::core::transact::tx_builder_supports::TransactionBuilder;
use redgold_keys::util::mnemonic_support::WordsPass;
use redgold_keys::xpub_wrapper::XpubWrapper;
use crate::core::internal_message::{Channel, new_channel, SendErrorInfo};
use crate::gui::common;
use crate::gui::common::{bounded_text_area, data_item, data_item_multiline_fixed, editable_text_input_copy, medium_data_item, valid_label};
use crate::node_config::NodeConfig;
use redgold_schema::util::lang_util::JsonCombineResult;
use crate::observability::logging::Loggable;
use redgold_schema::local_stored_state::NamedXpub;
use crate::core::transact::tx_builder_supports::TransactionBuilderSupport;
use crate::gui::tabs::{cold_wallet, hot_wallet};


#[derive(Debug, EnumIter, EnumString, PartialEq)]
#[repr(i32)]
pub enum WalletTab {
    Hardware,
    Software,
}

pub struct DeviceListStatus {
    pub device_output: Option<String>,
    last_polled: Instant,
}

impl DeviceListStatus {
    pub fn poll() -> Self {
        let result = trezor_list_devices().ok().flatten();
        Self {
            device_output: result,
            last_polled: Instant::now(),
        }
    }
}

// #[derive(Clone)]
pub struct StateUpdate {
    pub(crate) update: Box<dyn FnMut(&mut LocalState) + Send>,
}

#[derive(Clone, PartialEq)]
enum SendReceiveTabs {
    Send,
    Receive,
    CustomTx,
    // Swap
}

// #[derive(Clone)]
pub struct WalletState {
    tab: WalletTab,
    pub(crate) device_list_status: DeviceListStatus,
    pub(crate) public_key: Option<PublicKey>,
    public_key_msg: Option<String>,
    // futs: Vec<impl Future>
    updates: Channel<StateUpdate>,
    send_receive: Option<SendReceiveTabs>,
    destination_address: String,
    amount_input: String,
    faucet_success: String,
    balance: String,
    balance_f64: Option<f64>,

    balance_btc: Option<String>,
    balance_btc_f64: Option<f64>,

    address_info: Option<AddressInfo>,
    prepared_transaction: Option<Result<Transaction, ErrorInfo>>,
    unsigned_transaction_hash: Option<String>,
    signed_transaction: Option<Result<Transaction, ErrorInfo>>,
    signed_transaction_hash: Option<String>,
    signing_flow_status: Option<String>,
    signing_flow_transaction_box_msg: Option<String>,
    broadcast_transaction_response: Option<Result<SubmitTransactionResponse, ErrorInfo>>,
    pub show_btc_info: bool,
    pub hot_mnemonic_default: String,
    pub send_currency_type: SupportedCurrency,
    pub active_hot_mnemonic: Option<String>,
    pub active_hot_kp: Option<String>,
    pub derivation_path: String,
    pub xpub_derivation_path: String,
    pub derivation_path_valid: bool,
    pub xpub_derivation_path_valid: bool,
    pub derivation_path_last_check: String,
    pub xpub_derivation_path_last_check: String,
    pub mnemonic_checksum: String,
    pub active_xpub: String,
    pub active_derivation_path: String,
    pub xpub_save_name: String,
    pub mnemonic_save_name: String,
    pub mnemonic_save_data: String,
    pub is_mnemonic_or_kp: Option<bool>,
    pub valid_save_mnemonic: String,
    pub show_xpub_loader_window: bool,
    pub selected_xpub_name: String,
    pub selected_key_name: String,
    pub last_selected_key_name: String,
    pub add_new_key_window: bool,
    pub show_save_xpub_window: bool,
    pub purge_existing_xpubs_on_save: bool,
    pub allow_xpub_name_overwrite: bool,
    pub xpub_loader_rows: String,
    pub xpub_loader_error_message: String,
    pub hot_passphrase: String,
    pub hot_offset: String,
    pub custom_tx_json: String,
    pub mnemonic_save_persist: bool,
    pub mark_output_as_stake: bool,
    pub mark_output_as_swap: bool
}

impl WalletState {
    pub(crate) fn update_hot_mnemonic_info(&mut self) {
        let state = self;
        let m = state.hot_mnemonic();
        let check = m.checksum_words().unwrap_or("".to_string());
        // derivation_path_section(ui, &state, m);
        let pk = m.public_at(state.derivation_path.clone());
        state.public_key = pk.ok();
        state.mnemonic_checksum = check;
    }
}


impl WalletState {
    pub fn clear_data(&mut self) {
        self.update_unsigned_tx(None);
        self.update_signed_tx(None);
        self.signing_flow_status = None;
        self.broadcast_transaction_response = None;
        self.signing_flow_transaction_box_msg = None;
        self.faucet_success = "".to_string();
        self.balance_btc = None;
        self.balance = "".to_string();
        self.balance_f64 = None;
        self.balance_btc_f64 = None;
        self.destination_address = "".to_string();
        self.address_info = None;
        self.public_key = None;
        self.send_receive = None;
    }

    pub fn update_signed_tx(&mut self, tx_o: Option<RgResult<Transaction>>) {
        if let Some(tx) = tx_o.as_ref().and_then(|tx| tx.as_ref().ok()) {
            self.signed_transaction_hash = Some(tx.hash_hex_or_missing());
            self.signed_transaction = tx_o.clone();
        } else {
            self.signed_transaction_hash = None;
            self.signed_transaction = None;
        }
    }

    pub fn update_unsigned_tx(&mut self, tx_o: Option<RgResult<Transaction>>) {
        if let Some(tx) = tx_o.as_ref().and_then(|tx| tx.as_ref().ok()) {
            self.unsigned_transaction_hash = Some(tx.hash_hex_or_missing());
            self.prepared_transaction = tx_o.clone()
        } else {
            self.signed_transaction_hash = None;
            self.prepared_transaction = None;
        }
    }

    pub fn hot_mnemonic(&self) -> WordsPass {
        let pass = if self.hot_passphrase.is_empty() {
            None
        } else {
            Some(self.hot_passphrase.clone())
        };
        let m = self.active_hot_mnemonic.as_ref().unwrap_or(&self.hot_mnemonic_default);
        let mut w = WordsPass::new(m, pass.clone());
        if !self.hot_offset.is_empty() {
            w = w.hash_derive_words(self.hot_offset.clone()).expect("err");
            w.passphrase = pass;
        }
        w
    }

    pub fn new(hot_mnemonic: String) -> Self {
        Self {
            tab: WalletTab::Hardware,
            device_list_status: DeviceListStatus::poll(),
            public_key: None,
            public_key_msg: None,
            updates: new_channel(),
            send_receive: None,
            destination_address: "".to_string(),
            amount_input: "".to_string(),
            faucet_success: "".to_string(),
            balance: "loading".to_string(),
            balance_f64: None,
            balance_btc: None,
            balance_btc_f64: None,
            address_info: None,
            prepared_transaction: None,
            unsigned_transaction_hash: None,
            signed_transaction: None,
            signed_transaction_hash: None,
            signing_flow_status: None,
            signing_flow_transaction_box_msg: None,
            broadcast_transaction_response: None,
            show_btc_info: false,
            hot_mnemonic_default: hot_mnemonic,
            send_currency_type: SupportedCurrency::Redgold,
            active_hot_mnemonic: None,
            active_hot_kp: None,
            derivation_path: trezor::default_pubkey_path(),
            xpub_derivation_path: "m/44'/0'/0'".to_string(),
            derivation_path_valid: true,
            xpub_derivation_path_valid: false,
            derivation_path_last_check: "".to_string(),
            xpub_derivation_path_last_check: "".to_string(),
            mnemonic_checksum: "".to_string(),
            active_xpub: "".to_string(),
            active_derivation_path: "".to_string(),
            xpub_save_name: "".to_string(),
            mnemonic_save_name: "".to_string(),
            mnemonic_save_data: "".to_string(),
            is_mnemonic_or_kp: None,
            show_save_xpub_window: false,
            purge_existing_xpubs_on_save: false,
            allow_xpub_name_overwrite: true,
            selected_xpub_name: "Select Xpub".to_string(),
            selected_key_name: "default".to_string(),
            last_selected_key_name: "default".to_string(),
            show_xpub_loader_window: false,
            xpub_loader_rows: "".to_string(),
            xpub_loader_error_message: "".to_string(),
            hot_passphrase: "".to_string(),
            hot_offset: "".to_string(),
            custom_tx_json: "".to_string(),
            valid_save_mnemonic: "".to_string(),
            add_new_key_window: false,
            mnemonic_save_persist: true,
            mark_output_as_stake: false,
            mark_output_as_swap: false,
        }
    }
    pub fn update_hardware(&mut self) {
        if self.device_list_status.last_polled.elapsed().as_secs() > 5 {
            self.device_list_status = DeviceListStatus::poll();
        }
    }
}

pub fn wallet_screen(ui: &mut Ui, ctx: &egui::Context, local_state: &mut LocalState) {
    match local_state.wallet_state.updates.recv_while() {
        Ok(updates) => {
            for mut update in updates {
                info!("Received item of update, applying");
                (update.update)(local_state);
                // info!("New wallet state faucet message: {}", local_state.wallet_state.faucet_success.clone());
            }
        }
        Err(e) => { error!("Error receiving updates: {}", e.json_or()) }
    }
    local_state.wallet_state.update_hardware();
    ui.style_mut().spacing.item_spacing.y = 2f32;
    ui.heading("Wallet");
    ScrollArea::vertical().show(ui, |ui| wallet_screen_scrolled(ui, ctx, local_state));
}


pub trait ValidateDerivationPath {
    fn valid_derivation_path(&self) -> bool;
}

impl ValidateDerivationPath for String {
    fn valid_derivation_path(&self) -> bool {
        WordsPass::words(TestConstants::new().words).public_at(self.clone()).is_ok()
    }
    // fn valid_xpub_path(&self) -> bool {
    //     WordsPass::words(TestConstants::new().words).xprv(self.clone()).is_ok()
    // }
}


pub fn wallet_screen_scrolled(ui: &mut Ui, ctx: &egui::Context, ls: &mut LocalState) {

    // local_state.wallet_state.updates.receiver.tr
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button("Cold Hardware").clicked() {
            ls.wallet_state.clear_data();
            ls.wallet_state.derivation_path = trezor::default_pubkey_path();
            ls.wallet_state.xpub_derivation_path = trezor::default_xpub_path();
            ls.wallet_state.public_key = None;
            ls.wallet_state.tab = WalletTab::Hardware;
            // device_list_status = None;
        }
        if ui.button("Hot Software").clicked() {
            ls.wallet_state.clear_data();
            hot_wallet::init_state(&mut ls.wallet_state);
            ls.wallet_state.tab = WalletTab::Software;
        }
    });

    ui.separator();

    match ls.wallet_state.tab {
        WalletTab::Hardware => {
            cold_wallet::cold_header(ui, &mut ls.wallet_state);
        }
        WalletTab::Software => {
            hot_wallet::hot_header(ls, ui, ctx);
        }
    }

    derivation_path_section(ui, ls);
    hot_passphrase_section(ui, ls);
    xpub_path_section(ui, ls, ctx);

    if let Some(pk) = ls.wallet_state.public_key.clone() {
        proceed_from_pk(ui, ls, &pk);
    }
    ui.spacing();
}

fn hot_passphrase_section(ui: &mut Ui, ls: &mut LocalState) {
    if ls.wallet_state.tab == WalletTab::Software {
        ui.horizontal(|ui| {
            ui.label("Passphrase:");
            egui::TextEdit::singleline(&mut ls.wallet_state.hot_passphrase)
                .desired_width(150f32)
                .password(true).show(ui);
            ui.label("Offset:");
            egui::TextEdit::singleline(&mut ls.wallet_state.hot_offset)
                .desired_width(150f32)
                .show(ui);
            if ui.button("Update").clicked() {
                ls.wallet_state.update_hot_mnemonic_info();
            };
        });
    }
}

fn proceed_from_pk(ui: &mut Ui, ls: &mut LocalState, pk: &PublicKey) {
    data_item(ui, "Public Key:", pk.hex_or());
    let address_str = pk.address()
        .and_then(|a| a.render_string())
        .unwrap_or("Address failure".to_string());
    data_item(ui, "Address", address_str);

    // TODO: Include bitcoin address / ETH address for path 0 here for verification.
    ui.separator();


    ui.heading(RichText::new(format!("Balance: RDG: {} {}",
                                     ls.wallet_state.balance.clone(),
        ls.wallet_state.balance_btc.clone().map(|b| format!("BTC: {}", b)).unwrap_or("".to_string())
    ))
        .color(Color32::LIGHT_GREEN));

    ui.checkbox(&mut ls.wallet_state.show_btc_info, "Show BTC Info / Enable BTC");
    if ls.wallet_state.show_btc_info {
        data_item(ui, "BTC Address", pk.to_bitcoin_address(&ls.node_config.network).unwrap_or("".to_string()));
    }

    send_receive_bar(ui, ls, pk);

    ui.separator();
    ui.spacing();

    if let Some(srt) = &ls.wallet_state.send_receive.clone() {
        let mut show_prepared = true;
        match srt {
            SendReceiveTabs::Send => {
                send_view(ui, ls, pk);
            }
            SendReceiveTabs::Receive => {
                show_prepared = false;
            }
            SendReceiveTabs::CustomTx => {
                ui.label("Enter custom transaction JSON:");
                ui.horizontal(|ui| bounded_text_area(ui, &mut ls.wallet_state.custom_tx_json));
            }
            // SendReceiveTabs::Swap => {
            //     // show_prepared = false;
            //     // swap_view(ui, ls, pk);
            // }
        }
        if show_prepared {
            prepared_view(ui, ls, pk);
        }
    }
}

fn send_view(ui: &mut Ui, ls: &mut LocalState, _pk: &PublicKey) {

    ComboBox::from_label("Currency")
        .selected_text(format!("{:?}", ls.wallet_state.send_currency_type))
        .show_ui(ui, |ui| {
            let styles = vec![SupportedCurrency::Bitcoin, SupportedCurrency::Redgold];
            for style in styles {
                ui.selectable_value(&mut ls.wallet_state.send_currency_type, style.clone(), format!("{:?}", style));
            }
        });
    ui.horizontal(|ui| {
        ui.label("Destination Address");
        let string = &mut ls.wallet_state.destination_address;
        ui.add(egui::TextEdit::singleline(string).desired_width(460.0));
        common::copy_to_clipboard(ui, string.clone());
        let valid_addr = Address::parse(string.clone()).is_ok();
        if valid_addr {
            ui.label(RichText::new("Valid").color(Color32::GREEN));
        } else {
            ui.label(RichText::new("Invalid").color(Color32::RED));
        }
    });
    // TODO: Amount USD and conversions etc.
    ui.horizontal(|ui| {
        ui.label("Amount");
        let string = &mut ls.wallet_state.amount_input;
        ui.add(egui::TextEdit::singleline(string).desired_width(200.0));
        ui.checkbox(&mut ls.wallet_state.mark_output_as_stake, "Mark as Stake");
        ui.checkbox(&mut ls.wallet_state.mark_output_as_swap, "Mark as Swap");

    });

}

fn swap_view(_ui: &mut Ui, _ls: &mut LocalState, _pk: &PublicKey) {
    //
    // ComboBox::from_label("Currency")
    //     .selected_text(format!("{:?}", ls.wallet_state.send_currency_type))
    //     .show_ui(ui, |ui| {
    //         let styles = vec![SupportedCurrency::Bitcoin, SupportedCurrency::Redgold];
    //         for style in styles {
    //             ui.selectable_value(&mut ls.wallet_state.send_currency_type, style.clone(), format!("{:?}", style));
    //         }
    //     });
    // ui.horizontal(|ui| {
    //     ui.label("Destination Address");
    //     let string = &mut ls.wallet_state.destination_address;
    //     ui.add(egui::TextEdit::singleline(string).desired_width(460.0));
    //     common::copy_to_clipboard(ui, string.clone());
    //     let valid_addr = Address::parse(string.clone()).is_ok();
    //     if valid_addr {
    //         ui.label(RichText::new("Valid").color(Color32::GREEN));
    //     } else {
    //         ui.label(RichText::new("Invalid").color(Color32::RED));
    //     }
    // });
    // // TODO: Amount USD and conversions etc.
    // ui.horizontal(|ui| {
    //     ui.label("Amount");
    //     let string = &mut ls.wallet_state.amount_input;
    //     ui.add(egui::TextEdit::singleline(string).desired_width(200.0));
    // });
}

pub fn prepared_view(ui: &mut Ui, ls: &mut LocalState, pk: &PublicKey) {


    if ui.button("Prepare Transaction").clicked() {
        if ls.wallet_state.send_currency_type == SupportedCurrency::Bitcoin {
            if let Ok(amount) = ls.wallet_state.amount_input.parse::<f64>() {
                let mut w = SingleKeyBitcoinWallet::new_wallet(
                    pk.clone(), ls.node_config.network, true
                ).expect("w");
                let result = w.prepare_single(
                    ls.wallet_state.destination_address.clone(),
                    amount
                );
                ls.wallet_state.signing_flow_transaction_box_msg = Some(
                    result.clone().json_or_combine()
                );
                let status = result.map(|_x| "Transaction Prepared".to_string())
                    .unwrap_or("Preparation Failed".to_string());
                ls.wallet_state.signing_flow_status = Some(status);
            }
        } else {
            match &ls.wallet_state.address_info {
                None => {}
                Some(ai) => {
                    let result = prepare_transaction(
                        ai,
                        &ls.wallet_state.amount_input,
                        &ls.wallet_state.destination_address,
                        &ls.wallet_state,
                        &ls.node_config
                    );
                    ls.wallet_state.update_unsigned_tx(Some(result.clone()));
                    ls.wallet_state.signing_flow_transaction_box_msg = Some(
                        result.clone().json_or_combine()
                    );
                    let status = result.map(|_x| "Transaction Prepared".to_string())
                        .unwrap_or("Preparation Failed".to_string());
                    ls.wallet_state.signing_flow_status = Some(status);
                }
            }
        }
        if ls.wallet_state.send_receive == Some(SendReceiveTabs::CustomTx) {
            ls.wallet_state.prepared_transaction = Some(
                ls.wallet_state.custom_tx_json.json_from::<Transaction>()
            )
        }
    }
    if let Some(p) = &ls.wallet_state.signing_flow_transaction_box_msg {
        // ui.with_layout(
        //     Layout::centered_and_justified(Direction::TopDown)
        //     ,|ui|
        ui.label("Rendered Transaction Information"); //);
        ui.spacing();
        let string1 = &mut p.clone();
        common::bounded_text_area(ui, string1);
    }
    if let Some(res) = &ls.wallet_state.prepared_transaction {
        if let Some(t) = res.as_ref().ok() {
            ui.allocate_ui(egui::Vec2::new(500.0, 0.0), |ui| {
                ui.centered_and_justified(|ui| {
                    medium_data_item(ui, "Unsigned Transaction Hash:".to_string(), t.hash_hex_or_missing());
                });
            });
            if ui.button("Sign Transaction").clicked() {
                if ls.wallet_state.send_currency_type == SupportedCurrency::Redgold {
                    match ls.wallet_state.tab {
                        WalletTab::Hardware => {
                            initiate_hardware_signing(
                                t.clone(),
                                ls.wallet_state.updates.sender.clone(),
                                pk.clone().clone(),
                            );
                            ls.wallet_state.signing_flow_status = Some("Awaiting hardware response...".to_string());
                        }
                        WalletTab::Software => {
                            let kp = ls.wallet_state.hot_mnemonic().keypair_at(ls.wallet_state.derivation_path.clone()).expect("kp");
                            let mut t2 = t.clone();
                            let signed = t2.sign(&kp);
                            ls.wallet_state.update_signed_tx(Some(signed));
                        }
                    }
                } else if ls.wallet_state.send_currency_type == SupportedCurrency::Bitcoin {
                    match ls.wallet_state.tab {
                        WalletTab::Hardware => {
                            error!("Hardware signing not supported yet for btc");
                        }
                        WalletTab::Software => {
                            error!("Software signing not yet supported for btc");
                            // let mut w = SingleKeyBitcoinWallet::new_wallet(
                            //     pk.clone(), ls.node_config.network, true
                            // ).expect("w");
                            // let result = w.prepare_single_sign(
                            //     ls.wallet_state.destination_address.clone(),
                            //     ls.wallet_state.amount_input.parse::<f64>().expect("f64")
                            // );
                            // if let Ok(tx) = result {
                            //     let signed = w.sign_single(&tx);
                            //     ls.wallet_state.update_signed_tx(Some(signed));
                        }
                    }
                }
            }
        }
    }
    if let Some(m) = &ls.wallet_state.signing_flow_status {
        ui.label(m);
    }
    if let Some(t) = &ls.wallet_state.signed_transaction {
        if let Some(t) = t.as_ref().ok() {
            medium_data_item(ui, "Signed TX Hash:", ls.wallet_state.signed_transaction_hash.clone().unwrap_or("error".to_string()));
            if ui.button("Broadcast Transaction").clicked() {
                broadcast_transaction(
                    ls.node_config.clone(),
                    t.clone(),
                    NetworkEnvironment::Dev,
                    ls.wallet_state.updates.sender.clone(),
                );
                ls.wallet_state.signing_flow_status = Some("Awaiting broadcast response...".to_string());
            }
        }
    }
}

fn send_receive_bar(ui: &mut Ui, ls: &mut LocalState, pk: &PublicKey) {
    ui.horizontal(|ui| {
        let style = ui.style_mut();
        style.override_text_style = Some(TextStyle::Heading);
        if ui.button("Send").clicked() {
            let some = Some(SendReceiveTabs::Send);
            if ls.wallet_state.send_receive == some.clone() {
                ls.wallet_state.send_receive = None;
            } else {
                ls.wallet_state.send_receive = some;
            }
        }
        if ui.button("Receive").clicked() {
            let some = Some(SendReceiveTabs::Receive);
            if ls.wallet_state.send_receive == some.clone() {
                ls.wallet_state.send_receive = None;
            } else {
                ls.wallet_state.send_receive = some;
            }
        }
        if ui.button("Custom Tx").clicked() {
            let some = Some(SendReceiveTabs::CustomTx);
            if ls.wallet_state.send_receive == some.clone() {
                ls.wallet_state.send_receive = None;
            } else {
                ls.wallet_state.send_receive = some;
            }
        }
        // if ui.button("Swap").clicked() {
        //     let some = Some(SendReceiveTabs::Swap);
        //     if ls.wallet_state.send_receive == some.clone() {
        //         ls.wallet_state.send_receive = None;
        //     } else {
        //         ls.wallet_state.send_receive = some;
        //     }
        // }

        let layout = egui::Layout::right_to_left(egui::Align::RIGHT);

        ui.with_layout(layout, |ui| {

            let url_env = if ls.node_config.network.is_main() {
                "".to_string()
            } else {
                format!("{}.",ls.node_config.network.to_std_string())
            };
            // TODO: Format the address of some xpub.
            let env_formatted_faucet = format!("https://{}explorer.redgold.io/faucet", url_env, );
            ui.hyperlink_to("Faucet", env_formatted_faucet);
            ui.label(ls.wallet_state.faucet_success.clone());
            if ui.button("Refresh Balance").clicked() {
                get_address_info(&ls.node_config, ls.wallet_state.public_key.clone().expect("pk"),
                                 ls.wallet_state.show_btc_info.clone(),
                                 ls.wallet_state.updates.sender.clone(),
                );
            };
        });
    });
}

pub fn derivation_path_section(ui: &mut Ui, ls: &mut LocalState) {
    ui.horizontal(|ui| {
        ui.horizontal(|ui| {
            editable_text_input_copy(ui, "Derivation Path", &mut ls.wallet_state.derivation_path, 150.0);
            if ls.wallet_state.derivation_path != ls.wallet_state.derivation_path_last_check {
                ls.wallet_state.derivation_path_last_check = ls.wallet_state.derivation_path.clone();
                ls.wallet_state.derivation_path_valid = ls.wallet_state.derivation_path.valid_derivation_path();
            }
            valid_label(ui, ls.wallet_state.derivation_path_valid);
        });
        ui.spacing();
        match ls.wallet_state.tab {
            WalletTab::Hardware => {
                if ui.button("Request Public Key").clicked() {
                    ls.wallet_state.public_key = None;
                    ls.wallet_state.public_key_msg = Some("Awaiting input on device...".to_string());
                    // This blocks the entire UI... ah jeez
                    match trezor::get_public_node(ls.wallet_state.derivation_path.clone()).and_then(|x| x.public_key()) {
                        Ok(pk) => {
                            ls.wallet_state.public_key = Some(pk.clone());
                            ls.wallet_state.public_key_msg = Some("Got public key".to_string());
                            get_address_info(
                                &ls.node_config,
                                pk.clone(),
                                ls.wallet_state.show_btc_info,
                                ls.wallet_state.updates.sender.clone(),
                            );
                        }
                        Err(e) => {
                            ls.wallet_state.public_key_msg = Some("Error getting public key".to_string());
                            error!("Error getting public key: {}", e.json_or());
                        }
                    }
                }
            }
            _ => {}
        }
    });
}


fn window_xpub(
    _ui: &mut Ui,
    ls: &mut LocalState,
    ctx: &egui::Context,
) {
    egui::Window::new("Xpub")
        .open(&mut ls.wallet_state.show_save_xpub_window)
        .resizable(false)
        .collapsible(false)
        .min_width(500.0)
        .default_width(500.0)
        .show(ctx, |ui| {
            // Layout doesn't seem to work here.
            // let layout = egui::Layout::top_down(egui::Align::Center);
            // ui.with_layout(layout, |ui| {
            ui.vertical(|ui| {
                data_item_multiline_fixed(ui, "Xpub", ls.wallet_state.active_xpub.clone(), 200.0);
                medium_data_item(ui, "Derivation Path:", ls.wallet_state.xpub_derivation_path.clone());
                editable_text_input_copy(ui, "Name", &mut ls.wallet_state.xpub_save_name, 150.0);
                if ui.button("Save Internal").clicked() {
                    let xpub = ls.wallet_state.active_xpub.clone();
                    let named_xpub = NamedXpub {
                        name: ls.wallet_state.xpub_save_name.clone(),
                        derivation_path: ls.wallet_state.xpub_derivation_path.clone(),
                        xpub,
                        hot_offset: if ls.wallet_state.tab == WalletTab::Software {
                            if ls.wallet_state.hot_offset != "" {
                                Some(ls.wallet_state.hot_offset.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        },
                    };
                    ls.updates.sender.send(StateUpdate {
                        update: Box::new(
                            move |lss: &mut LocalState| {
                                let new_named = named_xpub.clone();
                                let mut new_xpubs = lss.local_stored_state.xpubs.iter().filter(|x| {
                                    x.name != new_named.name
                                }).map(|x| x.clone()).collect_vec();
                                new_xpubs.push(new_named);
                                lss.local_stored_state.xpubs = new_xpubs;
                                lss.persist_local_state_store();
                            })
                    }).unwrap();
                    ;
                    ls.wallet_state.xpub_save_name = "".to_string();
                    LocalState::send_update(&ls.updates, |lss| {
                        lss.wallet_state.show_save_xpub_window = false;
                    })
                }
            });
        });
}


fn parse_xpub_rows(str: &str) -> RgResult<Vec<NamedXpub>> {
    let mut rdr = csv::Reader::from_reader(str.as_bytes());
    let mut res = vec![];
    for result in rdr.deserialize() {
        // Notice that we need to provide a type hint for automatic
        // deserialization.
        let record: NamedXpub = result.error_info("server line parse failure")?;
        res.push(record);
    }
    Ok(res)
}


// TODO: This window exceeds the max size bound for some crazy reason??
fn window_xpub_loader(
    _ui: &mut Ui,
    ls: &mut LocalState,
    ctx: &egui::Context,
) {
    egui::Window::new("Xpub Loader")
        .open(&mut ls.wallet_state.show_xpub_loader_window)
        .resizable(false)
        .collapsible(false)
        .min_width(300.0)
        .default_width(300.0)
        .constrain(true)
        .fixed_size((300.0, 300.0))
        .show(ctx, |ui| {
            // Layout doesn't seem to work here.
            // let layout = egui::Layout::top_down(egui::Align::Center);
            // ui.with_layout(layout, |ui| {
            ui.vertical(|ui| {
                ui.label("Enter CSV data with format name,derivation_path,xpub");
                egui::TextEdit::multiline(&mut ls.wallet_state.xpub_loader_rows)
                    .desired_rows(3)
                    .desired_width(200.0)
                    .ui(ui);
                ui.checkbox(&mut ls.wallet_state.purge_existing_xpubs_on_save, "Purge all existing xpubs on load");
                ui.checkbox(&mut ls.wallet_state.allow_xpub_name_overwrite, "Allow overwrite of xpub by name");
                if ui.button("Save Internal").clicked() {
                    let data = ls.wallet_state.xpub_loader_rows.clone();
                    let parsed = parse_xpub_rows(&*data).ok();
                    if let Some(rows) = parsed {
                        LocalState::send_update(&ls.updates, move |lss| {
                            let rows2 = rows.clone();
                            let names = rows2.iter().map(|n| n.name.clone()).collect_vec();
                            let has_existing = lss.local_stored_state.xpubs.iter().find(|n| names.contains(&n.name)).is_some();
                            if has_existing && !lss.wallet_state.allow_xpub_name_overwrite {
                                lss.wallet_state.xpub_loader_error_message = "Existing xpubs found, please enable overwrite".to_string();
                            } else {
                                if lss.wallet_state.purge_existing_xpubs_on_save {
                                    lss.local_stored_state.xpubs = vec![];
                                }
                                for p in rows2 {
                                    lss.add_named_xpub(true, p.clone()).expect("added");
                                }
                                lss.persist_local_state_store();
                                lss.wallet_state.show_xpub_loader_window = false;
                            }
                        });
                    } else {
                        ls.wallet_state.xpub_loader_error_message = "Failed to parse rows".to_string();
                    }
                }
            });
        });
}


pub fn xpub_path_section(ui: &mut Ui, ls: &mut LocalState, ctx: &Context) {
    window_xpub(ui, ls, ctx);
    window_xpub_loader(ui, ls, ctx);

    ui.horizontal(|ui| {
        ui.horizontal(|ui| {
            editable_text_input_copy(
                ui, "Xpub Derivation Path",
                &mut ls.wallet_state.xpub_derivation_path, 150.0,
            );
            if ls.wallet_state.xpub_derivation_path != ls.wallet_state.xpub_derivation_path_last_check {
                ls.wallet_state.xpub_derivation_path_last_check = ls.wallet_state.xpub_derivation_path.clone();
                ls.wallet_state.xpub_derivation_path_valid = ls.wallet_state.xpub_derivation_path.valid_derivation_path();
            }
            valid_label(ui, ls.wallet_state.xpub_derivation_path_valid);


            if ls.wallet_state.tab == WalletTab::Hardware {
                ui.spacing();

                if ui.button("Request Xpub").clicked() {
                    ls.wallet_state.public_key = None;
                    ls.wallet_state.public_key_msg = Some("Awaiting input on device...".to_string());
                    // This blocks the entire UI... ah jeez
                    match trezor::get_public_node(ls.wallet_state.xpub_derivation_path.clone()).map(|x| x.xpub) {
                        Ok(xpub) => {
                            ls.wallet_state.show_save_xpub_window = true;
                            ls.wallet_state.active_xpub = xpub.clone();
                            let pk = XpubWrapper::new(xpub).public_at(0, 0).expect("xpub failure");
                            ls.wallet_state.public_key = Some(pk.clone());
                            ls.wallet_state.public_key_msg = Some("Got public key".to_string());
                            get_address_info(
                                &ls.node_config,
                                pk,
                                             ls.wallet_state.show_btc_info.clone(),
                                             ls.wallet_state.updates.sender.clone(),
                            );
                        }
                        Err(e) => {
                            ls.wallet_state.public_key_msg = Some("Error getting public key".to_string());
                            error!("Error getting public key: {}", e.json_or());
                        }
                    }
                }
            }
        });
    });


    ui.horizontal(|ui| {
        ComboBox::from_label("Set Xpub Source")
            .selected_text(ls.wallet_state.selected_xpub_name.clone())
            .show_ui(ui, |ui| {
                for style in ls.local_stored_state.xpubs.iter().map(|x| x.name.clone()) {
                    ui.selectable_value(&mut ls.wallet_state.selected_xpub_name, style.clone(), style.to_string());
                }
                ui.selectable_value(&mut ls.wallet_state.selected_xpub_name,
                                    "Select Xpub".to_string(), "Select Xpub".to_string());
            });
        if ui.button("Load Xpub").clicked() {
            let xpub = ls.local_stored_state.xpubs.iter().find(|x|
                x.name == ls.wallet_state.selected_xpub_name);
            if let Some(named_xpub) = xpub {
                let xpub = named_xpub.clone().xpub.clone();
                ls.wallet_state.active_xpub = xpub.clone();
                let pk = XpubWrapper::new(xpub).public_at(0, 0).expect("xpub failure");
                ls.wallet_state.public_key = Some(pk.clone());
                let dp = format!("{}/0/0", named_xpub.derivation_path.clone());
                if ls.wallet_state.tab == WalletTab::Hardware {
                    ls.wallet_state.active_derivation_path = dp;
                } else {
                    ls.wallet_state.derivation_path = dp;
                    ls.wallet_state.xpub_derivation_path = named_xpub.derivation_path.clone();
                    ls.wallet_state.active_derivation_path = named_xpub.derivation_path.clone();
                }
            }
        }
    });
    medium_data_item(ui, "Active Derivation Path:", ls.wallet_state.active_derivation_path.clone());

    if ls.wallet_state.tab == WalletTab::Software {
        if ui.button("Save Xpub").clicked() {
            let xpub = ls.wallet_state.hot_mnemonic().xpub(ls.wallet_state.xpub_derivation_path.clone()).expect("xpub failure");
            ls.wallet_state.active_xpub = xpub.to_string();
            ls.wallet_state.show_save_xpub_window = true;
        }
    }

    if ui.button("Load Xpubs from CSV").clicked() {
        ls.wallet_state.show_xpub_loader_window = true;
    }
}


//
//
// fn spawn_update(fun: impl Future<Output = Box<dyn FnMut(&mut LocalState) + Send>> + std::marker::Send, update_channel: Sender<StateUpdate>) {
//     tokio::spawn(async move {
//         let res = fun.await;
//         let up = StateUpdate {
//             update: Box::new(res),
//         };
//         update_channel.send_err(up).log_error().ok();
//     });
// }

// TODO: Abstract over spawn/send updates
fn broadcast_transaction(nc: NodeConfig, tx: Transaction, ne: NetworkEnvironment, send: Sender<StateUpdate>) {
    tokio::spawn(async move {
        let mut nc = nc.clone();
        nc.network = ne;
        let res = nc.clone().api_client().send_transaction(&tx.clone(), true).await;

        let st = Some(res.clone());
        let st_msg = Some(res.clone().json_or_combine());
        let ss = Some(res
            .map(|_x| "Transaction Accepted".to_string())
            .unwrap_or("Rejected Transaction".to_string()));

        let fun = move |ls: &mut LocalState| {
            ls.wallet_state.broadcast_transaction_response = st.clone();
            ls.wallet_state.signing_flow_transaction_box_msg = st_msg.clone();
            ls.wallet_state.signing_flow_status = ss.clone();
        };
        let up = StateUpdate {
            update: Box::new(fun),
        };
        send.send_err(up).log_error().ok();
    });
}

pub fn initiate_hardware_signing(t: Transaction, send: Sender<StateUpdate>, public: PublicKey) {
    tokio::spawn(async move {
        let t = &mut t.clone();
        let res = trezor::sign_transaction(
            t, public, trezor::default_pubkey_path())
            .await
            .log_error()
            .map(|x| x.clone())
            .map_err(|e| e.clone());

        let st = Some(res.clone());
        let st_msg = Some(res.clone().json_or_combine());
        let ss = Some(res
            .map(|_x| "Signed Successfully".to_string())
            .unwrap_or("Signing error".to_string()));

        let fun = move |ls: &mut LocalState| {
            ls.wallet_state.update_signed_tx(st.clone());
            ls.wallet_state.signing_flow_transaction_box_msg = st_msg.clone();
            ls.wallet_state.signing_flow_status = ss.clone();
        };
        let up = StateUpdate {
            update: Box::new(fun),
        };
        send.send_err(up).log_error().ok();
    });
}

pub fn prepare_transaction(ai: &AddressInfo, amount: &String, destination: &String, x: &WalletState, nc: &NodeConfig)
                           -> Result<Transaction, ErrorInfo> {
    let destination = Address::parse(destination.clone())?;
    let amount = CurrencyAmount::from_float_string(amount)?;
    let mut tb = TransactionBuilder::new(&nc.network);
    let a = ai.address.as_ref().expect("a");
    tb.with_address_info(ai.clone());
    tb.with_output(&destination, &amount);
    if x.mark_output_as_swap {
        tb.with_last_output_withdrawal_swap();
    }
    if x.mark_output_as_stake {
        tb.with_last_output_stake();
        tb.with_stake_usd_bounds(None, None, a);
    }
    if x.mark_output_as_swap && x.mark_output_as_stake {
        return Err(error_info("Cannot mark as both swap and stake"));
    }
    let res = tb.build();
    res
}


pub fn get_address_info(
    node_config: &NodeConfig,
    public_key: PublicKey,
    show_btc_info: bool,
    update_channel: flume::Sender<StateUpdate>,
) {
    let node_config = node_config.clone();
    let address = public_key.address().expect("works");
    let _ = tokio::spawn(async move {

        let btc_bal = if show_btc_info {
            let w = SingleKeyBitcoinWallet::new_wallet(
                public_key.clone(), node_config.network.clone(), true)
                .expect("worx");
            let fb = w.get_wallet_balance().expect("b").confirmed as f64 / 100_000_000.0;
            Some(fb)
        } else {
            None
        };

        let client = node_config.api_client();
        let response = client
            .address_info(address).await;
        let fun: Box<dyn FnMut(&mut LocalState) + Send> = match response {
            Ok(ai) => {
                info!("balance success: {}", ai.json_or());
                Box::new(move |ls: &mut LocalState| {
                    info!("Applied update function inside closure for balance thing");
                    let o = rounded_balance_i64(ai.balance.clone());
                    ls.wallet_state.balance = o.to_string();
                    ls.wallet_state.balance_f64 = Some(o.clone());
                    ls.wallet_state.address_info = Some(ai.clone());
                    ls.wallet_state.balance_btc_f64 = btc_bal.clone();
                    ls.wallet_state.balance_btc = btc_bal.clone().map(|b| b.to_string());
                })
            }
            Err(e) => {
                error!("balance error: {}", e.json_or());
                Box::new(move |ls: &mut LocalState| {
                    ls.wallet_state.balance = "error".to_string();
                })
            }
        };
        let up = StateUpdate {
            update: fun,
        };
        update_channel.send_err(up).log_error().ok();
    });
}


fn handle_faucet(
    node_config: NodeConfig,
    address: Address,
    network: NetworkEnvironment,
    update_channel: flume::Sender<StateUpdate>,
) {
    let mut node_config = node_config;
    node_config.network = network;
    let _ = tokio::spawn(async move {
        let client = node_config.api_client();
        let fun = match client.faucet(&address).await {
            Ok(o) => {
                info!("Faucet success: {}", o.json_or());
                |ls: &mut LocalState| {
                    info!("Applied update function inside closure for faucet thing");
                    ls.wallet_state.faucet_success = "Faucet success".to_string();
                }
            }
            Err(e) => {
                error!("Faucet error: {}", e.json_or());
                |ls: &mut LocalState| {
                    ls.wallet_state.faucet_success = "Faucet error".to_string();
                }
            }
        };
        let up = StateUpdate {
            update: Box::new(fun),
        };
        update_channel.send_err(up).log_error().ok();
    });
}
