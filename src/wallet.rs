use std::str::FromStr;

use bip39::{Language, Mnemonic};
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::{Address, CompressedPublicKey, Network, PrivateKey, PublicKey};
use secp256k1::Secp256k1;
use sha3::{Digest, Keccak256};

use crate::error::{Kum4Error, Result};

#[allow(dead_code)]
const GAP_LIMIT: u32 = 20;

#[derive(Clone)]
pub struct Wallet {
    master_xprv: Xpriv,
    network: Network,
}

impl Wallet {
    pub fn from_seed_phrase(phrase: &str, network: Network) -> Result<Self> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase)
            .map_err(|e| Kum4Error::Wallet(format!("Invalid mnemonic: {e}")))?;
        let seed = mnemonic.to_seed("");
        let master_xprv = Xpriv::new_master(network, &seed)
            .map_err(|e| Kum4Error::Wallet(format!("Master key derivation: {e}")))?;
        Ok(Wallet {
            master_xprv,
            network,
        })
    }

    fn derive_path(&self, path_str: &str) -> Result<Xpriv> {
        let path = DerivationPath::from_str(path_str)
            .map_err(|e| Kum4Error::Wallet(format!("Invalid path: {e}")))?;
        let secp = Secp256k1::new();
        self.master_xprv
            .derive_priv(&secp, &path)
            .map_err(|e| Kum4Error::Bip32(e.to_string()))
    }

    pub fn btc_address_at_index(&self, index: u32) -> Result<(Address, Xpriv)> {
        let child = self.derive_path(&format!("m/84'/0'/0'/0/{index}"))?;
        let privkey = PrivateKey::new(child.private_key, self.network);
        let secp = Secp256k1::new();
        let compressed = CompressedPublicKey::from_private_key(&secp, &privkey)?;
        let address = Address::p2wpkh(&compressed, self.network);
        Ok((address, child))
    }

    pub fn btc_address(&self, index: u32) -> Result<Address> {
        Ok(self.btc_address_at_index(index)?.0)
    }

    #[allow(dead_code)]
    pub fn btc_addresses(&self) -> Result<Vec<(Address, u32)>> {
        let mut addresses = Vec::new();
        for i in 0..GAP_LIMIT {
            let addr = self.btc_address(i)?;
            addresses.push((addr, i));
        }
        Ok(addresses)
    }

    pub fn eth_address_at_index(&self, index: u32) -> Result<String> {
        let child = self.derive_path(&format!("m/44'/60'/0'/0/{index}"))?;
        let secp = Secp256k1::new();
        let pubkey = secp256k1::PublicKey::from_secret_key(&secp, &child.private_key);
        let pubkey_bytes = pubkey.serialize_uncompressed();
        let hash = Keccak256::digest(&pubkey_bytes[1..]);
        let address_bytes = &hash[12..];
        Ok(format!("0x{}", hex::encode(address_bytes)))
    }

    pub fn tron_address_at_index(&self, index: u32) -> Result<String> {
        let eth_addr = self.eth_address_at_index(index)?;
        let eth_bytes = hex::decode(&eth_addr[2..])?;
        let mut tron_bytes = vec![0x41u8];
        tron_bytes.extend_from_slice(&eth_bytes);
        let hash = sha2::Sha256::digest(&tron_bytes);
        let hash2 = sha2::Sha256::digest(hash);
        let mut buf = Vec::with_capacity(25);
        buf.extend_from_slice(&tron_bytes);
        buf.extend_from_slice(&hash2[..4]);
        Ok(bs58::encode(&buf).into_string())
    }

    #[allow(dead_code)]
    pub fn btc_private_key_at_index(&self, index: u32) -> Result<PrivateKey> {
        let (_, xprv) = self.btc_address_at_index(index)?;
        Ok(PrivateKey::new(xprv.private_key, self.network))
    }

    #[allow(dead_code)]
    pub fn btc_public_key_at_index(&self, index: u32) -> Result<PublicKey> {
        let secp = Secp256k1::new();
        let privkey = self.btc_private_key_at_index(index)?;
        Ok(privkey.public_key(&secp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SEED: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_mnemonic_to_seed() {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, TEST_SEED).unwrap();
        let seed = mnemonic.to_seed("");
        assert_eq!(seed.len(), 64);
    }

    #[test]
    fn test_btc_address_derivation() {
        let wallet = Wallet::from_seed_phrase(TEST_SEED, Network::Bitcoin).unwrap();
        let address = wallet.btc_address(0).unwrap();
        assert!(address.to_string().starts_with("bc1q"));
    }

    #[test]
    fn test_btc_addresses_gap_limit() {
        let wallet = Wallet::from_seed_phrase(TEST_SEED, Network::Bitcoin).unwrap();
        let addrs = wallet.btc_addresses().unwrap();
        assert_eq!(addrs.len(), GAP_LIMIT as usize);
    }

    #[test]
    fn test_eth_address_format() {
        let wallet = Wallet::from_seed_phrase(TEST_SEED, Network::Bitcoin).unwrap();
        let addr = wallet.eth_address_at_index(0).unwrap();
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    #[test]
    fn test_tron_address_format() {
        let wallet = Wallet::from_seed_phrase(TEST_SEED, Network::Bitcoin).unwrap();
        let addr = wallet.tron_address_at_index(0).unwrap();
        assert!(addr.starts_with("T"));
    }
}
