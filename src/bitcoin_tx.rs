use bitcoin::consensus::Encodable;
use bitcoin::ecdsa::Signature;
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::transaction::{TxIn, TxOut, Version};
use bitcoin::{Address, Amount, CompressedPublicKey, Network, OutPoint, PrivateKey, ScriptBuf, Sequence, Transaction, Txid};
use serde::{Deserialize, Serialize};

use crate::error::{Kum4Error, Result};
use crate::price::Prices;

#[allow(dead_code)]
const MAX_RETRIES: u32 = 3;
#[allow(dead_code)]
const RETRY_DELAY_MS: u64 = 1000;

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoEntry {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub address: String,
    pub script_pubkey: String,
    pub confirmed: bool,
}

#[allow(dead_code)]
pub struct BitcoinTxBuilder {
    network: Network,
    client: reqwest::Client,
}

#[allow(dead_code)]
impl BitcoinTxBuilder {
    pub fn new(network: Network) -> Self {
        BitcoinTxBuilder { network, client: reqwest::Client::new() }
    }

    pub async fn fetch_utxos(client: &reqwest::Client, mempool_url: &str, address: &str) -> Result<Vec<UtxoEntry>> {
        let url = format!("{}/api/address/{}/utxo", mempool_url.trim_end_matches('/'), address);
        let resp = Self::retry(|| async {
            client.get(&url).send().await.map_err(|e| Kum4Error::Network(e.to_string()))
        }).await?;
        let data: Vec<serde_json::Value> = resp.json().await.map_err(|e| Kum4Error::Network(e.to_string()))?;
        let mut utxos = Vec::new();
        for item in data {
            let txid = item["txid"].as_str().unwrap_or("").to_string();
            let vout = item["vout"].as_u64().unwrap_or(0) as u32;
            let value = item["value"].as_u64().unwrap_or(0);
            let status = &item["status"];
            if txid.is_empty() || value == 0 { continue; }
            utxos.push(UtxoEntry {
                txid,
                vout,
                value,
                address: address.to_string(),
                script_pubkey: "".into(),
                confirmed: status["confirmed"].as_bool().unwrap_or(false),
            });
        }
        utxos.sort_by_key(|u| std::cmp::Reverse(u.value));
        Ok(utxos)
    }

    pub fn estimate_tx_vbytes(input_count: usize, output_count: usize) -> u64 {
        let base_weight = 4 * (8 + 1 + output_count * 34 + input_count * (1 + 36));
        let witness_weight = input_count * (1 + 73 + 1 + 33);
        (base_weight + witness_weight).div_ceil(4) as u64
    }

    pub fn calculate_payout(
        usdt_amount: f64,
        profit_fee_usd: f64,
        prices: &Prices,
        input_count: usize,
        output_count: usize,
    ) -> u64 {
        let tx_vbytes = Self::estimate_tx_vbytes(input_count, output_count);
        let btc_gas_usd = (prices.fee_rate_sat_per_vb * tx_vbytes as f64) * prices.btc_usd / 100_000_000.0;
        let usdt_net = usdt_amount - profit_fee_usd - btc_gas_usd;
        if usdt_net <= 0.0 {
            return 0;
        }
        ((usdt_net / prices.btc_usd) * 100_000_000.0) as u64
    }

