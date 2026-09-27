//! JSON/YAML multi-document decoding for `apply`.
//!
//! Manifests are accepted by content, not by file extension. Both JSON and
//! YAML are parsed through the same YAML document stream so `-` (stdin) and
//! multi-document files behave identically.

use crate::error::CliError;
use agentkube_agents::{AgentDeploymentSpec, AgentSpec};
use agentkube_tasks::TaskSpec;
use std::io::Read;

/// Maximum buffered input size (2 MiB, matching the API default ceiling).
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
/// Required manifest API version.
pub const EXPECTED_API_VERSION: &str = "agentkube.ai/v1";

/// Authoring manifest validated against exact domain types.
#[derive(Debug, Clone, PartialEq)]
pub enum Manifest {
    /// Agent authoring document.
    Agent {
        /// Resource name from `metadata.name`.
        name: String,
        /// Namespace from `metadata.namespace` or `default`.
        namespace: String,
        /// Validated desired state.
        spec: AgentSpec,
    },
    /// Deployment authoring document.
    Deployment {
        /// Resource name.
        name: String,
        /// Namespace or `default`.
        namespace: String,
        /// Validated desired state.
        spec: AgentDeploymentSpec,
    },
    /// Task authoring document.
    Task {
        /// Resource name.
        name: String,
        /// Namespace or `default`.
        namespace: String,
        /// Validated desired state.
        spec: TaskSpec,
    },
}

impl Manifest {
    /// Returns the canonical resource kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Agent { .. } => "Agent",
            Self::Deployment { .. } => "AgentDeployment",
            Self::Task { .. } => "AgentTask",
        }
    }

    /// Returns the resource name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Agent { name, .. } | Self::Deployment { name, .. } | Self::Task { name, .. } => {
                name
            }
        }
    }

    /// Returns the namespace (always resolved, defaulting to `default`).
    #[must_use]
    pub fn namespace(&self) -> &str {
        match self {
            Self::Agent { namespace, .. }
            | Self::Deployment { namespace, .. }
            | Self::Task { namespace, .. } => namespace,
        }
    }
}

/// Loads manifests from a file path or `-` for stdin with a 2 MiB ceiling.
pub fn load_documents_from_path(path: &str) -> Result<Vec<Manifest>, CliError> {
    if path == "-" {
        return load_documents_from_reader(std::io::stdin().lock());
    }
    let file = std::fs::File::open(path)
        .map_err(|error| CliError::invalid_input(format!("cannot read file {path:?}: {error}")))?;
    load_documents_from_reader(file)
}

/// Reads a bounded stream and decodes every document it contains.
fn load_documents_from_reader<R: Read>(mut reader: R) -> Result<Vec<Manifest>, CliError> {
    let mut buffer = Vec::new();
    let mut limited = reader.by_ref().take((MAX_INPUT_BYTES as u64) + 1);
    limited
        .read_to_end(&mut buffer)
        .map_err(|error| CliError::invalid_input(format!("cannot read input: {error}")))?;
    if buffer.len() > MAX_INPUT_BYTES {
        return Err(CliError::invalid_input(format!(
            "input exceeds the {} byte ceiling",
            MAX_INPUT_BYTES
        )));
    }
    parse_documents(&buffer)
}

