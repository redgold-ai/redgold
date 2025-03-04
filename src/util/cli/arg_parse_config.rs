use std::fs;
use std::io::Read;
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::{abort, exit};
use std::slice::Iter;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bdk::bitcoin::bech32::ToBase32;
use clap::{Args, Parser, Subcommand};
use crypto::digest::Digest;
use crypto::sha2::Sha256;
#[allow(unused_imports)]
use futures::StreamExt;
use itertools::Itertools;
use log::{error, info};
use tokio::runtime::Runtime;

use redgold_data::data_store::DataStore;
use redgold_keys::util::mnemonic_support::WordsPass;
use redgold_schema::{error_info, ErrorInfoContext, from_hex, RgResult, SafeBytesAccess, SafeOption};
use redgold_schema::constants::default_node_internal_derivation_path;
use redgold_schema::EasyJson;
use redgold_schema::seeds::get_seeds_by_env;
use redgold_schema::servers::Server;
use redgold_schema::structs::{ErrorInfo, Hash, PeerId, Seed, TrustData};

use crate::{e2e, gui, util};
use crate::api::RgHttpClient;
use crate::node_config::NodeConfig;
// use crate::gui::image_capture::debug_capture;
use crate::observability::logging::Loggable;
use crate::observability::metrics_registry;
use crate::schema::structs::NetworkEnvironment;
use crate::util::{init_logger, init_logger_main, ip_lookup, not_local_debug_mode, sha256_vec};
use crate::util::cli::{args, commands};
use crate::util::cli::args::{GUI, NodeCli, RgArgs, RgTopLevelSubcommand, TestCaptureCli};
use crate::util::cli::data_folder::DataFolder;

// https://github.com/mehcode/config-rs/blob/master/examples/simple/src/main.rs

pub fn get_default_data_top_folder() -> PathBuf {
    let home_or_current = dirs::home_dir()
        .expect("Unable to find home directory for default data store path as path not specified explicitly")
        .clone();
    let redgold_dir = home_or_current.join(".rg");
    redgold_dir
}

pub struct ArgTranslate {
    // runtime: Arc<Runtime>,
    pub opts: RgArgs,
    pub node_config: NodeConfig,
    pub args: Vec<String>,
    pub abort: bool,
}

impl ArgTranslate {

    pub fn new(
        // runtime: Arc<Runtime>,
        opts: &RgArgs, node_config: &NodeConfig) -> Self {
        let args = std::env::args().collect_vec();
        let mut config = node_config.clone();
        config.opts = opts.clone();
        ArgTranslate {
            // runtime,
            opts: opts.clone(),
            node_config: config,
            args,
            abort: false
        }
    }

