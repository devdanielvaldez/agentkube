#!/usr/bin/env bash
# Demo: three local AgentKube clusters on one machine.
#
# Starts one agentkube-api per cluster (each with isolated in-memory state),
# applies that cluster's manifests with akctl, exercises get/describe/scale,
# and stops everything at the end (unless --keep).
#
# Usage:
#   ./demo/demo.sh [--keep]
#
# Env:
#   AKCTL    akctl binary (default: akctl from PATH)
#   API_BIN  agentkube-api binary (default: agentkube-api from PATH)
#
# Clusters:
#   ollama-local  127.0.0.1:18081  local models only (ollama)
#   cloud         127.0.0.1:18082  hosted providers (openai, anthropic)
#   edge          127.0.0.1:18083  tiny on-device footprint (ollama)

set -euo pipefail

AKCTL="${AKCTL:-akctl}"
API_BIN="${API_BIN:-agentkube-api}"
KEEP=0
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

usage() {
  echo "Usage: demo.sh [--keep]" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "demo.sh: unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

CLUSTERS="ollama-local:18081 cloud:18082 edge:18083"
PIDS=""
LOG_DIR="$SCRIPT_DIR/.logs"

cleanup() {
  if [ "$KEEP" = "1" ]; then
    return 0
  fi
  for pid in $PIDS; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT

command -v "$AKCTL" >/dev/null 2>&1 || { echo "missing binary: $AKCTL" >&2; exit 1; }
command -v "$API_BIN" >/dev/null 2>&1 || { echo "missing binary: $API_BIN" >&2; exit 1; }
mkdir -p "$LOG_DIR"

section() {
  echo ""
  echo "=== $1 ==="
}

# start <name> <port>
start() {
  AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS="127.0.0.1:$2" "$API_BIN" > "$LOG_DIR/$1.log" 2>&1 &
  PIDS="$PIDS $!"
  for _ in $(seq 1 50); do
    if AGENTKUBE_NO_UPDATE_CHECK=1 "$AKCTL" --server "http://127.0.0.1:$2" health > /dev/null 2>&1; then
      echo "cluster $1 up at http://127.0.0.1:$2"
      return 0
    fi
    sleep 0.2
  done
  echo "cluster $1 did not start, see $LOG_DIR/$1.log" >&2
  exit 1
}

section "Starting clusters"
for entry in $CLUSTERS; do
  name="${entry%%:*}"
  port="${entry##*:}"
  start "$name" "$port"
done

section "Applying manifests"
for entry in $CLUSTERS; do
  name="${entry%%:*}"
  port="${entry##*:}"
  export AGENTKUBE_NO_UPDATE_CHECK=1
  "$AKCTL" --server "http://127.0.0.1:$port" apply -f "$SCRIPT_DIR/clusters/$name/agent.yaml"
  "$AKCTL" --server "http://127.0.0.1:$port" apply -f "$SCRIPT_DIR/clusters/$name/deployment.yaml"
  "$AKCTL" --server "http://127.0.0.1:$port" apply -f "$SCRIPT_DIR/clusters/$name/task.yaml"
done

section "Cluster inventory"
for entry in $CLUSTERS; do
  name="${entry%%:*}"
  port="${entry##*:}"
  echo "--- $name (http://127.0.0.1:$port) ---"
  "$AKCTL" --server "http://127.0.0.1:$port" get agents
  "$AKCTL" --server "http://127.0.0.1:$port" get deployments
  "$AKCTL" --server "http://127.0.0.1:$port" get tasks
done

section "Scaling the edge cluster"
"$AKCTL" --server http://127.0.0.1:18083 scale deployment edge-helpers --replicas 2
"$AKCTL" --server http://127.0.0.1:18083 get deployments

section "Detail view (cloud researcher)"
"$AKCTL" --server http://127.0.0.1:18082 describe agent cloud-researcher | head -n 20

section "Done"
echo "Each cluster keeps isolated in-memory state. Keep exploring, e.g.:"
echo "  akctl --server http://127.0.0.1:18081 get tasks -o json"
echo "  akctl --server http://127.0.0.1:18082 get deployments -o yaml"
if [ "$KEEP" = "1" ]; then
  echo "Servers left running (pids:$PIDS). Stop them with:"
  echo "  kill$PIDS"
else
  echo "Throwaway servers stopped. Re-run with --keep to leave them up."
fi
