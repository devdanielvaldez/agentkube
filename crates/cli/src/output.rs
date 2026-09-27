//! Table, JSON, and YAML rendering.
//!
//! JSON and YAML contain only the requested document or list on stdout.
//! Diagnostics belong on stderr. Tables are stable, aligned, and free of debug
//! formatting. Color is used only when stdout is a terminal and `--no-color`
//! is absent.

use crate::error::CliError;
use agentkube_agents::{AgentDeploymentDocument, AgentDocument};
use agentkube_tasks::TaskDocument;
use serde::Serialize;
use std::io::{IsTerminal, Write};

/// Writes pretty JSON to stdout, handling broken pipes cleanly.
pub fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value).map_err(|error| {
        CliError::transport("render JSON", format!("cannot encode JSON: {error}"))
    })?;
    write_stdout(&rendered, true)
}

/// Writes valid YAML to stdout preserving API field names.
pub fn print_yaml(value: &impl Serialize) -> Result<(), CliError> {
    let rendered = serde_yaml::to_string(value).map_err(|error| {
        CliError::transport("render YAML", format!("cannot encode YAML: {error}"))
    })?;
    write_stdout(rendered.trim_end(), true)
}

/// Writes a plain line to stdout.
pub fn print_line(line: &str) -> Result<(), CliError> {
    write_stdout(line, true)
}

fn write_stdout(content: &str, trailing_newline: bool) -> Result<(), CliError> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let result = if trailing_newline {
        writeln!(handle, "{content}")
    } else {
        write!(handle, "{content}")
    };
    match result {
        Ok(()) => {
            // Ensure broken pipes surface even when stdout is block-buffered.
            match handle.flush() {
                Ok(()) => Ok(()),
                Err(error) => Err(CliError::from_stdout_io("write output", &error)),
            }
        }
        Err(error) => Err(CliError::from_stdout_io("write output", &error)),
    }
}

/// Returns true when table color may be used.
#[must_use]
pub fn color_enabled(no_color: bool) -> bool {
    !no_color && std::io::stdout().is_terminal()
}

fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.len());
            }
        }
    }
    let mut output = String::new();
    output.push_str(&format_row(headers, &widths));
    output.push('\n');
    for row in rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        output.push_str(&format_row(&cells, &widths));
        output.push('\n');
    }
    output.trim_end().to_owned()
}

fn format_row(cells: &[&str], widths: &[usize]) -> String {
    let mut parts = Vec::with_capacity(cells.len());
    for (index, cell) in cells.iter().enumerate() {
        let width = widths.get(index).copied().unwrap_or(cell.len());
        if index + 1 == cells.len() {
            parts.push((*cell).to_owned());
        } else {
            parts.push(format!("{cell:<width$}"));
        }
    }
    parts.join("  ")
}

fn agent_phase(document: &AgentDocument) -> String {
    let status = document.status();
    match status {
        Some(value) => match serde_json::to_value(value) {
            Ok(json) => json
                .get("phase")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("-")
                .to_owned(),
            Err(_) => "-".to_owned(),
        },
        None => "-".to_owned(),
    }
}

fn agent_role(document: &AgentDocument) -> String {
    document.spec().role().as_str().to_owned()
}

fn deployment_numbers(document: &AgentDeploymentDocument) -> (String, String, String, String) {
    let desired = document.spec().replicas().get().to_string();
    match document.status() {
        Some(status) => (
            desired,
            status.ready_replicas().get().to_string(),
            status.available_replicas().get().to_string(),
            status.updated_replicas().get().to_string(),
        ),
        None => (desired, "-".to_owned(), "-".to_owned(), "-".to_owned()),
    }
}

fn task_state(document: &TaskDocument) -> String {
    match document.status() {
        Some(status) => match serde_json::to_value(status) {
            Ok(json) => json
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("-")
                .to_owned(),
            Err(_) => "-".to_owned(),
        },
        None => "-".to_owned(),
    }
}

fn task_priority(document: &TaskDocument) -> String {
    match serde_json::to_value(document.spec().priority()) {
        Ok(json) => json.as_str().unwrap_or("-").to_owned(),
        Err(_) => "-".to_owned(),
    }
}