    pub fn select_utxos(utxos: &[UtxoEntry], target_sats: u64) -> (Vec<UtxoEntry>, u64) {
        let mut selected = Vec::new();
        let mut total = 0u64;
        let mut sorted = utxos.to_vec();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.value));
        for utxo in &sorted {
            if total >= target_sats {
                break;
            }
            selected.push(utxo.clone());
            total += utxo.value;
        }
        (selected, total)
    }

    pub fn build_unsigned_tx(
        selected_utxos: &[UtxoEntry],
        merchant_address: &Address,
        payout_sats: u64,
        fee_sats: u64,
    ) -> Result<Transaction> {
        let mut inputs = Vec::new();
        let mut total_input = 0u64;
        for utxo in selected_utxos {
            let txid: Txid = utxo.txid.parse()
                .map_err(|e| Kum4Error::Parse(format!("Invalid txid {}: {e}", utxo.txid)))?;
            inputs.push(TxIn {
                previous_output: OutPoint { txid, vout: utxo.vout },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::from_consensus(0xfffffffd),
                witness: bitcoin::Witness::new(),
            });
            total_input += utxo.value;
        }

        let mut outputs = Vec::new();
        outputs.push(TxOut {
            value: Amount::from_sat(payout_sats),
            script_pubkey: merchant_address.script_pubkey(),
        });

        let change = total_input - payout_sats - fee_sats;
        if change > 0 {
            outputs.push(TxOut {
                value: Amount::from_sat(change),
                script_pubkey: merchant_address.script_pubkey(),
            });
        }

        Ok(Transaction {
            version: Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: inputs,
            output: outputs,
        })
    }

    pub fn sign_p2wpkh(
        tx: &mut Transaction,
        selected_utxos: &[UtxoEntry],
        priv_key: &PrivateKey,
        pub_key: &CompressedPublicKey,
    ) -> Result<()> {
        let secp = secp256k1::Secp256k1::new();
        let wpubkey_hash = pub_key.wpubkey_hash();
        let script_code = ScriptBuf::p2wpkh_script_code(wpubkey_hash);
        let mut cache = SighashCache::new(&mut *tx);

        for (i, utxo) in selected_utxos.iter().enumerate() {
            let sighash = cache.p2wpkh_signature_hash(
                i,
                &script_code,
                Amount::from_sat(utxo.value),
                EcdsaSighashType::All,
            )?;
            let msg = secp256k1::Message::from_digest_slice(&sighash[..])
                .map_err(|e| Kum4Error::Bitcoin(format!("Message creation: {e}")))?;
            let sig = secp.sign_ecdsa(&msg, &priv_key.inner);
            let ecdsa_sig = Signature { signature: sig, sighash_type: EcdsaSighashType::All };
            let witness = bitcoin::Witness::p2wpkh(&ecdsa_sig, &pub_key.0);
            *cache.witness_mut(i).ok_or_else(|| Kum4Error::Bitcoin("Missing witness slot".into()))? = witness;
        }
        Ok(())
    }

    pub fn tx_vsize(tx: &Transaction) -> u64 {
        let mut buf = Vec::new();
        tx.consensus_encode(&mut buf).unwrap();
        buf.len() as u64
    }

    pub async fn broadcast_tx(&self, tx_hex: String, mempool_url: &str) -> Result<String> {
        let url = format!("{}/api/tx", mempool_url.trim_end_matches('/'));
        let resp = self.client
            .post(&url)
            .header("Content-Type", "text/plain")
            .body(tx_hex)
            .send()
            .await?;
        let txid = resp.text().await?;
        if txid.len() < 10 {
            return Err(Kum4Error::Network(format!("Broadcast failed: {txid}")));
        }
        Ok(txid.trim().to_string())
    }

    pub async fn broadcast_tx_with_client(
        client: &reqwest::Client,
        mempool_url: &str,
        tx_hex: String,
    ) -> Result<String> {
        let url = format!("{}/api/tx", mempool_url.trim_end_matches('/'));
        let resp = client
            .post(&url)
            .header("Content-Type", "text/plain")
            .body(tx_hex)
            .send()
            .await?;
        let txid = resp.text().await?;
        if txid.len() < 10 {
            return Err(Kum4Error::Network(format!("Broadcast failed: {txid}")));
        }
        Ok(txid.trim().to_string())
    }

    async fn retry<F, Fut, T>(f: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            match f().await {
                Ok(val) => return Ok(val),
                Err(e) => {
                    tracing::warn!("Attempt {}/{} failed: {e}", attempt + 1, MAX_RETRIES);
                    last_err = Some(e);
                    if attempt + 1 < MAX_RETRIES {
                        tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Kum4Error::Network("Retry exhausted".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tx_vbytes() {
        let vbytes = BitcoinTxBuilder::estimate_tx_vbytes(1, 2);
        assert!(vbytes > 100);
        assert!(vbytes < 200);
    }

    #[test]
    fn test_select_utxos() {
        let utxos = vec![
            UtxoEntry { txid: "a".into(), vout: 0, value: 10_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
            UtxoEntry { txid: "b".into(), vout: 0, value: 50_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
            UtxoEntry { txid: "c".into(), vout: 0, value: 100_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
        ];
        let (selected, total) = BitcoinTxBuilder::select_utxos(&utxos, 75_000);
        assert_eq!(selected.len(), 1);
        assert_eq!(total, 100_000);
    }

    #[test]
    fn test_calculate_payout() {
        let prices = Prices { btc_usd: 100_000.0, fee_rate_sat_per_vb: 50.0 };
        let payout = BitcoinTxBuilder::calculate_payout(100.0, 1.0, &prices, 1, 2);
        assert!(payout > 0);
        assert!(payout < 1_000_000);
    }

    #[test]
    fn test_select_utxos_multiple() {
        let utxos = vec![
            UtxoEntry { txid: "a".into(), vout: 0, value: 10_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
            UtxoEntry { txid: "b".into(), vout: 0, value: 5_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
            UtxoEntry { txid: "c".into(), vout: 0, value: 3_000, address: "".into(), script_pubkey: "".into(), confirmed: true },
        ];
        let (selected, total) = BitcoinTxBuilder::select_utxos(&utxos, 12_000);
        assert_eq!(selected.len(), 2);
        assert_eq!(total, 15_000);
    }
}
