# CLI module implementation specification

This document is the implementation handoff for the final AgentKube module. It
describes the required behavior of `akctl` against the HTTP API that exists in
this repository today. The implementing agent should treat this document and
the current Rust types as the source of truth; aspirational commands in
`architecture.md` must not be faked with placeholder output.

## 1. Objective and boundaries

Create a workspace crate named `agentkube-cli` that produces an `akctl` binary.
The CLI must be a real HTTP client for `agentkube-api`, not a second control
plane and not a direct user of the storage repositories.

The first complete release must support:

- API health and readiness checks;
- create-or-update (`apply`) for `Agent` and `AgentDeployment` documents;
- create-only `apply` for `AgentTask` documents;
- get/list for agents, deployments, and tasks;
- detailed output for one resource;
- delete for agents, deployments, and terminal tasks;
- scaling deployments through optimistic-concurrency updates;
- table, JSON, and YAML output;
- deterministic configuration, errors, exit codes, and tests.

The following are explicitly deferred because the API has no endpoint for them
yet: logs, streaming watches, nodes, workflows, providers, models, metrics,
events, authentication negotiation, and server-side dry-run. Do not add commands
that always fail or print mock data. The CLI architecture should make these easy
to add later.

## 2. Existing server contract

The executable server is:

```bash
cargo run -p agentkube-api --bin agentkube-api
```

It listens on `http://127.0.0.1:8080` by default. The implemented routes are:

| Method | Route | Behavior |
|---|---|---|
| `GET` | `/healthz` | Process liveness |
| `GET` | `/readyz` | Queue/backend readiness |
| `POST` | `/v1/agents` | Create an agent |
| `GET` | `/v1/agents` | Paginated agent list |
| `GET` | `/v1/agents/{name}` | Get by `metadata.name` |
| `PUT` | `/v1/agents/{name}` | Replace using `metadata.resourceVersion` |
| `DELETE` | `/v1/agents/{name}` | Delete an agent |
| `POST` | `/v1/deployments` | Create a deployment |
| `GET` | `/v1/deployments` | Paginated deployment list |
| `GET` | `/v1/deployments/{name}` | Get by `metadata.name` |
| `PUT` | `/v1/deployments/{name}` | Replace using `metadata.resourceVersion` |
| `DELETE` | `/v1/deployments/{name}` | Delete a deployment |
| `POST` | `/v1/tasks` | Create, persist, and enqueue a task |
| `GET` | `/v1/tasks` | Paginated task list |
| `GET` | `/v1/tasks/{name}` | Get by `metadata.name` |
| `DELETE` | `/v1/tasks/{name}` | Delete only when the task is terminal |

Important protocol details:

- Paths use resource names, not UUIDs or `TaskId` values.
- Only the `default` namespace is accepted by the current API.
- Requests and responses use JSON. YAML is a CLI file/output concern.
- List queries accept `limit` (maximum 200) and `continue`.
- List responses use `apiVersion`, `kind`, `metadata`, and `items`.
- The next token is `metadata.continue`; treat it as opaque even though the
  current implementation is human-readable.
- Non-success responses deserialize as `agentkube_protocol::ApiError`.
- `409 CONFLICT` represents duplicates, stale resource versions, or an invalid
  lifecycle operation such as deleting a queued task.
- `422 UNPROCESSABLE_ENTITY` represents domain validation failures.
- `503 SERVICE_UNAVAILABLE` is retryable for safe reads.
- A newly posted task is returned in `QUEUED` state. The client must not enqueue
  or mutate its status itself.
- Agent and deployment status is server/controller owned. `apply` may preserve
  received status in its local document, but it must never synthesize status.

Read these files before implementation:

- `crates/api/src/server.rs`
- `crates/api/src/handlers.rs`
- `crates/protocol/src/error.rs`
- `crates/protocol/src/list.rs`
- `crates/agents/src/lib.rs`
- `crates/tasks/src/lib.rs`

## 3. Command-line contract

The global syntax should be:

```text
akctl [GLOBAL OPTIONS] <COMMAND>
```

Global options:

```text
--server <URL>       API base URL
--timeout <DURATION> Complete request timeout, default 30s
-o, --output <MODE>  table, json, or yaml; default table
--no-color           Disable diagnostic color
-v, --verbose...     Increase diagnostic verbosity without printing secrets
```

Resolution order for the API URL:

1. `--server`;
2. `AGENTKUBE_SERVER`;
3. `http://127.0.0.1:8080`.

Normalize the base URL once: reject non-HTTP(S) schemes, fragments, query
strings, credentials embedded in the URL, and malformed values; remove only
trailing `/` characters. Use TLS through Rustls rather than a platform OpenSSL
dependency.

