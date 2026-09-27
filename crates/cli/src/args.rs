//! Command-line argument types for `akctl`.
//!
//! This module contains only `clap` types and parsing helpers. Resolution of
//! configuration values and all I/O lives in other modules.

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use std::{fmt, str::FromStr};

/// Output rendering mode selected with `--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    /// Stable aligned human-readable tables.
    Table,
    /// Pretty-printed JSON on stdout.
    Json,
    /// Valid YAML on stdout preserving API field names.
    Yaml,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Table => formatter.write_str("table"),
            Self::Json => formatter.write_str("json"),
            Self::Yaml => formatter.write_str("yaml"),
        }
    }
}

/// Canonical resource selector for `get` (plural canonical names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetResource {
    /// Agent resources, canonical `agents`.
    Agents,
    /// Deployment resources, canonical `deployments`.
    Deployments,
    /// Task resources, canonical `tasks`.
    Tasks,
}

impl GetResource {
    /// Returns the canonical plural name used in help and diagnostics.
    #[must_use]
    pub const fn canonical(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Deployments => "deployments",
            Self::Tasks => "tasks",
        }
    }
}

impl FromStr for GetResource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "agent" | "agents" => Ok(Self::Agents),
            "deployment" | "deployments" => Ok(Self::Deployments),
            "task" | "tasks" => Ok(Self::Tasks),
            other => Err(format!(
                "invalid resource {other:?}: expected agents, deployments, or tasks"
            )),
        }
    }
}

/// Canonical singular resource selector for `describe` and `delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleResource {
    /// One agent, canonical `agent`.
    Agent,
    /// One deployment, canonical `deployment`.
    Deployment,
    /// One task, canonical `task`.
    Task,
}

impl SingleResource {
    /// Returns the canonical singular name used in help and diagnostics.
    #[must_use]
    pub const fn canonical(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Deployment => "deployment",
            Self::Task => "task",
        }
    }
}

impl FromStr for SingleResource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "agent" | "agents" => Ok(Self::Agent),
            "deployment" | "deployments" => Ok(Self::Deployment),
            "task" | "tasks" => Ok(Self::Task),
            other => Err(format!(
                "invalid resource {other:?}: expected agent, deployment, or task"
            )),
        }
    }
}

/// Target selector for `scale` (only deployments are supported by the API).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleResource {
    /// Deployment target, canonical `deployment`.
    Deployment,
}

impl ScaleResource {
    /// Returns the canonical name used in help and diagnostics.
    #[must_use]
    pub const fn canonical(self) -> &'static str {
        "deployment"
    }
}

impl FromStr for ScaleResource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "deployment" | "deployments" => Ok(Self::Deployment),
            other => Err(format!(
                "invalid resource {other:?}: only deployment can be scaled"
            )),
        }
    }
}

/// Global `akctl` arguments.
#[derive(Debug, Parser)]
#[command(
    name = "akctl",
    version,
    about = "Command-line client for the AgentKube API"
)]
pub struct Cli {
    /// API base URL (for example http://127.0.0.1:8080).
    #[arg(long, value_name = "URL", global = true)]
    pub server: Option<String>,

    /// Complete request timeout (for example 30s, 500ms, 1m). Total operation deadline.
    #[arg(long, value_name = "DURATION", default_value = "30s", global = true)]
    pub timeout: String,

    /// Output mode: table, json, or yaml. Describe defaults to yaml when unset.
    #[arg(short, long, value_enum, value_name = "MODE", global = true)]
    pub output: Option<OutputMode>,

