use agentkube_core::ResourceVersion;
use serde::{Deserialize, Deserializer, Serialize};

/// High-level observed state of an agent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentPhase {
    /// The definition has not yet been reconciled.
    Pending,
    /// At least one usable instance is available.
    Ready,
    /// The desired state is only partially available.
    Degraded,
    /// Execution was administratively suspended.
    Suspended,
    /// The definition was terminated.
    Terminated,
}

/// Well-known condition associated with an agent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentConditionType {
    /// The definition has been accepted by the control plane.
    Accepted,
    /// Required dependencies have been resolved.
    DependenciesReady,
    /// Desired instances are being created or replaced.
    Progressing,
    /// The desired number of instances is available.
    Available,
}

/// Three-valued condition status used when observation may be incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConditionStatus {
    /// The condition is satisfied.
    True,
    /// The condition is not satisfied.
    False,
    /// The controller cannot currently determine the condition.
    Unknown,
}

/// One controller observation about an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCondition {
    condition_type: AgentConditionType,
    status: ConditionStatus,
    reason: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentStatusWire {
    phase: AgentPhase,
    observed_version: ResourceVersion,
    #[serde(default)]
    conditions: Vec<AgentCondition>,
}

impl<'de> Deserialize<'de> for AgentStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AgentStatusWire::deserialize(deserializer)?;
        let mut status = Self {
            phase: wire.phase,
            observed_version: wire.observed_version,
            conditions: Vec::with_capacity(wire.conditions.len()),
        };
        for condition in wire.conditions {
            status.set_condition(condition);
        }
        Ok(status)
    }
}

impl AgentCondition {
    /// Creates a condition. Reasons and messages are intentionally controller-defined.
    #[must_use]
    pub fn new(
        condition_type: AgentConditionType,
        status: ConditionStatus,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            condition_type,
            status,
            reason: reason.into(),
            message: message.into(),
        }
    }

    /// Returns the condition category.
    #[must_use]
    pub const fn condition_type(&self) -> AgentConditionType {
        self.condition_type
    }

    /// Returns the three-valued status.
    #[must_use]
    pub const fn status(&self) -> ConditionStatus {
        self.status
    }

    /// Returns the stable controller reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Controller-maintained observed status of an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStatus {
    phase: AgentPhase,
    observed_version: ResourceVersion,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    conditions: Vec<AgentCondition>,
}

impl AgentStatus {
    /// Creates a pending status for the supplied desired-state version.
    #[must_use]
    pub const fn pending(observed_version: ResourceVersion) -> Self {
        Self {
            phase: AgentPhase::Pending,
            observed_version,
            conditions: Vec::new(),
        }
    }

    /// Updates the high-level phase.
    pub fn set_phase(&mut self, phase: AgentPhase) {
        self.phase = phase;
    }

    /// Records the most recently reconciled desired-state version.
    pub fn set_observed_version(&mut self, version: ResourceVersion) {
        self.observed_version = version;
    }

    /// Inserts or replaces a condition of the same type.
    pub fn set_condition(&mut self, condition: AgentCondition) {
        if let Some(existing) = self
            .conditions
            .iter_mut()
            .find(|existing| existing.condition_type == condition.condition_type)
        {
            *existing = condition;
        } else {
            self.conditions.push(condition);
            self.conditions.sort_by_key(AgentCondition::condition_type);
        }
    }

    /// Returns the high-level phase.
    #[must_use]
    pub const fn phase(&self) -> AgentPhase {
        self.phase
    }

    /// Returns the most recently reconciled version.
    #[must_use]
    pub const fn observed_version(&self) -> ResourceVersion {
        self.observed_version
    }

    /// Returns deterministically ordered conditions.
    #[must_use]
    pub fn conditions(&self) -> &[AgentCondition] {
        &self.conditions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_a_condition_replaces_the_previous_observation() {
        let mut status = AgentStatus::pending(ResourceVersion::INITIAL);
        status.set_condition(AgentCondition::new(
            AgentConditionType::Available,
            ConditionStatus::False,
            "Starting",
            "Instance is starting",
        ));
        status.set_condition(AgentCondition::new(
            AgentConditionType::Available,
            ConditionStatus::True,
            "Ready",
            "Instance is ready",
        ));

        assert_eq!(status.conditions().len(), 1);
        assert_eq!(status.conditions()[0].status(), ConditionStatus::True);
        assert_eq!(status.conditions()[0].reason(), "Ready");
    }

    #[test]
    fn deserialization_normalizes_duplicate_conditions() {
        let json = r#"{
            "phase":"READY",
            "observedVersion":1,
            "conditions":[
                {"conditionType":"available","status":"FALSE","reason":"Old","message":"old"},
                {"conditionType":"available","status":"TRUE","reason":"New","message":"new"}
            ]
        }"#;
        let status: AgentStatus = serde_json::from_str(json).unwrap();

        assert_eq!(status.conditions().len(), 1);
        assert_eq!(status.conditions()[0].reason(), "New");
    }
}
