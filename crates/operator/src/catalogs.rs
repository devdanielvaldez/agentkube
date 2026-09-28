//! Model catalogs assembled from configuration, discovery, and tables.
//!
//! Catalogs are explicit by design: the operator never guesses model
//! capabilities. Well-known tables below record vendor-published context
//! windows (September 2026 snapshot); JSON configuration entries override
//! them per name. Ollama models discovered on the server but absent from both
//! sources receive conservative fallback capabilities and a startup warning,
//! so unknown local models stay usable without ever overclaiming.

use crate::OperatorError;
use agentkube_agents::ModelName;
use agentkube_config::{ProviderModelEntry, ProvidersConfig};
use agentkube_providers::ModelCapabilities;
use std::num::NonZeroU32;

/// Assembled per-provider catalogs ready for adapter construction.
pub struct CatalogSet {
    /// Ollama models with capabilities.
    pub ollama: Vec<(ModelName, ModelCapabilities)>,
    /// OpenAI models with capabilities.
    pub openai: Vec<(ModelName, ModelCapabilities)>,
    /// Anthropic models with capabilities.
    pub anthropic: Vec<(ModelName, ModelCapabilities)>,
    /// Gemini models with capabilities.
    pub gemini: Vec<(ModelName, ModelCapabilities)>,
}

