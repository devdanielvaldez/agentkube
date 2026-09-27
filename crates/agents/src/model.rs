use crate::{ModelName, ProviderName};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeSet, error::Error, fmt};

/// Objective considered by automatic model routing, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OptimizationObjective {
    /// Prefer historically stronger task outcomes.
    Quality,
    /// Prefer lower financial cost.
    Cost,
    /// Prefer lower end-to-end latency.
    Latency,
    /// Prefer providers satisfying stricter data-locality requirements.
    Privacy,
}

/// Policy used to select a model for agent execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "strategy", rename_all = "camelCase")]
pub enum ModelPolicy {
    /// Always use one provider-specific model.
    Fixed {
        /// Provider adapter name.
        provider: ProviderName,
        /// Provider-specific model identifier.
        model: ModelName,
    },
    /// Let the model router select a compatible model.
    Auto {
        /// Empty means every healthy provider is eligible.
        #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
        allowed_providers: BTreeSet<ProviderName>,
        /// Ordered routing objectives, from highest to lowest priority.
        optimize_for: Vec<OptimizationObjective>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "strategy", rename_all = "camelCase")]
enum ModelPolicyWire {
    Fixed {
        provider: ProviderName,
        model: ModelName,
    },
    Auto {
        #[serde(default)]
        allowed_providers: BTreeSet<ProviderName>,
        optimize_for: Vec<OptimizationObjective>,
    },
}

impl<'de> Deserialize<'de> for ModelPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ModelPolicyWire::deserialize(deserializer)? {
            ModelPolicyWire::Fixed { provider, model } => Ok(Self::fixed(provider, model)),
            ModelPolicyWire::Auto {
                allowed_providers,
                optimize_for,
            } => Self::automatic(allowed_providers, optimize_for).map_err(de::Error::custom),
        }
    }
}

impl ModelPolicy {
    /// Creates a fixed provider/model policy.
    #[must_use]
    pub const fn fixed(provider: ProviderName, model: ModelName) -> Self {
        Self::Fixed { provider, model }
    }

    /// Creates a validated automatic routing policy.
    pub fn automatic(
        allowed_providers: impl IntoIterator<Item = ProviderName>,
        optimize_for: impl IntoIterator<Item = OptimizationObjective>,
    ) -> Result<Self, ModelPolicyError> {
        let allowed_providers = allowed_providers.into_iter().collect();
        let optimize_for: Vec<_> = optimize_for.into_iter().collect();
        if optimize_for.is_empty() {
            return Err(ModelPolicyError::MissingObjective);
        }
        let unique: BTreeSet<_> = optimize_for.iter().copied().collect();
        if unique.len() != optimize_for.len() {
            return Err(ModelPolicyError::DuplicateObjective);
        }
        Ok(Self::Auto {
            allowed_providers,
            optimize_for,
        })
    }
}

/// Invalid automatic model-routing policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPolicyError {
    /// At least one routing objective is required.
    MissingObjective,
    /// Each routing objective may appear only once.
    DuplicateObjective,
}

impl fmt::Display for ModelPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingObjective => {
                formatter.write_str("automatic model policy needs an objective")
            }
            Self::DuplicateObjective => {
                formatter.write_str("automatic model policy contains a duplicate objective")
            }
        }
    }
}

impl Error for ModelPolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_policy_preserves_objective_priority() {
        let policy = ModelPolicy::automatic(
            [ProviderName::new("openai").unwrap()],
            [OptimizationObjective::Quality, OptimizationObjective::Cost],
        )
        .unwrap();
        let value = serde_json::to_value(policy).unwrap();

        assert_eq!(value["strategy"], "auto");
        assert_eq!(
            value["optimize_for"],
            serde_json::json!(["quality", "cost"])
        );
    }

    #[test]
    fn duplicate_objectives_are_rejected() {
        let result = ModelPolicy::automatic(
            [],
            [OptimizationObjective::Cost, OptimizationObjective::Cost],
        );
        assert_eq!(result, Err(ModelPolicyError::DuplicateObjective));
    }

    #[test]
    fn deserialization_cannot_bypass_auto_policy_validation() {
        let empty = r#"{"strategy":"auto","optimize_for":[]}"#;
        let duplicate = r#"{"strategy":"auto","optimize_for":["cost","cost"]}"#;

        assert!(serde_json::from_str::<ModelPolicy>(empty).is_err());
        assert!(serde_json::from_str::<ModelPolicy>(duplicate).is_err());
    }
}