    /// Disable diagnostic color.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Increase diagnostic verbosity without printing secrets.
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// Available `akctl` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the CLI version.
    Version,
    /// Check API liveness and readiness.
    Health,
    /// Show a pretty overview of everything running.
    Status,
    /// Create or update resources from a JSON/YAML file or stdin.
    Apply {
        /// Resource file path, or - for stdin.
        #[arg(short, long, value_name = "FILE")]
        file: String,
        /// Validate only without sending requests. Only client mode is supported.
        #[arg(long = "dry-run", value_name = "MODE", num_args = 0..=1, default_missing_value = "client")]
        dry_run: Option<String>,
    },
    /// Get one resource or list resources of a kind.
    Get {
        /// Resource kind: agents, deployments, or tasks (singular aliases accepted).
        #[arg(value_name = "RESOURCE")]
        resource: GetResource,
        /// Optional resource name. When omitted, all resources are listed.
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Page size used for list pagination (1..=200).
        #[arg(long, value_name = "N")]
        page_size: Option<u32>,
        /// Continuation token to start listing from.
        #[arg(long = "continue", value_name = "TOKEN")]
        continue_token: Option<String>,
    },
    /// Show full details for one resource (defaults to YAML output).
    Describe {
        /// Resource kind: agent, deployment, or task (plural aliases accepted).
        #[arg(value_name = "RESOURCE")]
        resource: SingleResource,
        /// Resource name.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Delete one resource.
    Delete {
        /// Resource kind: agent, deployment, or task (plural aliases accepted).
        #[arg(value_name = "RESOURCE")]
        resource: SingleResource,
        /// Resource name.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Scale a deployment to N replicas with optimistic concurrency.
    Scale {
        /// Resource kind: deployment (deployments alias accepted).
        #[arg(value_name = "RESOURCE")]
        resource: ScaleResource,
        /// Deployment name.
        #[arg(value_name = "NAME")]
        name: String,
        /// Desired replica count (0..=10000).
        #[arg(long, value_name = "N")]
        replicas: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn get_accepts_singular_and_plural_aliases() {
        assert_eq!("agent".parse::<GetResource>().unwrap(), GetResource::Agents);
        assert_eq!(
            "deployments".parse::<GetResource>().unwrap(),
            GetResource::Deployments
        );
        assert_eq!("TASKS".parse::<GetResource>().unwrap(), GetResource::Tasks);
    }

    #[test]
    fn single_resource_uses_canonical_singular_names() {
        assert_eq!(
            SingleResource::Agent.canonical(),
            "agent",
            "help must use canonical singular names"
        );
        assert!("agents".parse::<SingleResource>().is_ok());
        assert!("invalid".parse::<SingleResource>().is_err());
    }

    #[test]
    fn scale_rejects_non_deployment_resources() {
        assert!("deployment".parse::<ScaleResource>().is_ok());
        assert!("deployments".parse::<ScaleResource>().is_ok());
        assert!("agent".parse::<ScaleResource>().is_err());
    }

    #[test]
    fn global_options_parse_with_defaults() {
        let cli = Cli::try_parse_from(["akctl", "version"]).unwrap();
        assert_eq!(cli.timeout, "30s");
        assert!(cli.output.is_none());
        assert!(!cli.no_color);
    }

    #[test]
    fn global_options_parse_before_and_after_the_subcommand() {
        let before = Cli::try_parse_from([
            "akctl",
            "--server",
            "http://x:1",
            "-o",
            "json",
            "get",
            "tasks",
        ])
        .unwrap();
        assert_eq!(before.output, Some(OutputMode::Json));

        // Same flags after the subcommand (kubectl-style) must also work.
        let after = Cli::try_parse_from([
            "akctl",
            "get",
            "tasks",
            "-o",
            "json",
            "--server",
            "http://x:1",
        ])
        .unwrap();
        assert_eq!(after.output, Some(OutputMode::Json));
        assert_eq!(after.server.as_deref(), Some("http://x:1"));
        match after.command {
            Command::Get { resource, .. } => assert_eq!(resource, GetResource::Tasks),
            other => panic!("expected get, got {other:?}"),
        }
    }

    #[test]
    fn apply_dry_run_defaults_to_client_when_flag_has_no_value() {
        let cli = Cli::try_parse_from(["akctl", "apply", "-f", "-", "--dry-run"]).unwrap();
        match cli.command {
            Command::Apply { dry_run, .. } => assert_eq!(dry_run.as_deref(), Some("client")),
            other => panic!("expected apply, got {other:?}"),
        }
    }
}