    pub fn is_gui(&self) -> bool {
        if let Some(sc) = &self.opts.subcmd {
            match sc {
                RgTopLevelSubcommand::GUI(_) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub fn is_node(&self) -> bool {
        if let Some(sc) = &self.opts.subcmd {
            match sc {
                RgTopLevelSubcommand::Node(_) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub fn secure_data_path_string() -> Option<String> {
        std::env::var("REDGOLD_SECURE_DATA_PATH").ok()
    }

    pub fn secure_data_path_buf() -> Option<PathBuf> {
        std::env::var("REDGOLD_SECURE_DATA_PATH").ok().map(|a| PathBuf::from(a))
    }

    pub fn secure_data_or_cwd() -> PathBuf {
        Self::secure_data_path_string().map(|s|
            std::path::Path::new(&s).to_path_buf()
        ).unwrap_or(std::env::current_dir().ok().expect("Can't get current dir"))
    }

    pub fn load_internal_servers(&mut self) -> Result<(), ErrorInfo> {
        // TODO: From better data folder options
        let data_folder = Self::secure_data_or_cwd();
        let rg = data_folder.join(".rg");
        let df = DataFolder::from_path(rg);
        if let Some(servers) = df.all().servers().ok() {
            self.node_config.servers = servers;
        }
        Ok(())
    }

    pub fn read_servers_file(servers: PathBuf) -> Result<Vec<Server>, ErrorInfo> {
        let result = if servers.is_file() {
            let contents = fs::read_to_string(servers)
                .error_info("Failed to read servers file")?;
            let servers = Server::parse(contents)?;
            servers
        } else {
            vec![]
        };
        Ok(result)
    }

    pub async fn translate_args(&mut self) -> Result<(), ErrorInfo> {
        self.immediate_debug();
        self.set_gui_on_empty();
        self.check_load_logger()?;
        self.determine_network()?;
        self.ports();
        metrics_registry::register_metrics(self.node_config.port_offset);
        self.data_folder()?;
        self.secure_data_folder();
        self.load_mnemonic().await?;
        self.load_peer_id()?;
        // self.load_peer_tx()?;
        self.set_public_key();
        self.load_internal_servers()?;
        self.calculate_executable_checksum_hash();
        self.guard_faucet();
        self.e2e_enable();
        self.configure_seeds().await;
        self.set_discovery_interval();
        self.apply_node_opts();
        self.genesis();
        self.alias();

        self.abort = immediate_commands(&self.opts, &self.node_config, self.args()).await;
        if self.abort {
            return Ok(());
        }

        // Unnecessary for CLI commands, hence after immediate commands
        self.lookup_ip().await;

        tracing::info!("Starting node with data store path: {}", self.node_config.data_store_path());
        tracing::info!("Parsed args successfully with args: {:?}", self.args);
        tracing::info!("RgArgs options parsed: {:?}", self.opts);
        info!("Development mode: {}", self.opts.development_mode);

        Ok(())
    }

    fn set_discovery_interval(&mut self) {
        if !self.node_config.is_local_debug() {
            self.node_config.discovery_interval = Duration::from_secs(60)
        }
    }

    fn guard_faucet(&mut self) {
        // Only enable on main if CLI flag with additional precautions
        if self.node_config.network == NetworkEnvironment::Main {
            self.node_config.faucet_enabled = false;
        }
    }

    async fn lookup_ip(&mut self) {

        std::env::var("REDGOLD_EXTERNAL_IP").ok().map(|a| {
            // TODO: First determine if this is an nslookup requirement
            let parsed = IpAddr::from_str(&a);
            match parsed {
                Ok(_) => {
                    self.node_config.external_ip = a;
                }
                Err(_) => {
                    let lookup = dns_lookup::lookup_host(&a);
                    match lookup {
                        Ok(addr) => {
                            if addr.len() > 0 {
                                self.node_config.external_ip = addr[0].to_string();
                            }
                        }
                        Err(_) => {
                            error!("Invalid REDGOLD_EXTERNAL_IP environment variable: {}", a);
                        }
                    }
                }
            }
            // self.node_config.external_ip = a;
        });
        // TODO: We can use the lb or another node to check if port is reciprocal open
        // TODO: Check ports open in separate thing
        // TODO: Also set from HOSTNAME maybe? With nslookup for confirmation of IP?
        if !self.node_config.is_local_debug() &&
            self.node_config.external_ip == "127.0.0.1".to_string() &&
            !self.is_gui() {
            let ip =
                // runtime.block_on(
                ip_lookup::get_self_ip()
                    .await
                    .expect("Ip lookup failed");
            info!("Assigning external IP from ip lookup: {}", ip);
            self.node_config.external_ip = ip;
        }
    }

    fn calculate_executable_checksum_hash(&mut self) {

        let path_exec = std::env::current_exe().expect("Can't find the current exe");

        let buf1 = path_exec.clone();
        let path_str = buf1.to_str().expect("Path exec format failure");
        info!("Path of current executable: {:?}", path_str);
        let exec_name = path_exec.file_name().expect("filename access failure").to_str()
            .expect("Filename missing").to_string();
        info!("Filename of current executable: {:?}", exec_name.clone());
        // This is somewhat slow for loading the GUI
        // let self_exe_bytes = fs::read(path_exec.clone()).expect("Read bytes of current exe");
        // let mut md5f = crypto::md5::Md5::new();
        // md5f.input(&*self_exe_bytes);
        //
        // info!("Md5 of currently running executable with read byte {}", md5f.result_str());
        // let sha256 = sha256_vec(&self_exe_bytes);
        // info!("Sha256 of currently running executable with read byte {}", hex::encode(sha256.to_vec()));

        // let sha3_256 = Hash::calc_bytes(self_exe_bytes);
        // info!("Sha3-256 of current exe {}", sha3_256.hex());

        use std::process::Command;

        let shasum = calc_sha_sum(path_str.to_string()).log_error().ok();

        self.node_config.executable_checksum = shasum.clone();
        info!("Executable checksum Sha256 from shell script: {:?}", shasum);
    }

    async fn load_mnemonic(&mut self) -> Result<(), ErrorInfo> {

        // Remove any defaults; we want to be explicit
        self.node_config.mnemonic_words = "".to_string();

        // First try to load from the all environment data folder for re-use across environments
        if let Ok(words) = self.node_config.data_folder.all().mnemonic().await {
            self.node_config.mnemonic_words = words;
        };

        // Then override with environment specific mnemonic;
        if let Ok(words) = self.node_config.env_data_folder().mnemonic().await {
            self.node_config.mnemonic_words = words;
        };

        // TODO: Merge this with CLI
        // Then override with environment variable
        if let Some(words) = std::env::var("REDGOLD_WORDS").ok() {
            self.node_config.mnemonic_words = words;
        };


        // Then override with command line
        if let Some(words) = &self.opts.words {
            self.node_config.mnemonic_words = words.clone();
        }

        // Then override with a file from the command line (more secure than passing directly)
        if let Some(words) = &self.opts
            .mnemonic_path
            .clone()
            .map(fs::read_to_string)
            .map(|x| x.expect("Something went wrong reading the mnemonic_path file")) {
            self.node_config.mnemonic_words = words.clone();
        };


        // If empty, generate a new mnemonic;
        if self.node_config.mnemonic_words.is_empty() {
            tracing::info!("Unable to load mnemonic for wallet / node keys, attempting to generate new one");
            tracing::info!("Generating with entropy for 24 words, process may halt if insufficient entropy on system");
            let mnem = WordsPass::generate()?.words;
            tracing::info!("Successfully generated new mnemonic");
            self.node_config.mnemonic_words = mnem.clone();
            let buf = self.node_config.env_data_folder().mnemonic_path();
            fs::write(
                buf.clone(),
                self.node_config.mnemonic_words.clone()).expect("Unable to write mnemonic to file");

            info!("Wrote mnemonic to path: {}", buf.to_str().expect("Path format failure"));
        };

        // Validate that this is loadable
        let _ = WordsPass::words(self.node_config.mnemonic_words.clone()).mnemonic()?;

        Ok(())
    }

    // TODO: Load merkle tree of this
    fn load_peer_id(&mut self) -> Result<(), ErrorInfo> {
        // // TODO: Use this
        // let _peer_id_from_store: Option<String> = None; // mnemonic_store.get(0).map(|x| x.peer_id.clone());

        // TODO: From environment variable too?
        // TODO: write merkle tree to disk

        if let Some(path) = &self.opts.peer_id_path {
            let p = fs::read_to_string(path)
                .error_info("Failed to read peer_id_path file")?;
            self.node_config.peer_id = PeerId::from_hex(p)?;
        }

        // TODO: This will have to change to read the whole merkle tree really, lets just remove this maybe?
        if let Some(p) = &self.opts.peer_id {
            self.node_config.peer_id = PeerId::from_hex(p)?;
        }

        if let Some(p) = fs::read_to_string(self.node_config.env_data_folder().peer_id_path()).ok() {
            self.node_config.peer_id = PeerId::from_hex(p)?;
        }

        if self.node_config.peer_id.peer_id.is_none() {
            tracing::info!("No peer_id found, attempting to generate a single key peer_id from existing mnemonic");
            // let string = self.node_config.mnemonic_words.clone();
            // TODO: we need to persist the merkle tree here as json or something
            // let tree = crate::node_config::peer_id_from_single_mnemonic(string)?;
            self.node_config.peer_id = self.node_config.default_peer_id()?;
        }

        info!("Starting with peer id {}", self.node_config.peer_id.json_or());

        Ok(())

    }

    fn data_folder(&mut self) -> Result<(), ErrorInfo> {

        let mut data_folder_path =  self.opts.data_folder.clone()
            .map(|p| PathBuf::from(p))
            .unwrap_or(get_default_data_top_folder());

        // Testing only modification, could potentially do this in a separate function to
        // unify this with other debug mods.
        if let Some(id) = self.opts.debug_id {
            data_folder_path = data_folder_path.join("local_test");
            data_folder_path = data_folder_path.join(format!("id_{}", id));
        }

        self.node_config.data_folder = DataFolder { path: data_folder_path };
        self.node_config.data_folder.ensure_exists();
        self.node_config.env_data_folder().ensure_exists();

        Ok(())
    }

    fn ports(&mut self) {
        self.node_config.port_offset = self.node_config.network.default_port_offset();

        // Unify with other debug id stuff?
        if let Some(dbg_id) = self.opts.debug_id {
            self.node_config.port_offset = Self::debug_id_port_offset(
                self.node_config.network.default_port_offset(),
                dbg_id
            );
        }
    }

    fn debug_id_port_offset(offset: u16, debug_id: i32) -> u16 {
        offset + ((debug_id * 1000) as u16)
    }

    // pub fn parse_seed(&mut self) {
    //     if let Some(a) = &self.opts.seed_address {
    //         let default_port = self.node_config.network.default_port_offset();
    //         let port = self.opts.seed_port_offset.map(|p| p as u16).unwrap_or(default_port);
    //         self.node_config.seeds.push(SeedNode {
    //             peer_id: vec![],
    //             trust: 1.0,
    //             public_key: None,
    //             external_address: a.clone(),
    //             port
    //         });
    //     }
    // }
    fn check_load_logger(&mut self) -> Result<(), ErrorInfo> {
        let log_level = &self.opts.log_level
            .clone()
            .and(std::env::var("REDGOLD_LOG_LEVEL").ok())
            .unwrap_or("DEBUG".to_string());
        let mut enable_logger = false;

        if let Some(sc) = &self.opts.subcmd {
            enable_logger = match sc {
                RgTopLevelSubcommand::GUI(_) => { true }
                RgTopLevelSubcommand::Node(_) => { true }
                RgTopLevelSubcommand::TestTransaction(_) => {true}
                _ => { false }
            }
        }
        if enable_logger {
            init_logger_main(log_level.clone());
        }
        self.node_config.enable_logging = enable_logger;
        self.node_config.log_level = log_level.clone();


        Ok(())
    }
    fn determine_network(&mut self) -> Result<(), ErrorInfo> {
        if let Some(n) = std::env::var("REDGOLD_NETWORK").ok() {
            NetworkEnvironment::parse_safe(n)?;
        }
        self.node_config.network = match &self.opts.network {
            None => {
                if util::local_debug_mode() {
                    NetworkEnvironment::Debug
                } else {
                    NetworkEnvironment::Local
                }
            }
            Some(n) => {
                NetworkEnvironment::parse_safe(n.clone())?
            }
        };

        if self.is_gui() && self.node_config.network == NetworkEnvironment::Local {
            if self.opts.development_mode {
                self.node_config.network = NetworkEnvironment::Dev;
            } else {
                self.node_config.network = NetworkEnvironment::Main;
            }
        }

        if self.node_config.network == NetworkEnvironment::Local || self.node_config.network == NetworkEnvironment::Debug {
            self.node_config.disable_auto_update = true;
            self.node_config.load_balancer_url = "127.0.0.1".to_string();
        }
        Ok(())
    }

    fn e2e_enable(&mut self) {

        if self.opts.disable_e2e {
            self.node_config.e2e_enabled = false;
        }
        // std::env::var("REDGOLD_ENABLE_E2E").ok().map(|b| {
        //     self.node_config.e2e_enable = true;
        // }
        // self.opts.enable_e2e.map(|_| {
        //     self.node_config.e2e_enable = true;
        // });
    }
    async fn configure_seeds(&mut self) {

        let seeds = get_seeds_by_env(&self.node_config.network);
        for seed in seeds {
            self.node_config.seeds.push(seed);
        }


        let port = self.node_config.public_port();
        // Enrich keys for missing seed info
        if self.is_node() {
            for seed in self.node_config.seeds.iter_mut() {
                if seed.public_key.is_none() {
                    info!("Querying seed: {}", seed.external_address.clone());

                    let response = RgHttpClient::new(
                        seed.external_address.clone(),
                                                     port, // TODO: Account for seed listed offset instead of direct.
                                                     // seed.port_offset.map(|p| (p + 1) as u16)
                                                     //     .unwrap_or(port),
                                                     None
                    ).about().await;
                    if let Ok(response) = response {
                        let nmd = response.peer_node_info.as_ref()
                            .and_then(|n| n.latest_node_transaction.as_ref())
                            .and_then(|n| n.node_metadata().ok());
                        let pk = nmd.as_ref().and_then(|n| n.public_key.as_ref());
                        let pid = nmd.as_ref().and_then(|n| n.peer_id.as_ref());
                        if let (Some(pk), Some(pid)) = (pk, pid) {
                            info!("Enriched seed {} public {} peer id {}", seed.external_address.clone(), pk.json_or(), pid.json_or());
                            seed.public_key = Some(pk.clone());
                            seed.peer_id = Some(pid.clone());
                        }
                    }
                }
            }
        }
        let mut remove_index = vec![];
        for (i, seed) in self.node_config.seeds.iter().enumerate() {
            if let Some(pk) = &seed.public_key {
                if &self.node_config.public_key() == pk {
                    info!("Removing self from seeds");
                    remove_index.push(i);
                }
            }
        }
        for i in remove_index {
            self.node_config.seeds.remove(i);
        }

        // TODO: Test config should pass ids so we get ids for local_test
        if let Some(a) = &self.opts.seed_address {

            let default_port = self.node_config.network.default_port_offset();
            let port = self.opts.seed_port_offset.map(|p| p as u16).unwrap_or(default_port);
            info!("Adding seed from command line arguments {a}:{port}");
            // TODO: replace this with the other seed class.
            self.node_config.seeds.push(Seed {
                external_address: a.clone(),
                environments: vec![self.node_config.network as i32],
                port_offset: Some(port as u32),
                trust: vec![TrustData::from_label(1.0)],
                peer_id: None, // Some(self.node_config.peer_id()),
                public_key: None, //Some(self.node_config.public_key()),
            });
        }


    }
    fn apply_node_opts(&mut self) {
        match &self.opts.subcmd {
            Some(RgTopLevelSubcommand::Node(node_cli)) => {
                if let Some(i) = &node_cli.live_e2e_interval {
                    self.node_config.live_e2e_interval = Duration::from_secs(i.clone());
                }
            }
            _ => {}
        }
    }
    fn genesis(&mut self) {
        if let Some(o) = std::env::var("REDGOLD_GENESIS").ok() {
            if let Ok(b) = o.parse::<bool>() {
                self.node_config.genesis = b;
            }
        }
        if self.opts.genesis {
            self.node_config.genesis = true;
        }
        if self.node_config.genesis {
            self.node_config.seeds.push(self.node_config.self_seed())
        }
        if self.node_config.genesis {
            info!("Starting node as genesis node");
        }
    }

    fn args(&self) -> Vec<&String> {
        // First argument is the executable path
        self.args.iter().dropping(1).collect_vec()
    }

    fn set_gui_on_empty(&mut self) {
        // println!("args: {:?}", self.args.clone());

        if self.args.len() == 1 || self.opts.subcmd.is_none() {
            self.opts.subcmd = Some(RgTopLevelSubcommand::GUI(GUI{}));
        }

    }
    fn set_public_key(&mut self) {
        let pk = self.node_config.public_key();
        self.node_config.public_key = pk.clone();
        info!("Starting node with public key: {}", pk.json_or());
    }
    fn secure_data_folder(&mut self) {
        if let Some(pb) = Self::secure_data_path_buf() {
            let pb_joined = pb.join(".rg");
            self.node_config.secure_data_folder = Some(DataFolder::from_path(pb_joined));
        }
    }
    fn alias(&mut self) {
        if let Ok(a) = std::env::var("REDGOLD_ALIAS") {
            if !a.trim().is_empty() {
                self.node_config.node_info.alias = Some(a.trim().to_string());
            }
        }
    }
    fn immediate_debug(&self) {
        if let Some(cmd) = &self.opts.subcmd {
            match cmd {
                RgTopLevelSubcommand::TestCapture(_t) => {
                    println!("Attempting test capture");
                    // debug_capture();
                    unsafe {
                        exit(0)
                    }
                }
                _ => {}
            }
        }
    }
}


/**
This function uses an external program for calculating checksum.
Tried doing this locally, but for some reason it seemed to have a different output than the shell script.
There's internal libraries for getting the current exe path and calculating checksum, but they
seem to produce a different result than the shell script.
*/
fn calc_sha_sum(path: String) -> RgResult<String> {
    util::cmd::run_cmd_safe("shasum", vec!["-a", "256", &*path])
        .and_then(|x|
            x.0
             .split_whitespace()
             .next()
                .ok_or(error_info("No output from shasum"))
                .map(|x| x.to_string())
        )
}

// #[tokio::test]
// async fn debug_open_database() {
//     util::init_logger().ok(); //expect("log");
//     let net_dir = get_default_data_directory(NetworkEnvironment::Local);
//     let ds_path = net_dir.as_path().clone();
//     info!(
//         "Attempting to make directory for datastore in: {:?}",
//         ds_path.clone().to_str()
//     );
//     fs::create_dir_all(ds_path).expect("Directory unable to be created.");
//     let path = ds_path
//         .join("data_store.sqlite")
//         .as_path()
//         .to_str()
//         .expect("Path format error")
//         .to_string();
//
//     let mut node_config = NodeConfig::default();
//     node_config.data_store_path = path.clone();
//     info!("Using path: {}", path);
//
//     let store = node_config.data_store().await;
//     store
//         .create_all_err_info()
//         // .await
//         .expect("Unable to create initial tables");
//
//     store.create_mnemonic().await.expect("Create mnemonic");
// }

#[test]
fn test_shasum() {
    println!("{:?}", calc_sha_sum("Cargo.toml".to_string()));
}

#[test]
fn load_ds_path() {
    let _config = NodeConfig::default();
    // let res = load_node_config_initial(args::empty_args(), config);
    // println!("{}", res.data_store_path());
}

// TODO: Settings from config if necessary
/*    let mut settings = config::Config::default();
    let mut settings2 = settings.clone();
    settings
        // Add in `./Settings.toml`
        .merge(config::File::with_name("Settings"))
        .unwrap_or(&mut settings2)
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
        .merge(config::Environment::with_prefix("REDGOLD"))
        .unwrap();
*/
// Pre logger commands
pub async fn immediate_commands(opts: &RgArgs, config: &NodeConfig,
                                // , simple_runtime: Arc<Runtime>
                                args: Vec<&String>
) -> bool {
    let mut abort = false;
    let res: Result<(), ErrorInfo> = match &opts.subcmd {
        None => {Ok(())}
        Some(c) => {
            abort = true;
            match c {
                RgTopLevelSubcommand::GenerateWords(m) => {
                    commands::generate_mnemonic(&m);
                    Ok(())
                },
                RgTopLevelSubcommand::Address(a) => {
                    commands::generate_address(a.clone(), &config).map(|_| ())
                }
                RgTopLevelSubcommand::Send(a) => {
                    commands::send(&a, &config).await
                }
                RgTopLevelSubcommand::Query(a) => {
                    commands::query(&a, &config).await
                }
                RgTopLevelSubcommand::Faucet(a) => {
                    commands::faucet(&a, &config).await
                }
                RgTopLevelSubcommand::AddServer(a) => {
                    commands::add_server(a, &config).await
                }
                RgTopLevelSubcommand::Balance(a) => {
                    commands::balance_lookup(a, &config).await
                }
                RgTopLevelSubcommand::TestTransaction(test_transaction_cli) => {
                    commands::test_transaction(&test_transaction_cli, &config).await
                }
                RgTopLevelSubcommand::Deploy(d) => {
                    commands::deploy(d, &config).await.unwrap().abort();
                    Ok(())
                }
                RgTopLevelSubcommand::TestBitcoinBalance(_b) => {
                    commands::test_btc_balance(args.get(0).unwrap(), config.network.clone()).await;
                    Ok(())
                }
                _ => {
                    abort = false;
                    Ok(())
                }
            }
        }
    };
    if res.is_err() {
        println!("{}", serde_json::to_string(&res.err().unwrap()).expect("json"));
        abort = true;
    }
    abort
}