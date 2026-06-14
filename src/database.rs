use std::sync::Arc;

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

    pub fn utxos(&self) -> Result<Tree> {
        self.tree("utxos")
    }

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

    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }
}