Required commands:

```bash
akctl version
akctl health
akctl apply -f RESOURCE.yaml
akctl get agents [NAME]
akctl get deployments [NAME]
akctl get tasks [NAME]
akctl describe agent NAME
akctl describe deployment NAME
akctl describe task NAME
akctl delete agent NAME
akctl delete deployment NAME
akctl delete task NAME
akctl scale deployment NAME --replicas N
```

Command aliases may accept singular and plural resource names (`agent` and
`agents`), but help and diagnostics must use the canonical names shown above.
`get <resource> NAME` and `describe <resource> NAME` may share the same HTTP
operation; `describe` defaults to YAML while `get ... NAME` honors the global
output mode.

### `apply`

`apply` must accept JSON and YAML by content, not merely by file extension. It
should accept `-` for stdin and support multiple YAML documents in one stream.
Authoring manifests are not identical to persisted wire documents: users may
omit `metadata.uid`, `metadata.resourceVersion`, and `status`, as shown in
`architecture.md`. Implement a strict CLI manifest envelope containing
`apiVersion`, `kind`, authoring metadata (`name` and optional `namespace`), and
the typed `spec`. Do not deserialize a short authoring manifest directly into
`Metadata`, because persisted metadata intentionally requires identity fields.

For every document:

1. Read `apiVersion` and `kind` without discarding unknown fields.
2. Require `apiVersion: agentkube.ai/v1`.
3. Deserialize `spec` into the exact domain type (`AgentSpec`,
   `AgentDeploymentSpec`, or `TaskSpec`) and construct the corresponding domain
   resource so all invariants run.
4. Treat input UID, resource version, and status as non-authoritative. This
   allows output from `get -o yaml` to be applied safely without letting clients
   own controller status.
5. For `Agent` and `AgentDeployment`, `GET` by name:
   - on `404`, construct fresh `Metadata` from the manifest name/namespace,
     which creates a new UID at resource version one, then send `POST`;
   - on success, copy the server's immutable UID and current resource version
     by cloning the complete server `Metadata` into a newly constructed desired
     resource, then send `PUT`;
   - propagate every other error.
6. For `AgentTask`, construct a new pending task and use `POST` only. Tasks are
   execution records and must not be silently replaced.
7. Print one result line per successfully processed document.

This normalization layer is required because `Metadata` deliberately exposes no
UID mutator. Do not add one to `agentkube-core` merely for CLI convenience.

Multi-document apply is not atomic because the API has no transaction endpoint.
Stop on the first failure and clearly state how many previous documents were
applied. Never claim rollback occurred. A `--dry-run=client` option is desirable:
it should parse and validate every document without making requests.

### `scale`

Scaling must:

1. `GET /v1/deployments/{name}`;
2. validate `N` using `ReplicaCount` (currently `0..=10_000`);
3. change only `spec.replicas`;
4. `PUT` the complete document with the current UID and resource version;
5. surface a `409` conflict instead of silently overwriting a concurrent edit.

Do not automatically retry mutating requests. Without idempotency keys, retrying
`POST` or replaying a stale `PUT` can produce surprising results.

## 4. Output behavior

Human-readable tables must be stable, aligned, and free of debug formatting.
Suggested columns are:

```text
AGENTS:       NAME  ROLE  PHASE  VERSION
DEPLOYMENTS:  NAME  DESIRED  READY  AVAILABLE  UPDATED  VERSION
TASKS:        NAME  STATE  PRIORITY  ATTEMPTS  AGENT  VERSION
```

Use `-` for absent values. Preserve the deterministic order returned by the
server. Table output may use color only when stdout is a terminal and
`--no-color` is absent. JSON and YAML must contain only the requested document
or list on stdout, with no progress messages.

For machine-readable modes:

- JSON must be pretty-printed valid JSON.
- YAML must be valid YAML and preserve the API field naming.
- diagnostics, retries, and progress belong on stderr;
- never log an `Authorization` value or complete request headers.

List commands should follow all continuation tokens by default and combine the
items into one output. Add `--page-size` in the inclusive range `1..=200` and an
optional `--continue` for advanced/manual pagination. Protect against a server
returning the same continuation token repeatedly.

## 5. Errors, retries, and exit codes

Create a single CLI error type that retains context and preserves an
`ApiError` response when one exists. Diagnostics should include the operation,
HTTP status/reason, and server message without dumping response bodies that
failed size limits.

Stable process exit codes:

| Code | Meaning |
|---|---|
| `0` | Complete success |
| `1` | Transport, server, or unexpected runtime failure |
| `2` | CLI usage, local file, decoding, or client-side validation failure |
| `3` | Partial multi-document apply |