fn task_agent(document: &TaskDocument) -> String {
    match document.status() {
        Some(status) => match status.assigned_agent() {
            Some(id) => id.to_string(),
            None => "-".to_owned(),
        },
        None => "-".to_owned(),
    }
}

fn task_attempts(document: &TaskDocument) -> String {
    match document.status() {
        Some(status) => status.attempts_started().to_string(),
        None => "-".to_owned(),
    }
}

fn maybe_colorize(value: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("{code}{value}\x1b[0m")
    } else {
        value.to_owned()
    }
}

fn color_for_phase(phase: &str, enabled: bool) -> String {
    if !enabled {
        return phase.to_owned();
    }
    match phase {
        "READY" => maybe_colorize(phase, "\x1b[32m", true),
        "PENDING" => maybe_colorize(phase, "\x1b[33m", true),
        "DEGRADED" | "SUSPENDED" | "TERMINATED" => maybe_colorize(phase, "\x1b[31m", true),
        _ => phase.to_owned(),
    }
}

fn color_for_state(state: &str, enabled: bool) -> String {
    if !enabled {
        return state.to_owned();
    }
    match state {
        "COMPLETED" => maybe_colorize(state, "\x1b[32m", true),
        "PENDING" | "QUEUED" | "SCHEDULED" | "RUNNING" => maybe_colorize(state, "\x1b[33m", true),
        "FAILED" | "CANCELLED" => maybe_colorize(state, "\x1b[31m", true),
        _ => state.to_owned(),
    }
}

/// Renders the agent table with columns `NAME ROLE PHASE VERSION`.
#[must_use]
pub fn render_agents_table(documents: &[AgentDocument], no_color: bool) -> String {
    let enabled = color_enabled(no_color);
    let rows: Vec<Vec<String>> = documents
        .iter()
        .map(|document| {
            let phase = agent_phase(document);
            vec![
                document.metadata().name().as_str().to_owned(),
                agent_role(document),
                color_for_phase(&phase, enabled),
                document.metadata().resource_version().get().to_string(),
            ]
        })
        .collect();
    format_table(&["NAME", "ROLE", "PHASE", "VERSION"], &rows)
}

/// Renders deployments with `NAME DESIRED READY AVAILABLE UPDATED VERSION`.
#[must_use]
pub fn render_deployments_table(documents: &[AgentDeploymentDocument], no_color: bool) -> String {
    let _ = no_color;
    let rows: Vec<Vec<String>> = documents
        .iter()
        .map(|document| {
            let (desired, ready, available, updated) = deployment_numbers(document);
            vec![
                document.metadata().name().as_str().to_owned(),
                desired,
                ready,
                available,
                updated,
                document.metadata().resource_version().get().to_string(),
            ]
        })
        .collect();
    format_table(
        &[
            "NAME",
            "DESIRED",
            "READY",
            "AVAILABLE",
            "UPDATED",
            "VERSION",
        ],
        &rows,
    )
}

/// Renders tasks with `NAME STATE PRIORITY ATTEMPTS AGENT VERSION`.
#[must_use]
pub fn render_tasks_table(documents: &[TaskDocument], no_color: bool) -> String {
    let enabled = color_enabled(no_color);
    let rows: Vec<Vec<String>> = documents
        .iter()
        .map(|document| {
            let state = task_state(document);
            vec![
                document.metadata().name().as_str().to_owned(),
                color_for_state(&state, enabled),
                task_priority(document),
                task_attempts(document),
                task_agent(document),
                document.metadata().resource_version().get().to_string(),
            ]
        })
        .collect();
    format_table(
        &["NAME", "STATE", "PRIORITY", "ATTEMPTS", "AGENT", "VERSION"],
        &rows,
    )
}

/// Prints the agent table to stdout.
pub fn print_agents_table(documents: &[AgentDocument], no_color: bool) -> Result<(), CliError> {
    write_stdout(&render_agents_table(documents, no_color), true)
}

