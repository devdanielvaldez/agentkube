use crate::{Objective, RetryPolicy, TaskRequirements};
use agentkube_core::HumanDuration;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// Scheduling priority assigned to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    /// May preempt lower-priority work when supported.
    Critical,
    /// Time-sensitive work.
    High,
    /// Default interactive work.
    #[default]
    Normal,
    /// Work that may wait behind normal demand.
    Low,
    /// Opportunistic background work.
    Background,
}

impl TaskPriority {
    /// Returns a stable ordering weight; larger values mean higher priority.
    #[must_use]
    pub const fn weight(self) -> u8 {
        match self {
            Self::Critical => 5,
            Self::High => 4,
            Self::Normal => 3,
            Self::Low => 2,
            Self::Background => 1,
        }
    }
}

/// Financial, token, and wall-clock bounds for a task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_cost_micro_usd: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<HumanDuration>,
}

impl TaskBudget {
    /// Creates a budget with no task-specific limits.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_tokens: None,
            max_cost_micro_usd: None,
            timeout: None,
        }
    }

    /// Sets the token ceiling.
    #[must_use]
    pub const fn with_max_tokens(mut self, value: NonZeroU64) -> Self {
        self.max_tokens = Some(value);
        self
    }

    /// Sets the cost ceiling in millionths of one US dollar.
    #[must_use]
    pub const fn with_max_cost_micro_usd(mut self, value: NonZeroU64) -> Self {
        self.max_cost_micro_usd = Some(value);
        self
    }

    /// Sets the wall-clock timeout.
    #[must_use]
    pub const fn with_timeout(mut self, value: HumanDuration) -> Self {
        self.timeout = Some(value);
        self
    }

    /// Returns the token ceiling.
    #[must_use]
    pub const fn max_tokens(&self) -> Option<NonZeroU64> {
        self.max_tokens
    }

    /// Returns the cost ceiling in millionths of one US dollar.
    #[must_use]
    pub const fn max_cost_micro_usd(&self) -> Option<NonZeroU64> {
        self.max_cost_micro_usd
    }

    /// Returns the wall-clock timeout.
    #[must_use]
    pub const fn timeout(&self) -> Option<HumanDuration> {
        self.timeout
    }
}

/// Immutable desired state of an agent task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSpec {
    objective: Objective,
    #[serde(default)]
    requirements: TaskRequirements,
    #[serde(default)]
    priority: TaskPriority,
    #[serde(default)]
    budget: TaskBudget,
    #[serde(default)]
    retry_policy: RetryPolicy,
}

impl TaskSpec {
    /// Creates a normal-priority task with secure default requirements.
    #[must_use]
    pub fn new(objective: Objective) -> Self {
        Self {
            objective,
            requirements: TaskRequirements::default(),
            priority: TaskPriority::Normal,
            budget: TaskBudget::default(),
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Applies scheduling requirements.
    #[must_use]
    pub fn with_requirements(mut self, requirements: TaskRequirements) -> Self {
        self.requirements = requirements;
        self
    }

    /// Applies scheduling priority.
    #[must_use]
    pub const fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Applies a task budget.
    #[must_use]
    pub fn with_budget(mut self, budget: TaskBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Applies retry behavior.
    #[must_use]
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Returns the task objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        &self.objective
    }

    /// Returns scheduling requirements.
    #[must_use]
    pub const fn requirements(&self) -> &TaskRequirements {
        &self.requirements
    }

    /// Returns scheduling priority.
    #[must_use]
    pub const fn priority(&self) -> TaskPriority {
        self.priority
    }

    /// Returns the task budget.
    #[must_use]
    pub const fn budget(&self) -> &TaskBudget {
        &self.budget
    }

    /// Returns retry behavior.
    #[must_use]
    pub const fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }
}