/// Discovers Ollama model names via `/api/tags`.
///
/// Returns discovered names with an optional warning. Discovery failures
/// degrade to a warning (the server may start later); unknown names never
/// abort the boot.
pub async fn discover_ollama_models(base_url: &str) -> (Vec<String>, Option<String>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return (
                Vec::new(),
                Some(format!("ollama discovery unavailable: {error}")),
            );
        }
    };
    let response = match client
        .get(format!("{}/api/tags", base_url.trim_end_matches('/')))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (
                Vec::new(),
                Some(format!("ollama discovery failed: {error}")),
            );
        }
    };
    if !response.status().is_success() {
        return (
            Vec::new(),
            Some(format!(
                "ollama discovery returned HTTP {}",
                response.status().as_u16()
            )),
        );
    }
    let body: serde_json::Value = match response.json().await {
        Ok(body) => body,
        Err(error) => {
            return (
                Vec::new(),
                Some(format!("ollama discovery returned invalid JSON: {error}")),
            );
        }
    };
    let mut names = Vec::new();
    if let Some(models) = body.get("models").and_then(serde_json::Value::as_array) {
        for model in models {
            if let Some(name) = model.get("name").and_then(serde_json::Value::as_str) {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names.dedup();
    (names, None)
}

/// Assembles catalogs from configuration, well-known tables, and discovery.
pub fn build_catalogs(
    config: &ProvidersConfig,
    discovered_ollama: &[String],
) -> Result<(CatalogSet, Vec<String>), OperatorError> {
    let mut warnings = Vec::new();

    let mut ollama: Vec<(ModelName, ModelCapabilities)> = Vec::new();
    if config.ollama_enabled() {
        for (name, context, max_output) in well_known_ollama() {
            ollama.push((model_name(name)?, ollama_caps(context, max_output, true)?));
        }
        for entry in config.ollama_models() {
            replace_model(&mut ollama, entry)?;
        }
        for name in discovered_ollama {
            for key in discovery_keys(name) {
                if ollama.iter().any(|(existing, _)| existing.as_str() == key) {
                    continue;
                }
                let Ok(model) = ModelName::new(key.clone()) else {
                    warnings.push(format!("ignoring undiscoverable ollama model name {key:?}"));
                    continue;
                };
                ollama.push((model, fallback_caps()?));
                warnings.push(format!(
                    "ollama model {key:?} uses fallback capabilities; add a catalog entry for exact specs"
                ));
            }
        }
    }

    let mut openai: Vec<(ModelName, ModelCapabilities)> = Vec::new();
    if config.openai_api_key().is_some() {
        for (name, context, max_output, input_price, output_price) in well_known_openai() {
            openai.push((
                model_name(name)?,
                openai_caps(context, max_output, input_price, output_price)?,
            ));
        }
        for entry in config.openai_models() {
            replace_model(&mut openai, entry)?;
        }
    }

    let mut anthropic: Vec<(ModelName, ModelCapabilities)> = Vec::new();
    if config.anthropic_api_key().is_some() {
        for (name, context, max_output, input_price, output_price) in well_known_anthropic() {
            anthropic.push((
                model_name(name)?,
                hosted_caps(context, max_output, input_price, output_price)?,
            ));
        }
        for entry in config.anthropic_models() {
            replace_model(&mut anthropic, entry)?;
        }
    }

    let mut gemini: Vec<(ModelName, ModelCapabilities)> = Vec::new();
    if config.gemini_api_key().is_some() {
        for (name, context, max_output, input_price, output_price) in well_known_gemini() {
            gemini.push((
                model_name(name)?,
                hosted_caps(context, max_output, input_price, output_price)?,
            ));
        }
        for entry in config.gemini_models() {
            replace_model(&mut gemini, entry)?;
        }
    }

    ollama.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    openai.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    anthropic.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    gemini.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok((
        CatalogSet {
            ollama,
            openai,
            anthropic,
            gemini,
        },
        warnings,
    ))
}

/// Strips an Ollama `:tag` suffix for catalog keying, keeping the full name
/// as well so manifests may use either form.
fn discovery_keys(name: &str) -> Vec<String> {
    let mut keys = vec![name.to_owned()];
    if let Some((base, _)) = name.rsplit_once(':')
        && !base.is_empty()
        && base != name
    {
        keys.push(base.to_owned());
    }
    keys
}

fn replace_model(
    catalog: &mut Vec<(ModelName, ModelCapabilities)>,
    entry: &ProviderModelEntry,
) -> Result<(), OperatorError> {
    let name = ModelName::new(entry.name())
        .map_err(|_| OperatorError::Config(format!("invalid model name {:?}", entry.name())))?;
    let capabilities = ModelCapabilities::new(
        entry.context_window_tokens(),
        entry.max_output_tokens(),
        entry.supports_tools(),
        entry.supports_streaming(),
        entry.input_price(),
        entry.output_price(),
    );
    if let Some(slot) = catalog.iter_mut().find(|(existing, _)| existing == &name) {
        *slot = (name, capabilities);
    } else {
        catalog.push((name, capabilities));
    }
    Ok(())
}

fn model_name(name: &str) -> Result<ModelName, OperatorError> {
    ModelName::new(name)
        .map_err(|_| OperatorError::Config(format!("invalid static model name {name:?}")))
}

fn checked(value: u32, what: &str) -> Result<NonZeroU32, OperatorError> {
    NonZeroU32::new(value).ok_or_else(|| OperatorError::Config(format!("invalid static {what}")))
}

fn ollama_caps(
    context: u32,
    max_output: u32,
    supports_tools: bool,
) -> Result<ModelCapabilities, OperatorError> {
    Ok(ModelCapabilities::new(
        checked(context, "ollama context window")?,
        checked(max_output, "ollama output ceiling")?,
        supports_tools,
        true,
        0,
        0,
    ))
}

fn openai_caps(
    context: u32,
    max_output: u32,
    input_price: u64,
    output_price: u64,
) -> Result<ModelCapabilities, OperatorError> {
    hosted_caps(context, max_output, input_price, output_price)
}

/// Hosted-model capabilities shared by OpenAI, Anthropic, and Gemini tables.
fn hosted_caps(
    context: u32,
    max_output: u32,
    input_price: u64,
    output_price: u64,
) -> Result<ModelCapabilities, OperatorError> {
    Ok(ModelCapabilities::new(
        checked(context, "hosted context window")?,
        checked(max_output, "hosted output ceiling")?,
        true,
        true,
        input_price,
        output_price,
    ))
}

fn fallback_caps() -> Result<ModelCapabilities, OperatorError> {
    ollama_caps(8_192, 2_048, false)
}

/// Vendor-published Ollama context windows (September 2026 snapshot).
fn well_known_ollama() -> Vec<(&'static str, u32, u32)> {
    vec![
        ("llama3.1", 131_072, 4_096),
        ("qwen2.5-coder", 32_768, 4_096),
        ("mistral", 32_768, 4_096),
        ("phi3", 131_072, 4_096),
        ("gemma2", 8_192, 4_096),
    ]
}

/// Vendor-published OpenAI specs with list prices in micro-USD per million
/// tokens (September 2026 snapshot; override via configuration entries).
fn well_known_openai() -> Vec<(&'static str, u32, u32, u64, u64)> {
    vec![
        ("gpt-4o", 128_000, 16_384, 2_500_000, 10_000_000),
        ("gpt-4o-mini", 128_000, 16_384, 150_000, 600_000),
    ]
}

/// Vendor-published Anthropic specs with list prices in micro-USD per million
/// tokens (September 2026 snapshot; override via configuration entries).
/// Context windows have held at 200K across Sonnet generations.
fn well_known_anthropic() -> Vec<(&'static str, u32, u32, u64, u64)> {
    vec![
        ("claude-sonnet-4-5", 200_000, 64_000, 3_000_000, 15_000_000),
        ("claude-haiku-4-5", 200_000, 64_000, 1_000_000, 5_000_000),
    ]
}

/// Vendor-published Gemini specs with list prices in micro-USD per million
/// tokens (September 2026 snapshot; override via configuration entries).
fn well_known_gemini() -> Vec<(&'static str, u32, u32, u64, u64)> {
    vec![
        ("gemini-2.5-flash", 1_048_576, 65_536, 300_000, 2_500_000),
        ("gemini-2.0-flash", 1_048_576, 8_192, 100_000, 400_000),
    ]
}
