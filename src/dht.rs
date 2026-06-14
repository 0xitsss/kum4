use futures::StreamExt;
use libp2p::{
    identity::{ed25519, Keypair},
    kad::{self, store::MemoryStore, Behaviour as KadBehaviour},
    swarm::SwarmEvent,
    Multiaddr, PeerId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::error::{Kum4Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub peer_id: String,
    pub http_addr: String,
    pub fee_usd: f64,
    pub chains: Vec<String>,
    pub btc_reserve: f64,
    pub status: String,
}

#[allow(dead_code)]
impl NodeInfo {
    pub fn to_record(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_record(data: &[u8]) -> Option<Self> {
        serde_json::from_slice(data).ok()
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum DhtCmd {
    Bootstrap,
    Announce {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Lookup {
        key: Vec<u8>,
    },
}

#[derive(Debug)]
pub enum DhtEvent {
    Ready {
        peer_id: PeerId,
        listen_addrs: Vec<Multiaddr>,
    },
    ValueFound {
        #[allow(dead_code)]
        key: Vec<u8>,
        value: Option<Vec<u8>>,
    },
    Bootstrapped {
        num_peers: usize,
    },
    Announced,
    #[allow(dead_code)]
    Error(String),
}

pub struct DhtNode {
    swarm: libp2p::Swarm<KadBehaviour<MemoryStore>>,
    cmd_rx: mpsc::Receiver<DhtCmd>,
    event_tx: mpsc::Sender<DhtEvent>,
    peer_id: PeerId,
}

impl DhtNode {
    pub fn new(
        seed: &str,
        listen_addr: Multiaddr,
        cmd_rx: mpsc::Receiver<DhtCmd>,
        event_tx: mpsc::Sender<DhtEvent>,
    ) -> Result<Self> {
        let keypair = keypair_from_seed(seed)?;
        let peer_id = keypair.public().to_peer_id();

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                libp2p::tcp::Config::default(),
                libp2p::noise::Config::new,
                libp2p::yamux::Config::default,
            )
            .map_err(|e| Kum4Error::Dht(e.to_string()))?
            .with_behaviour(|key| {
                KadBehaviour::new(
                    key.public().to_peer_id(),
                    MemoryStore::new(key.public().to_peer_id()),
                )
            })
            .map_err(|e| Kum4Error::Dht(e.to_string()))?
            .build();

        swarm
            .listen_on(listen_addr)
            .map_err(|e| Kum4Error::Dht(format!("listen_on failed: {e}")))?;

        Ok(DhtNode { swarm, cmd_rx, event_tx, peer_id })
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    #[allow(dead_code)]
    pub fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.swarm.listeners().cloned().collect()
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                Some(event) = self.swarm.next() => {
                    self.handle_event(event).await
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_cmd(cmd)
                }
                else => break,
            }
        }
    }

    async fn handle_event(&mut self, event: SwarmEvent<kad::Event>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                tracing::info!("DHT listening: {address}");
                let _ = self
                    .event_tx
                    .send(DhtEvent::Ready {
                        peer_id: self.peer_id,
                        listen_addrs: vec![address],
                    })
                    .await;
            }
            SwarmEvent::Behaviour(kad_event) => {
                self.handle_kad(kad_event).await;
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                tracing::debug!("DHT connected: {peer_id}");
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                tracing::debug!("DHT disconnected: {peer_id}");
            }
            _ => {}
        }
    }

    async fn handle_kad(&mut self, event: kad::Event) {
        if let kad::Event::OutboundQueryProgressed { result, .. } = event { match result {
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(pr))) => {
                tracing::info!("DHT record found");
                let _ = self
                    .event_tx
                    .send(DhtEvent::ValueFound {
                        key: pr.record.key.as_ref().to_vec(),
                        value: Some(pr.record.value),
                    })
                    .await;
            }
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FinishedWithNoAdditionalRecord { .. })) => {
                tracing::info!("DHT record lookup finished, no more records");
                let _ = self
                    .event_tx
                    .send(DhtEvent::ValueFound {
                        key: vec![],
                        value: None,
                    })
                    .await;
            }
            kad::QueryResult::GetRecord(Err(e)) => {
                tracing::warn!("DHT GetRecord error: {e}");
            }
            kad::QueryResult::PutRecord(Ok(_)) => {
                tracing::info!("DHT record stored");
                let _ = self.event_tx.send(DhtEvent::Announced).await;
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                tracing::warn!("DHT PutRecord error: {e}");
            }
            kad::QueryResult::Bootstrap(Ok(report)) => {
                tracing::info!(
                    "DHT bootstrapped via {}, {} remaining",
                    report.peer,
                    report.num_remaining
                );
                let _ = self
                    .event_tx
                    .send(DhtEvent::Bootstrapped {
                        num_peers: report.num_remaining as usize,
                    })
                    .await;
            }
            kad::QueryResult::Bootstrap(Err(e)) => {
                tracing::warn!("DHT bootstrap error: {e}");
            }
            _ => {}
        } }
    }

    fn handle_cmd(&mut self, cmd: DhtCmd) {
        match cmd {
            DhtCmd::Bootstrap => {
                let _ = self.swarm.behaviour_mut().bootstrap();
            }
            DhtCmd::Announce { key, value } => {
                let record = kad::Record {
                    key: kad::RecordKey::new(&key),
                    value,
                    publisher: None,
                    expires: None,
                };
                let _ = self.swarm.behaviour_mut().put_record(record, kad::Quorum::One);
            }
            DhtCmd::Lookup { key } => {
                let _ = self.swarm.behaviour_mut().get_record(kad::RecordKey::new(&key));
            }
        }
    }
}

fn keypair_from_seed(seed: &str) -> Result<Keypair> {
    let hash = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&hash);
    let secret = ed25519::SecretKey::try_from_bytes(&mut bytes)
        .map_err(|e| Kum4Error::Dht(e.to_string()))?;
    let keypair = ed25519::Keypair::from(secret);
    Ok(Keypair::from(keypair))
}
