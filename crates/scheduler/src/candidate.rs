use crate::{BasisPoints, CandidateError};
use agentkube_agents::{AgentDefinition, AgentInstance, ProviderName};
use agentkube_core::{HumanDuration, NodeId, Resource};
use std::collections::BTreeMap;

/// Infrastructure locality advertised by a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLocality {
    /// Provider execution remains on cluster-controlled infrastructure.
    Local,
    /// Provider execution leaves the cluster for a managed service.
    Cloud,
}

/// Provider endpoint reachable from a candidate node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAvailability {
    name: ProviderName,
    locality: NodeLocality,
    accepts_confidential: bool,
}

impl ProviderAvailability {
    /// Creates provider placement metadata.
    #[must_use]
    pub const fn new(
        name: ProviderName,
        locality: NodeLocality,
        accepts_confidential: bool,
    ) -> Self {
        Self {
            name,
            locality,
            accepts_confidential,
        }
    }

    /// Provider name.
    #[must_use]
    pub const fn name(&self) -> &ProviderName {
        &self.name
    }
    /// Execution locality.
    #[must_use]
    pub const fn locality(&self) -> NodeLocality {
        self.locality
    }
    /// Whether policy permits confidential inputs.
    #[must_use]
    pub const fn accepts_confidential(&self) -> bool {
        self.accepts_confidential
    }
}

/// Immutable node snapshot used for one scheduling cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSnapshot {
    id: NodeId,
    ready: bool,
    schedulable: bool,
    used_slots: u16,
    total_slots: u16,
    providers: BTreeMap<ProviderName, ProviderAvailability>,
}

impl NodeSnapshot {
    /// Creates a node snapshot while validating capacity and provider uniqueness.
    pub fn new(
        id: NodeId,
        ready: bool,
        schedulable: bool,
        used_slots: u16,
        total_slots: u16,
        providers: impl IntoIterator<Item = ProviderAvailability>,
    ) -> Result<Self, CandidateError> {
        if total_slots == 0 || used_slots > total_slots {
            return Err(CandidateError::InvalidCapacity {
                used: used_slots,
                total: total_slots,
            });
        }
        let mut indexed = BTreeMap::new();
        for provider in providers {
            let name = provider.name.clone();
            if indexed.insert(name.clone(), provider).is_some() {
                return Err(CandidateError::DuplicateProvider(name));
            }
        }
        Ok(Self {
            id,
            ready,
            schedulable,
            used_slots,
            total_slots,
            providers: indexed,
        })
    }

    /// Node identifier.
    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }
    /// Whether node liveness is healthy.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.ready
    }
    /// Whether new placements are allowed.
    #[must_use]
    pub const fn is_schedulable(&self) -> bool {
        self.schedulable
    }
    /// Running execution slots.
    #[must_use]
    pub const fn used_slots(&self) -> u16 {
        self.used_slots
    }
    /// Total execution slots.
    #[must_use]
    pub const fn total_slots(&self) -> u16 {
        self.total_slots
    }
    /// Remaining execution slots.
    #[must_use]
    pub const fn available_slots(&self) -> u16 {
        self.total_slots - self.used_slots
    }
    /// Providers reachable from this node, sorted by name.
    #[must_use]
    pub const fn providers(&self) -> &BTreeMap<ProviderName, ProviderAvailability> {
        &self.providers
    }
}

/// Historical estimates associated with an agent/node placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateMetrics {
    quality: BasisPoints,
    expected_latency: HumanDuration,
    expected_cost_micro_usd: u64,
}

impl CandidateMetrics {
    /// Creates scheduling estimates. Quality is expressed in basis points.
    pub fn new(
        quality_basis_points: u16,
        expected_latency: HumanDuration,
        expected_cost_micro_usd: u64,
    ) -> Result<Self, CandidateError> {
        let quality =
            BasisPoints::new(quality_basis_points).ok_or(CandidateError::MetricOutOfRange {
                field: "quality",
                value: quality_basis_points,
            })?;
        Ok(Self {
            quality,
            expected_latency,
            expected_cost_micro_usd,
        })
    }

    /// Historical outcome quality.
    #[must_use]
    pub const fn quality(self) -> BasisPoints {
        self.quality
    }
    /// Estimated end-to-end latency.
    #[must_use]
    pub const fn expected_latency(self) -> HumanDuration {
        self.expected_latency
    }
    /// Estimated execution price in millionths of one US dollar.
    #[must_use]
    pub const fn expected_cost_micro_usd(self) -> u64 {
        self.expected_cost_micro_usd
    }
}

/// Fully validated agent, definition, node, and telemetry snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingCandidate {
    instance: AgentInstance,
    definition: AgentDefinition,
    node: NodeSnapshot,
    metrics: CandidateMetrics,
}

impl SchedulingCandidate {
    /// Creates a candidate and validates its immutable identity relationships.
    pub fn new(
        instance: AgentInstance,
        definition: AgentDefinition,
        node: NodeSnapshot,
        metrics: CandidateMetrics,
    ) -> Result<Self, CandidateError> {
        let definition_uid = definition.metadata().uid();
        if instance.definition_uid() != definition_uid {
            return Err(CandidateError::DefinitionMismatch {
                expected: instance.definition_uid(),
                actual: definition_uid,
            });
        }
        if instance.node_id() != Some(node.id()) {
            return Err(CandidateError::NodeMismatch {
                agent_id: instance.id(),
                expected: instance.node_id(),
                actual: node.id(),
            });
        }
        Ok(Self {
            instance,
            definition,
            node,
            metrics,
        })
    }

    /// Agent instance snapshot.
    #[must_use]
    pub const fn instance(&self) -> &AgentInstance {
        &self.instance
    }
    /// Agent definition snapshot.
    #[must_use]
    pub const fn definition(&self) -> &AgentDefinition {
        &self.definition
    }
    /// Node snapshot.
    #[must_use]
    pub const fn node(&self) -> &NodeSnapshot {
        &self.node
    }
    /// Historical estimates.
    #[must_use]
    pub const fn metrics(&self) -> CandidateMetrics {
        self.metrics
    }
}
