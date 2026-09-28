# Kubernetes parity: execution plan

Goal: turn AgentKube from a well-modeled control-plane prototype into an
operating orchestrator, without faking anything and without breaking the
trait boundaries that already exist. Each phase ships working, tested code
and ends with a demoable behavior change.

## Ground rules

- No placeholder output, no mock data in production paths.
- New backends implement existing ports (`ResourceRepository`, `TaskQueue`,
  `ModelProvider`, `WorkerStateStore`); domain crates stay
  infrastructure-free.
- Single-writer local operator first; multi-writer/HA is an explicit
  non-goal for this plan.
- Queue entries stay ephemeral by design (the queue transports identities);
  durable state lives in repositories. Crash recovery replays from storage.

## Phase 1 — Durable state (SQLite)

New crate `agentkube-sqlite`: `ResourceRepository` for `AgentDefinition`,
`AgentDeployment`, and `AgentTask` backed by one SQLite file (WAL mode).

- Store wire documents as JSON; validate on read via `from_document`.
- Mirror `InMemoryResourceRepository` error semantics exactly
  (`AlreadyExists`, `UidAlreadyExists`, `Conflict`, `IdentityChanged`,
  `InvalidInitialVersion`, `VersionExhausted`), including deterministic
  `(namespace, name)` list order.
- Queue stays in-memory. Boot recovery rule (owned by the operator):
  `QUEUED` tasks are re-enqueued; `RUNNING`/waiting tasks fail with
  `WorkerLost`; `SCHEDULED` tasks are cancelled (never executed);
  `PENDING` and terminal tasks are untouched.
- Done when: data survives process restarts with versions intact, all
  concurrency tests pass against SQLite, and `cargo +1.88 check` passes.

## Phase 2 — Operator binary (reconcile + dispatch)

New crate `agentkube-operator` with an `agentkube-operator` binary:

- Serves the existing `agentkube-api` router on SQLite repositories.
- Reconcile tick: runs `AgentDefinitionReconciler` per agent and persists
  status updates through `replace` (optimistic concurrency preserved).
- Dispatch tick: builds real `SchedulingCandidate`s (local node snapshot +
  ready instances + definitions + provider availability), calls
  `Scheduler::schedule` per queued task, and executes the winner with the
  existing worker state machinery + bounded `AgenticRuntime` + a repository-backed
  `WorkerStateStore` (TaskId→key index rebuilt at boot).
- Unschedulable tasks are left `QUEUED` with a visible warning, never
  force-assigned.
- Done when: `apply -f task.yaml` reaches a terminal state end to end
  without human intervention, and crash-recovery behaves per Phase 1.

Deployment reconciliation now materializes the requested number of embedded
instances, updates replica status, and supports scale-up and safe idle
scale-down. Dispatch claims queue leases before scheduling and runs independent
ready replicas concurrently while renewing their leases.

## Phase 3 — Real inference (provider adapters)

New crate `agentkube-providers-http` with `ModelProvider` implementations:

- `OllamaProvider`, `OpenAiProvider`, `AnthropicProvider`, and
  `GeminiProvider`, with credentials read from configuration secrets and never
  logged.
- Registered in the router's `ProviderRegistry`; `Fixed` policies route by
  name, `Auto` policies use router selection. Tool-requiring agents are
  rejected with `Validation`, never silently downgraded.
- Costs recorded from real usage; budgets keep enforcing.
- The embedded runtime supports bounded multi-turn tool loops through an
  explicit allowlisted `ToolRegistry`; missing and unauthorized tools fail
  closed.
- Done when: a task applied against local Ollama completes with model
  output, usage, and cost persisted in task status.

## Phase 4 — Hardening slice (auth, metrics, nodes)

- API Bearer auth: `AGENTKUBE_AUTH_TOKEN`; missing/invalid tokens get
  `401 UNAUTHORIZED` on `/v1/*` (health/readiness stay open). The CLI
  already sends `AGENTKUBE_TOKEN` as Bearer.
- `GET /metrics` (Prometheus text): per-resource counts, tasks by state,
  queue depth, worker heartbeats. No new dependencies.
- Node registry fed by embedded-worker heartbeats, served at
  `GET /v1/nodes` as the scheduling source of truth.
- Docs (`docs/install.md`, `demo/`) and README updated.
- Done when: unauthorized writes fail, metrics scrape, nodes reflect live
  workers, and the full gate suite
  (`fmt`, `clippy -D warnings`, `test --workspace --all-targets`, `doc`)
  passes on stable and MSRV.

## Non-goals for this plan

Multi-writer HA/etcd replacement, GPU/device scheduling, persistent
volumes, services/DNS, workflows/cron, admission webhooks, and a public
provider marketplace. Each is a follow-up with its own design doc.