/// Parses raw bytes as JSON or YAML multi-document content.
pub fn parse_documents(bytes: &[u8]) -> Result<Vec<Manifest>, CliError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(CliError::invalid_input(format!(
            "input exceeds the {} byte ceiling",
            MAX_INPUT_BYTES
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CliError::invalid_input("input is not valid UTF-8"))?;
    if text.trim().is_empty() {
        return Err(CliError::invalid_input(
            "input is empty or contains no resource documents",
        ));
    }
    let mut values = Vec::new();
    let deserializer = serde_yaml::Deserializer::from_str(text);
    for document in deserializer {
        let value: serde_yaml::Value =
            serde_yaml::Value::deserialize(document).map_err(|error| {
                CliError::invalid_input(format!("cannot decode YAML/JSON document: {error}"))
            })?;
        if value.is_null() {
            continue;
        }
        let json = serde_json::to_value(&value)
            .map_err(|error| CliError::invalid_input(format!("cannot decode document: {error}")))?;
        values.push(json);
    }
    if values.is_empty() {
        return Err(CliError::invalid_input(
            "input is empty or contains no resource documents",
        ));
    }
    let mut manifests = Vec::with_capacity(values.len());
    for value in values {
        manifests.push(parse_single_document(&value)?);
    }
    Ok(manifests)
}

/// Parses one generic document value into a validated [`Manifest`].
fn parse_single_document(value: &serde_json::Value) -> Result<Manifest, CliError> {
    let object = value
        .as_object()
        .ok_or_else(|| CliError::invalid_input("each document must be a JSON/YAML object"))?;
    let api_version = object
        .get("apiVersion")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::invalid_input("each document must set apiVersion: agentkube.ai/v1")
        })?;
    if api_version != EXPECTED_API_VERSION {
        return Err(CliError::invalid_input(format!(
            "unsupported apiVersion {api_version:?}: expected {EXPECTED_API_VERSION:?}"
        )));
    }
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CliError::invalid_input("each document must set a kind"))?;
    let metadata = object.get("metadata").ok_or_else(|| {
        CliError::invalid_input(format!("{kind} document must set metadata.name"))
    })?;
    let name = metadata
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::invalid_input(format!("{kind} document must set metadata.name"))
        })?;
    if name.trim().is_empty() {
        return Err(CliError::invalid_input(format!(
            "{kind} document has an empty metadata.name"
        )));
    }
    // Validate name and namespace through domain types so malformed
    // identifiers fail client-side with exit code 2.
    if let Err(error) = agentkube_core::ResourceName::new(name) {
        return Err(CliError::invalid_input(format!(
            "invalid metadata.name {name:?}: {error}"
        )));
    }
    let namespace = metadata
        .get("namespace")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("default");
    if let Err(error) = agentkube_core::Namespace::new(namespace) {
        return Err(CliError::invalid_input(format!(
            "invalid metadata.namespace {namespace:?}: {error}"
        )));
    }
    let spec = object
        .get("spec")
        .ok_or_else(|| CliError::invalid_input(format!("{kind} document must set spec")))?;
    match kind {
        "Agent" => {
            let typed: AgentSpec = serde_json::from_value(spec.clone()).map_err(|error| {
                CliError::invalid_input(format!("invalid Agent spec for {name:?}: {error}"))
            })?;
            Ok(Manifest::Agent {
                name: name.to_owned(),
                namespace: namespace.to_owned(),
                spec: typed,
            })
        }
        "AgentDeployment" => {
            let typed: AgentDeploymentSpec =
                serde_json::from_value(spec.clone()).map_err(|error| {
                    CliError::invalid_input(format!(
                        "invalid AgentDeployment spec for {name:?}: {error}"
                    ))
                })?;
            Ok(Manifest::Deployment {
                name: name.to_owned(),
                namespace: namespace.to_owned(),
                spec: typed,
            })
        }
        "AgentTask" => {
            let typed: TaskSpec = serde_json::from_value(spec.clone()).map_err(|error| {
                CliError::invalid_input(format!("invalid AgentTask spec for {name:?}: {error}"))
            })?;
            Ok(Manifest::Task {
                name: name.to_owned(),
                namespace: namespace.to_owned(),
                spec: typed,
            })
        }
        other => Err(CliError::invalid_input(format!(
            "unsupported kind {other:?}: expected Agent, AgentDeployment, or AgentTask"
        ))),
    }
}

use serde::Deserialize;

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_YAML: &str = r#"
apiVersion: agentkube.ai/v1
kind: Agent
metadata:
  name: backend-agent
spec:
  role: developer
  model:
    strategy: fixed
    provider: openai
    model: gpt-5
  instructions: Build reliable software.
"#;

    const TASK_JSON: &str = r#"{
  "apiVersion": "agentkube.ai/v1",
  "kind": "AgentTask",
  "metadata": {"name": "fix-bug"},
  "spec": {"objective": "Fix the login bug."}
}"#;

    #[test]
    fn yaml_and_json_are_accepted_by_content() {
        let agent = parse_documents(AGENT_YAML.as_bytes()).unwrap();
        assert_eq!(agent.len(), 1);
        assert_eq!(agent[0].kind(), "Agent");

        let task = parse_documents(TASK_JSON.as_bytes()).unwrap();
        assert_eq!(task[0].kind(), "AgentTask");
    }

    #[test]
    fn multi_document_streams_decode_every_document() {
        let stream = format!("{AGENT_YAML}\n---\n{TASK_JSON}\n");
        let manifests = parse_documents(stream.as_bytes()).unwrap();

        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests[0].kind(), "Agent");
        assert_eq!(manifests[1].kind(), "AgentTask");
    }

    #[test]
    fn wire_documents_with_status_are_accepted_but_ignored() {
        let with_status = format!(
            "{}\nstatus:\n  phase: READY\n  observedVersion: 5\n",
            AGENT_YAML.trim_end()
        );
        let manifests = parse_documents(with_status.as_bytes()).unwrap();

        assert_eq!(manifests.len(), 1, "status must be non-authoritative");
    }

    #[test]
    fn unknown_versions_kinds_and_bad_specs_are_rejected() {
        for input in [
            AGENT_YAML.replace("agentkube.ai/v1", "v1"),
            AGENT_YAML.replace("kind: Agent", "kind: Starship"),
            AGENT_YAML.replace("role: developer", "role: \"INVALID ROLE!!\""),
            String::from(""),
            String::from("   \n"),
        ] {
            assert!(parse_documents(input.as_bytes()).is_err(), "{input:?}");
        }
    }

    #[test]
    fn oversized_input_is_rejected() {
        let big = vec![b'a'; MAX_INPUT_BYTES + 1];
        assert!(parse_documents(&big).is_err());
    }
}
