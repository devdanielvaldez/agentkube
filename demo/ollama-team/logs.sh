#!/usr/bin/env bash
# Shows persisted task output/failures from the development AgentKube CLI.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
AKCTL="${AKCTL:-$REPO_DIR/target/debug/akctl}"
SERVER="${AGENTKUBE_SERVER:-http://127.0.0.1:18080}"
TASKS="plan-local-api implement-health-handler review-auth-design write-agentkube-quickstart"

usage() {
  echo "Usage: logs.sh [--snapshot] | logs.sh [TASK] | logs.sh --follow TASK" >&2
}

if [ ! -x "$AKCTL" ]; then
  echo "No existe $AKCTL. Ejecuta ./demo/ollama-team/run.sh primero." >&2
  exit 1
fi

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  -f|--follow)
    [ $# -eq 2 ] || { usage; exit 2; }
    exec "$AKCTL" --server "$SERVER" logs --follow "$2"
    ;;
  --snapshot)
    [ $# -eq 1 ] || { usage; exit 2; }
    for task in $TASKS; do
      echo "=== $task ==="
      "$AKCTL" --server "$SERVER" logs "$task" || true
      echo
    done
    ;;
  "")
    echo "Esperando la salida final de las tareas (Ctrl-C para cancelar)..."
    echo
    for task in $TASKS; do
      echo "=== $task ==="
      "$AKCTL" --server "$SERVER" logs --follow "$task" || true
      echo
    done
    ;;
  *)
    [ $# -eq 1 ] || { usage; exit 2; }
    exec "$AKCTL" --server "$SERVER" logs "$1"
    ;;
esac
