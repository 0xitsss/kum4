use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;
use serde::{Deserialize, Serialize};
use sled::{Db, Tree};

use crate::error::Result;

#[derive(Clone)]
pub struct Database {
    db: Arc<Db>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::open(path)?;
        Ok(Database { db: Arc::new(db) })
    }

    pub fn tree(&self, name: &str) -> Result<Tree> {
        Ok(self.db.open_tree(name)?)
    }

    pub fn tx_hashes(&self) -> Result<Tree> {
        self.tree("tx_hashes")
    }

    #[allow(dead_code)]
    pub fn utxos(&self) -> Result<Tree> {
        self.tree("utxos")
    }

    #[allow(dead_code)]
    pub fn deposits(&self) -> Result<Tree> {
        self.tree("deposits")
    }

    pub fn is_tx_processed(&self, tx_hash: &str) -> Result<bool> {
        let tree = self.tx_hashes()?;
        Ok(tree.contains_key(tx_hash.as_bytes())?)
    }

    pub fn mark_tx_processed(&self, tx_hash: &str) -> Result<()> {
        let tree = self.tx_hashes()?;
        tree.insert(tx_hash.as_bytes(), b"processed")?;
        tree.flush()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }

    pub fn addr_index(&self, chain: &str) -> Result<u32> {
        let tree = self.tree("addr_index")?;
        Ok(tree
            .get(chain.as_bytes())?
            .map(|v| {
                let mut buf = [0u8; 4];
                buf.copy_from_slice(&v);
                u32::from_le_bytes(buf)
            })
            .unwrap_or(0))
    }

    pub fn set_addr_index(&self, chain: &str, index: u32) -> Result<()> {
        let tree = self.tree("addr_index")?;
        tree.insert(chain.as_bytes(), &index.to_le_bytes())?;
        tree.flush()?;
        Ok(())
    }

    pub fn advance_addr_index(&self, chain: &str) -> Result<u32> {
        let next = self.addr_index(chain)? + 1;
        self.set_addr_index(chain, next)?;
        Ok(next)
    }

    pub fn current_address(&self, chain: &str) -> Result<Option<String>> {
        let tree = self.tree("current_address")?;
        Ok(tree
            .get(chain.as_bytes())?
            .map(|v| String::from_utf8_lossy(&v).to_string()))
    }

    pub fn set_current_address(&self, chain: &str, address: &str) -> Result<()> {
        let tree = self.tree("current_address")?;
        tree.insert(chain.as_bytes(), address.as_bytes())?;
        tree.flush()?;
        Ok(())
    }

    pub fn generate_exchange_id(&self) -> String {
        let mut bytes = [0u8; 12];
        rand::thread_rng().fill(&mut bytes);
        hex::encode(bytes)
    }

    pub fn create_exchange(
        &self,
        chain: &str,
        deposit_address: &str,
        btc_address: &str,
        usdt_amount: f64,
        btc_amount: f64,
    ) -> Result<ExchangeRequest> {
        let id = self.generate_exchange_id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req = ExchangeRequest {
            id,
            chain: chain.to_string(),
            deposit_address: deposit_address.to_string(),
            btc_address: btc_address.to_string(),
            status: "pending".into(),
            usdt_amount: Some(usdt_amount),
            btc_amount: Some(btc_amount),
            created_at: now,
            expires_at: now + 1800,
        };
        let tree = self.tree("exchanges")?;
        tree.insert(req.id.as_bytes(), serde_json::to_vec(&req)?)?;
        tree.flush()?;
        Ok(req)
    }

    pub fn get_exchange(&self, id: &str) -> Result<Option<ExchangeRequest>> {
        let tree = self.tree("exchanges")?;
        Ok(tree
            .get(id.as_bytes())?
            .map(|v| serde_json::from_slice(&v).expect("Invalid exchange data")))
    }

    pub fn find_exchange_by_address(&self, address: &str) -> Result<Option<ExchangeRequest>> {
        let tree = self.tree("exchanges")?;
        for result in tree.iter() {
            let (_, value) = result?;
            let req: ExchangeRequest =
                serde_json::from_slice(&value).expect("Invalid exchange data");
            if req.deposit_address == address && req.status == "pending" {
                return Ok(Some(req));
            }
        }
        Ok(None)
    }

    pub fn get_pending_exchanges(&self, chain: &str) -> Result<Vec<ExchangeRequest>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let tree = self.tree("exchanges")?;
        let mut result = Vec::new();
        for entry in tree.iter() {
            let (_, value) = entry?;
            let req: ExchangeRequest =
                serde_json::from_slice(&value).expect("Invalid exchange data");
            if req.chain == chain && req.status == "pending" && req.expires_at > now {
                result.push(req);
            }
        }
        Ok(result)
    }

    pub fn set_exchange_status(&self, id: &str, status: &str) -> Result<bool> {
        let tree = self.tree("exchanges")?;
        if let Some(bytes) = tree.get(id.as_bytes())? {
            let mut req: ExchangeRequest =
                serde_json::from_slice(&bytes).expect("Invalid exchange data");
            req.status = status.to_string();
            tree.insert(id.as_bytes(), serde_json::to_vec(&req)?)?;
            tree.flush()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn set_exchange_amounts(
        &self,
        id: &str,
        usdt_amount: f64,
        btc_amount: f64,
    ) -> Result<bool> {
        let tree = self.tree("exchanges")?;
        if let Some(bytes) = tree.get(id.as_bytes())? {
            let mut req: ExchangeRequest =
                serde_json::from_slice(&bytes).expect("Invalid exchange data");
            req.usdt_amount = Some(usdt_amount);
            req.btc_amount = Some(btc_amount);
            tree.insert(id.as_bytes(), serde_json::to_vec(&req)?)?;
            tree.flush()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRequest {
    pub id: String,
    pub chain: String,
    pub deposit_address: String,
    pub btc_address: String,
    pub status: String,
    pub usdt_amount: Option<f64>,
    pub btc_amount: Option<f64>,
    pub created_at: u64,
    pub expires_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addr_index_defaults_to_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(db.addr_index("tron").unwrap(), 0);
        assert_eq!(db.addr_index("bsc").unwrap(), 0);
    }

    #[test]
    fn test_set_and_read_addr_index() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().to_str().unwrap()).unwrap();
        db.set_addr_index("tron", 42).unwrap();
        assert_eq!(db.addr_index("tron").unwrap(), 42);
    }

    #[test]
    fn test_advance_addr_index() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(db.advance_addr_index("tron").unwrap(), 1);
        assert_eq!(db.addr_index("tron").unwrap(), 1);
        assert_eq!(db.advance_addr_index("tron").unwrap(), 2);
    }

    #[test]
    fn test_current_address_none_when_not_set() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().to_str().unwrap()).unwrap();
        assert!(db.current_address("tron").unwrap().is_none());
    }

    #[test]
    fn test_set_and_read_current_address() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().to_str().unwrap()).unwrap();
        db.set_current_address("tron", "TR...test").unwrap();
        assert_eq!(db.current_address("tron").unwrap().unwrap(), "TR...test");
    }
}