Retry only idempotent `GET` requests on connection resets and `502`, `503`, or
`504`. Use a small bounded exponential backoff, honor `retryAfterSeconds` when
present, and cap the total operation by `--timeout`. Do not retry `400`, `404`,
`409`, or `422`.

## 6. Recommended crate structure

```text
crates/cli/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── args.rs          # clap types only
│   ├── client.rs        # typed HTTP client and retry policy
│   ├── command.rs       # command orchestration
│   ├── config.rs        # URL/env/timeout resolution
│   ├── document.rs      # JSON/YAML multi-document decoding
│   ├── error.rs         # diagnostics and exit classification
│   ├── output.rs        # tables, JSON, YAML
│   └── bin/
│       └── akctl.rs     # minimal process entrypoint
└── tests/
    ├── cli.rs           # binary-level behavior
    └── http_contract.rs # real router or mock-server contract tests
```

Keep `main` minimal: parse arguments, build configuration/client, execute one
command, render the result, and convert the typed error into an exit code.
Business logic must remain testable through the library crate.

Suggested dependencies:

- `clap` with derive support;
- `reqwest` with `json` and `rustls-tls`, with default TLS features disabled;
- `tokio`;
- `serde`, `serde_json`, and a maintained YAML parser;
- `agentkube-agents`, `agentkube-core`, `agentkube-protocol`, and
  `agentkube-tasks` for exact wire/domain types;
- a small table-rendering crate only if it does not force ANSI output;
- `assert_cmd`, `tempfile`, and the real `agentkube-api` router or a focused
  HTTP mock for integration tests.

Do not duplicate resource structs inside the CLI. Do not depend on
`agentkube-storage`, `agentkube-queue`, or controller internals in production
CLI code.

## 7. Security and operational requirements

- Bound files and response bodies before buffering them. A 2 MiB default is
  consistent with the current API configuration; expose a documented ceiling.
- Reject stdin/file input that is empty or contains no resource documents.
- Never use `unwrap`/`expect` on user, network, or filesystem input.
- Send a descriptive `User-Agent` containing the CLI version.
- Set `Accept: application/json` and `Content-Type: application/json` when a
  body is present.
- Reserve support for `AGENTKUBE_TOKEN`; if implemented now, send it as a Bearer
  token and redact it from every diagnostic.
- Do not disable TLS certificate validation.
- Handle broken pipes as clean termination when output is piped to tools such
  as `head`.
- Make all behavior compatible with the workspace MSRV declared in the root
  `Cargo.toml`.

## 8. Required tests

At minimum, cover:

1. help, version, invalid arguments, and stable exit codes;
2. URL precedence and validation;
3. JSON, YAML, stdin, and multi-document parsing;
4. rejection of unknown `apiVersion`/`kind` and malformed domain resources;
5. agent apply create and update paths;
6. deployment apply and scale with resource-version conflicts;
7. task apply producing the server's `QUEUED` response;
8. deterministic table, JSON, and YAML rendering;
9. multi-page list traversal and repeated-token protection;
10. structured `ApiError` rendering for `404`, `409`, `422`, and `503`;
11. timeout/server-unavailable behavior;
12. proof that mutating requests are not automatically retried;
13. delete behavior, including the conflict returned for non-terminal tasks;
14. oversized input and response handling;
15. broken-pipe behavior.

Prefer integration tests against `agentkube_api::router` on a port-zero
`TcpListener` so tests validate the real wire contract. Keep unit tests for URL
normalization, document dispatch, retry classification, and output formatting.

## 9. Implementation order

1. Scaffold `agentkube-cli`, `akctl`, argument types, and typed errors.
2. Implement configuration resolution and the HTTP client.
3. Implement resource document decoding and output renderers.
4. Add health and read-only `get`/`describe` commands.
5. Add `apply`, preserving UID/resource-version semantics.
6. Add delete and scale.
7. Add pagination, safe GET retries, limits, and broken-pipe handling.
8. Complete binary-level and HTTP contract tests.
9. Update the README status table and replace “Planned CLI experience” with
   actual usage examples.

## 10. Definition of done

The module is complete only when all of the following are true:

- `cargo run -p agentkube-cli --bin akctl -- --help` succeeds;
- every required command above performs a real API operation;
- there are no placeholder commands or fabricated resources;
- resource types come from existing domain/protocol crates;
- JSON/YAML output is clean on stdout and diagnostics use stderr;
- the required integration scenarios pass;
- `cargo fmt --all` is clean;
- `cargo test --workspace --all-targets` passes;
- `cargo clippy --workspace --all-targets -- -D warnings` passes;
- `cargo doc --workspace --no-deps` passes;
- the README accurately marks the CLI as implemented only after those checks;
- the work is committed as one focused CLI feature commit.
