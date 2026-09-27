//! Durable SQLite repositories for AgentKube.
//!
//! This crate implements the infrastructure-independent ports from
//! `agentkube-storage` on top of a single SQLite file. Wire documents are
//! stored as JSON and revalidated through `from_document` on every read, so
//! persisted state always satisfies domain invariants.
//!
//! The queue intentionally stays in memory: it transports task identities
//! while durable state lives here. Operators recover by re-enqueueing
//! `QUEUED` tasks found in storage at boot (see `docs/parity-plan.md`).
//!
//! SQLite operations run under one mutex with short critical sections and no
//! `.await` while the lock is held. The blocking time is bounded by single
//! indexed-row statements, which keeps this suitable for local-first
//! single-writer operation.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod repository;

pub use error::SqliteError;
pub use repository::{SqliteResourceRepository, SqliteStores};