/// Prints the deployment table to stdout.
pub fn print_deployments_table(
    documents: &[AgentDeploymentDocument],
    no_color: bool,
) -> Result<(), CliError> {
    write_stdout(&render_deployments_table(documents, no_color), true)
}

/// Prints the task table to stdout.
pub fn print_tasks_table(documents: &[TaskDocument], no_color: bool) -> Result<(), CliError> {
    write_stdout(&render_tasks_table(documents, no_color), true)
}

/// Prints health status as a stable table.
pub fn print_health_table(
    health: &crate::client::HealthResponse,
    ready: &crate::client::HealthResponse,
) -> Result<(), CliError> {
    let rows = vec![
        vec![
            "healthz".to_owned(),
            health.status.clone(),
            health.version.clone(),
        ],
        vec![
            "readyz".to_owned(),
            ready.status.clone(),
            ready.version.clone(),
        ],
    ];
    write_stdout(
        &format_table(&["COMPONENT", "STATUS", "VERSION"], &rows),
        true,
    )
}

/// Aggregate counts for the `akctl status` overview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSummary {
    /// Number of agents.
    pub agents: usize,
    /// Number of deployments.
    pub deployments: usize,
    /// Sum of desired replicas across deployments.
    pub desired_replicas: u64,
    /// Sum of ready replicas across deployments.
    pub ready_replicas: u64,
    /// Task counts keyed by wire state (for example `QUEUED`).
    pub tasks_by_state: std::collections::BTreeMap<String, u64>,
}

/// Computes aggregate counts for a status overview.
#[must_use]
pub fn summarize_status(
    agents: &[AgentDocument],
    deployments: &[AgentDeploymentDocument],
    tasks: &[TaskDocument],
) -> StatusSummary {
    let mut desired_replicas = 0u64;
    let mut ready_replicas = 0u64;
    for document in deployments {
        let (desired, ready) = deployment_desired_ready(document);
        desired_replicas += desired;
        ready_replicas += ready;
    }
    let mut tasks_by_state = std::collections::BTreeMap::new();
    for document in tasks {
        *tasks_by_state.entry(task_state(document)).or_insert(0) += 1;
    }
    StatusSummary {
        agents: agents.len(),
        deployments: deployments.len(),
        desired_replicas,
        ready_replicas,
        tasks_by_state,
    }
}

fn deployment_desired_ready(document: &AgentDeploymentDocument) -> (u64, u64) {
    let desired = u64::from(document.spec().replicas().get());
    let ready = document
        .status()
        .map_or(0, |status| u64::from(status.ready_replicas().get()));
    (desired, ready)
}

/// Renders the pretty `akctl status` dashboard.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn render_status_table(
    server_url: &str,
    health: &crate::client::HealthResponse,
    ready: &crate::client::HealthResponse,
    agents: &[AgentDocument],
    deployments: &[AgentDeploymentDocument],
    tasks: &[TaskDocument],
    no_color: bool,
) -> String {
    let summary = summarize_status(agents, deployments, tasks);
    let mut output = format!(
        "Server: {server_url} (health: {}, ready: {}, version: {})",
        health.status, ready.status, health.version
    );
    output.push_str(&format!("\n\nAGENTS ({})\n", summary.agents));
    if agents.is_empty() {
        output.push_str("(none)\n");
    } else {
        output.push_str(&render_agents_table(agents, no_color));
        output.push('\n');
    }
    output.push_str(&format!(
        "\nDEPLOYMENTS ({}, desired {}, ready {})\n",
        summary.deployments, summary.desired_replicas, summary.ready_replicas
    ));
    if deployments.is_empty() {
        output.push_str("(none)\n");
    } else {
        output.push_str(&render_deployments_table(deployments, no_color));
        output.push('\n');
    }
    let mut task_header = format!("{}", tasks.len());
    if !summary.tasks_by_state.is_empty() {
        let states: Vec<String> = summary
            .tasks_by_state
            .iter()
            .map(|(state, count)| format!("{state} {count}"))
            .collect();
        task_header.push_str(&format!(", {}", states.join(", ")));
    }
    output.push_str(&format!("\nTASKS ({task_header})\n"));
    if tasks.is_empty() {
        output.push_str("(none)\n");
    } else {
        output.push_str(&render_tasks_table(tasks, no_color));
        output.push('\n');
    }
    output.trim_end().to_owned()
}

