//! Live worker-node registry backing `GET /v1/nodes`.
//!
//! The operator records one [`NodeInfo`] per embedded worker heartbeat. The
//! registry is infallible by construction: heartbeats are a side channel and
//! must never fail task execution.

use agentkube_core::NodeId;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, num::NonZeroU16, time::SystemTime};

/// One worker node as observed by its most recent heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    node_id: NodeId,
    last_heartbeat_secs: u64,
    active_executions: u16,
    capacity: u16,
}

impl NodeInfo {
    /// Records a heartbeat observation for a worker node.
    #[must_use]
    pub fn new(node_id: NodeId, active_executions: u16, capacity: NonZeroU16) -> Self {
        Self {
            node_id,
            last_heartbeat_secs: unix_now(),
            active_executions,
            capacity: capacity.get(),
        }
    }

    /// Returns the worker node identifier.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// Returns the heartbeat time as Unix seconds.
    #[must_use]
    pub const fn last_heartbeat_secs(&self) -> u64 {
        self.last_heartbeat_secs
    }

    /// Returns executions in flight at heartbeat time.
    #[must_use]
    pub const fn active_executions(&self) -> u16 {
        self.active_executions
    }

    /// Returns the worker execution capacity.
    #[must_use]
    pub const fn capacity(&self) -> u16 {
        self.capacity
    }
}

/// Thread-safe node registry shared between the operator and HTTP handlers.
#[derive(Debug, Clone, Default)]
pub struct NodeRegistry {
    inner: std::sync::Arc<tokio::sync::RwLock<HashMap<NodeId, NodeInfo>>>,
}

impl NodeRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a heartbeat, replacing the previous entry for the node.
    pub async fn record_heartbeat(&self, info: NodeInfo) {
        self.inner.write().await.insert(info.node_id(), info);
    }

    /// Lists known nodes in deterministic identifier order.
    pub async fn list(&self) -> Vec<NodeInfo> {
        let mut nodes: Vec<NodeInfo> = self.inner.read().await.values().cloned().collect();
        nodes.sort_by_key(|node| node.node_id().to_string());
        nodes
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn heartbeats_replace_prior_entries_in_stable_order() {
        let registry = NodeRegistry::new();
        assert!(registry.list().await.is_empty());

        let first = NodeId::new();
        registry
            .record_heartbeat(NodeInfo::new(first, 1, NonZeroU16::new(4).unwrap()))
            .await;
        registry
            .record_heartbeat(NodeInfo::new(first, 2, NonZeroU16::new(4).unwrap()))
            .await;

        let nodes = registry.list().await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].active_executions(), 2);
    }
}
