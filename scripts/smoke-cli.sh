#!/usr/bin/env bash
# End-to-end smoke test for every akctl command.
#
# Spins up a throwaway agentkube-api on 127.0.0.1:18080, exercises version,
# health, apply, get, describe, delete and scale (including table/json/yaml
# output, pagination, dry-run and every stable exit code), then stops it.
#
# Usage:
#   ./scripts/smoke-cli.sh
#
# Env:
#   AKCTL       akctl binary (default: akctl from PATH)
#   API_BIN     agentkube-api binary (default: agentkube-api from PATH)
#   SMOKE_PORT  throwaway server port (default: 18080)

set -u

AKCTL="${AKCTL:-akctl}"
API_BIN="${API_BIN:-agentkube-api}"
PORT="${SMOKE_PORT:-18080}"
SERVER_URL="http://127.0.0.1:${PORT}"

PASS=0
FAIL=0
SRV_PID=""
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"; kill "$SRV_PID" 2>/dev/null' EXIT

pass() { PASS=$((PASS + 1)); echo "PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $1"; }

# ak <args...> runs akctl against the throwaway server without update noise.
ak() {
  AGENTKUBE_NO_UPDATE_CHECK=1 "$AKCTL" --server "$SERVER_URL" "$@"
}

# expect_ok "desc" -- <akctl args...>
expect_ok() {
  local desc="$1"
  shift
  [ "${1:-}" = "--" ] && shift
  if ak "$@" > "$TMPDIR/out.txt" 2> "$TMPDIR/err.txt"; then
    pass "$desc"
  else
    fail "$desc (exit $?, stderr: $(head -c 200 "$TMPDIR/err.txt"))"
  fi
}

# expect_code <code> "desc" -- <akctl args...>
expect_code() {
  local code="$1" desc="$2"
  shift 2
  [ "${1:-}" = "--" ] && shift
  ak "$@" > "$TMPDIR/out.txt" 2> "$TMPDIR/err.txt"
  if [ "$?" = "$code" ]; then
    pass "$desc (exit $code)"
  else
    fail "$desc (want exit $code, stderr: $(head -c 200 "$TMPDIR/err.txt"))"
  fi
}

# expect_out "desc" "needle" -- <akctl args...>
expect_out() {
  local desc="$1" needle="$2"
  shift 2
  [ "${1:-}" = "--" ] && shift
  if ak "$@" > "$TMPDIR/out.txt" 2> "$TMPDIR/err.txt" && grep -qF "$needle" "$TMPDIR/out.txt"; then
    pass "$desc"
  else
    fail "$desc (missing '$needle')"
  fi
}

command -v "$AKCTL" >/dev/null 2>&1 || { echo "missing binary: $AKCTL" >&2; exit 1; }
command -v "$API_BIN" >/dev/null 2>&1 || { echo "missing binary: $API_BIN" >&2; exit 1; }

AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS="127.0.0.1:${PORT}" "$API_BIN" > "$TMPDIR/server.log" 2>&1 &
SRV_PID=$!
for _ in $(seq 1 50); do
  ak health > /dev/null 2>&1 && break
  sleep 0.2
done
ak health > /dev/null 2>&1 || { echo "server did not start, see $TMPDIR/server.log" >&2; exit 1; }
echo "server up at $SERVER_URL (pid $SRV_PID)"

cat > "$TMPDIR/agent.yaml" <<'EOF'
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
EOF

cat > "$TMPDIR/deployment.yaml" <<'EOF'
apiVersion: agentkube.ai/v1
kind: AgentDeployment
metadata:
  name: workers
spec:
  replicas: 2
  template:
    role: developer
    model:
      strategy: fixed
      provider: openai
      model: gpt-5
    instructions: Serve work.
EOF

cat > "$TMPDIR/task.yaml" <<'EOF'
apiVersion: agentkube.ai/v1
kind: AgentTask
metadata:
  name: review-code
spec:
  objective: Review the code.
EOF

