//! Command-line client for the AgentKube HTTP API.
//!
//! The `agentkube-cli` crate implements the `akctl` binary as a real HTTP
//! client for `agentkube-api`. It performs no direct storage access and
//! contains no control-plane logic.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod args;
pub mod client;
pub mod command;
pub mod config;
pub mod document;
pub mod error;
pub mod output;
pub mod update;