/// Prints the pretty `akctl status` dashboard to stdout.
#[allow(clippy::too_many_arguments)]
pub fn print_status_table(
    server_url: &str,
    health: &crate::client::HealthResponse,
    ready: &crate::client::HealthResponse,
    agents: &[AgentDocument],
    deployments: &[AgentDeploymentDocument],
    tasks: &[TaskDocument],
    no_color: bool,
) -> Result<(), CliError> {
    write_stdout(
        &render_status_table(
            server_url,
            health,
            ready,
            agents,
            deployments,
            tasks,
            no_color,
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentkube_agents::{
        AgentDefinition, AgentDeployment, AgentDeploymentSpec, AgentRole, AgentSpec, Instructions,
        ModelName, ModelPolicy, ProviderName, ReplicaCount,
    };
    use agentkube_core::Metadata;
    use agentkube_tasks::{AgentTask, Objective, TaskSpec};

    fn agent_fixture(name: &str) -> AgentDocument {
        AgentDefinition::new(
            Metadata::new(name).unwrap(),
            AgentSpec::new(
                AgentRole::new("developer").unwrap(),
                ModelPolicy::fixed(
                    ProviderName::new("openai").unwrap(),
                    ModelName::new("gpt-5").unwrap(),
                ),
                Instructions::new("Build.").unwrap(),
            ),
        )
        .into_document()
    }

    fn deployment_fixture(name: &str) -> AgentDeploymentDocument {
        AgentDeployment::new(
            Metadata::new(name).unwrap(),
            AgentDeploymentSpec::new(
                ReplicaCount::new(2).unwrap(),
                AgentSpec::new(
                    AgentRole::new("developer").unwrap(),
                    ModelPolicy::fixed(
                        ProviderName::new("openai").unwrap(),
                        ModelName::new("gpt-5").unwrap(),
                    ),
                    Instructions::new("Serve.").unwrap(),
                ),
            ),
        )
        .unwrap()
        .into_document()
    }

    fn task_fixture(name: &str) -> TaskDocument {
        AgentTask::new(
            Metadata::new(name).unwrap(),
            TaskSpec::new(Objective::new("Do work.").unwrap()),
        )
        .into_document()
    }

    #[test]
    fn agent_table_is_stable_and_aligned() {
        let documents = vec![agent_fixture("bravo"), agent_fixture("alpha")];
        let rendered = render_agents_table(&documents, true);

        assert!(rendered.contains("NAME"), "{rendered}");
        assert!(rendered.contains("ROLE"), "{rendered}");
        assert!(rendered.contains("PHASE"), "{rendered}");
        assert!(rendered.contains("VERSION"), "{rendered}");
        // Preserve server order (no sorting).
        assert!(rendered.find("bravo").unwrap() < rendered.find("alpha").unwrap());
        assert!(!rendered.contains("AgentDefinition"), "no debug formatting");
    }

    #[test]
    fn deployment_and_task_tables_use_expected_columns() {
        let deployments = render_deployments_table(&[deployment_fixture("web")], true);
        assert!(deployments.contains("DESIRED"), "{deployments}");
        assert!(deployments.contains("READY"), "{deployments}");

        let tasks = render_tasks_table(&[task_fixture("job")], true);
        assert!(tasks.contains("STATE"), "{tasks}");
        assert!(tasks.contains("PRIORITY"), "{tasks}");
        assert!(tasks.contains("ATTEMPTS"), "{tasks}");
        // Absent agent renders as dash.
        assert!(tasks.contains('-'), "{tasks}");
    }

    #[test]
    fn json_is_pretty_and_yaml_is_valid() {
        let document = agent_fixture("json-agent");
        let json = serde_json::to_string_pretty(&document).unwrap();
        assert!(json.contains('\n'), "JSON must be pretty-printed");
        let yaml = serde_yaml::to_string(&document).unwrap();
        assert!(yaml.contains("apiVersion"), "{yaml}");
    }
}
