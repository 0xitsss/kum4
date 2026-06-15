use thiserror::Error;

#[derive(Error, Debug)]
pub enum Kum4Error {
    #[error("Config error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(#[from] sled::Error),

    #[error("Network error: {0}")]
    Network(String),

    #[error("RPC error: {0}")]
    #[allow(dead_code)]
    Rpc(String),

    #[error("Bitcoin error: {0}")]
    Bitcoin(String),

    #[error("Bitcoin address error: {0}")]
    BitcoinAddress(#[from] bitcoin::address::ParseError),

    #[error("Wallet error: {0}")]
    Wallet(String),

    #[error("BIP32 error: {0}")]
    Bip32(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    #[allow(dead_code)]
    Parse(String),

    #[error("Serde JSON error: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("Hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),

    #[error("Sighash error: {0}")]
    Sighash(String),

    #[error("DHT error: {0}")]
    Dht(String),

    #[error("P2P error: {0}")]
    #[allow(dead_code)]
    P2p(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Kum4Error>;

impl From<reqwest::Error> for Kum4Error {
    fn from(e: reqwest::Error) -> Self {
        Kum4Error::Network(e.to_string())
    }
}

impl From<secp256k1::Error> for Kum4Error {
    fn from(e: secp256k1::Error) -> Self {
        Kum4Error::Bitcoin(e.to_string())
    }
}

impl From<bitcoin::sighash::P2wpkhError> for Kum4Error {
    fn from(e: bitcoin::sighash::P2wpkhError) -> Self {
        Kum4Error::Sighash(e.to_string())
    }
}

impl From<bitcoin::key::UncompressedPublicKeyError> for Kum4Error {
    fn from(e: bitcoin::key::UncompressedPublicKeyError) -> Self {
        Kum4Error::Bitcoin(e.to_string())
    }
}

impl From<teloxide::RequestError> for Kum4Error {
    fn from(e: teloxide::RequestError) -> Self {
        Kum4Error::Network(e.to_string())
    }
}