cat > "$TMPDIR/bad-kind.yaml" <<'EOF'
apiVersion: agentkube.ai/v1
kind: Starship
metadata:
  name: nope
spec: {}
EOF

# version + health in every output mode
expect_out "version table" "akctl v" -- version
expect_out "version json" "clientVersion" -- -o json version
expect_out "health table" "healthz" -- health
expect_out "health json" '"health"' -- -o json health

# agents: create, idempotent update, get, describe
expect_out "apply agent created" "created" -- apply -f "$TMPDIR/agent.yaml"
expect_out "apply agent configured" "configured" -- apply -f "$TMPDIR/agent.yaml"
expect_out "get agents table" "backend-agent" -- get agents
expect_out "get agent json" '"backend-agent"' -- -o json get agents backend-agent
expect_out "describe agent yaml" "kind: Agent" -- describe agent backend-agent
expect_out "describe agent json" '"kind": "Agent"' -- -o json describe agent backend-agent

# extra agents for pagination
for name in smoke-a smoke-b; do
  sed "s/backend-agent/$name/" "$TMPDIR/agent.yaml" > "$TMPDIR/$name.yaml"
  expect_out "apply agent $name" "created" -- apply -f "$TMPDIR/$name.yaml"
done
expect_out "list merges pages" "smoke-b" -- get agents --page-size 2
# Manual pagination starts from a continuation token (opaque namespace/name
# format served by the API, same contract as the http_contract tests) and
# still follows to the end: starting after smoke-a yields only smoke-b.
if ak get agents --continue default/smoke-a > "$TMPDIR/out.txt" 2> "$TMPDIR/err.txt" \
  && grep -q "smoke-b" "$TMPDIR/out.txt" \
  && ! grep -q "backend-agent" "$TMPDIR/out.txt"; then
  pass "manual --continue"
else
  fail "manual --continue"
fi
expect_code 2 "bad --page-size rejected" -- get agents --page-size 0
expect_code 2 "unknown resource rejected" -- get starship

# deployments: apply, scale, verify, conflicts, validation
expect_out "apply deployment" "created" -- apply -f "$TMPDIR/deployment.yaml"
expect_out "scale deployment" "scaled" -- scale deployment workers --replicas 3
expect_out "scaled replicas visible" '"replicas": 3' -- -o json get deployments workers
expect_out "describe deployment" "AgentDeployment" -- describe deployment workers
expect_code 2 "replicas out of range" -- scale deployment workers --replicas 10001
expect_code 1 "scale missing deployment" -- scale deployment missing --replicas 2

# tasks: create (QUEUED), read, lifecycle-guarded delete
expect_out "apply task" "created" -- apply -f "$TMPDIR/task.yaml"
expect_out "task is QUEUED" "QUEUED" -- get tasks
expect_out "flags after subcommand" "QUEUED" -- get tasks -o json
expect_out "describe task" "review-code" -- describe task review-code
expect_code 1 "queued task delete conflicts" -- delete task review-code

# dry-run, invalid docs, missing resources
{ cat "$TMPDIR/agent.yaml"; printf '\n---\n'; cat "$TMPDIR/task.yaml"; } > "$TMPDIR/multi.yaml"
expect_out "dry-run multi-doc" "validated" -- apply -f "$TMPDIR/multi.yaml" --dry-run=client
expect_code 2 "unknown kind rejected" -- apply -f "$TMPDIR/bad-kind.yaml" --dry-run=client
expect_code 1 "get missing agent" -- get agents does-not-exist
expect_code 1 "delete missing agent" -- delete agent does-not-exist

# cleanup
expect_ok "delete deployment" -- delete deployment workers
expect_ok "delete agents" -- delete agent smoke-a
ak delete agent smoke-b > /dev/null 2>&1
ak delete agent backend-agent > /dev/null 2>&1
expect_code 1 "deleted agent is gone" -- get agents backend-agent

# flags
expect_ok "verbose health" -- -v health
expect_ok "timeout flag" -- --timeout 5s health
expect_ok "no-color table" -- --no-color get agents

echo "---"
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = "0" ]
