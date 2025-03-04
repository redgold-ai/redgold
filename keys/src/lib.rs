use std::str::FromStr;
use bdk::bitcoin::secp256k1::Secp256k1;
use redgold_schema::{ErrorInfoContext, RgResult, SafeBytesAccess, structs};
use redgold_schema::structs::{Address, Hash};
use crate::util::{dhash_str, dhash_vec};
use crate::util::keys::ToPublicKeyFromLib;
use crate::util::mnemonic_support::WordsPass;

pub mod proof_support;
pub mod request_support;
pub mod transaction_support;
pub mod util;
pub mod debug;
pub mod xpub_wrapper;
pub mod address_external;
pub mod eth;
pub mod address_support;


pub struct TestConstants {
    pub secret: bdk::bitcoin::secp256k1::SecretKey,
    pub public: bdk::bitcoin::secp256k1::PublicKey,
    pub secret2: bdk::bitcoin::secp256k1::SecretKey,
    pub public2: bdk::bitcoin::secp256k1::PublicKey,
    pub hash_vec: Vec<u8>,
    pub addr: Vec<u8>,
    pub addr2: Vec<u8>,
    pub address_1: Address,
    pub rhash_1: Hash,
    pub rhash_2: Hash,
    pub words: String,
    pub words_pass: WordsPass,
}

impl TestConstants {
    pub fn key_pair(&self) -> KeyPair {
        KeyPair {
            secret_key: self.secret,
            public_key: self.public,
        }
    }

    pub fn new() -> TestConstants {
        let result = WordsPass::from_str_hashed("test_constants");
        let kp_default = result.default_kp().expect("");
        let (secret, public) = (kp_default.secret_key, kp_default.public_key);
        let kp2 = result.keypair_at_change(1).expect("");
        let (secret2, public2) = (kp2.secret_key, kp2.public_key);
        let hash_vec = Hash::from_string_calculate("asdf1").vec();
        let addr = Address::from_struct_public(&public.to_struct_public_key()).expect("").address.safe_bytes().expect("");
        let addr2 = Address::from_struct_public(&public2.to_struct_public_key()).expect("").address.safe_bytes().expect("");
        let mut peer_trusts: Vec<f64> = Vec::new();

        let public_peer_id = dhash_vec(&dhash_vec(&public.serialize().to_vec()).to_vec()).to_vec();

        return TestConstants {
            secret,
            public,
            secret2,
            public2,
            hash_vec,
            addr: addr.clone(),
            addr2,
            address_1: addr.into(),
            rhash_1: Hash::from_string_calculate("asdf"),
            rhash_2: Hash::from_string_calculate("asdf2"),
            words: "abuse lock pledge crowd pair become ridge alone target viable black plate ripple sad tape victory blood river gloom air crash invite volcano release".to_string(),
            words_pass: result
        };
    }
}

#[derive(Clone, Copy)]
pub struct KeyPair {
    pub secret_key: bdk::bitcoin::secp256k1::SecretKey,
    pub public_key: bdk::bitcoin::secp256k1::PublicKey,
}

impl KeyPair {
    pub fn new(
        secret_key: &bdk::bitcoin::secp256k1::SecretKey,
        public_key: &bdk::bitcoin::secp256k1::PublicKey,
    ) -> Self {
        return Self {
            secret_key: *secret_key,
            public_key: *public_key,
        };
    }

    pub fn address(&self) -> Vec<u8> {
        Address::from_struct_public(&self.public_key.to_struct_public_key())
            .expect("").address.safe_bytes().expect("")
    }

    pub fn address_typed(&self) -> Address {
        self.public_key.to_struct_public_key().address().expect("")
    }

    pub fn public_key_vec(&self) -> Vec<u8> {
        self.public_key.serialize().to_vec()
    }

    pub fn public_key(&self) -> structs::PublicKey {
        self.public_key.to_struct_public_key()
    }

    pub fn from_private_hex(hex: String) -> RgResult<Self> {
        let secret_key = bdk::bitcoin::secp256k1::SecretKey::from_str(&*hex)
            .error_info("Unable to parse private key hex")?;
        let public_key = bdk::bitcoin::secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), &secret_key);
        return Ok(Self {
            secret_key,
            public_key,
        });
    }
}
