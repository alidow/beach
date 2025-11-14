#!/usr/bin/env bash
set -euo pipefail

LOG_DIR=${LOG_DIR:-"$HOME/beach-debug"}
roles=(lhs rhs agent)

USE_JQ=false

if command -v jq >/dev/null 2>&1; then
  USE_JQ=true
elif ! command -v python3 >/dev/null 2>&1; then
  echo "Error: install jq or python3 to parse bootstrap JSON files." >&2
  exit 1
fi

json_value_python() {
  local file="$1"
  shift
  python3 - "$file" "$@" <<'PY'
import json
import sys
from collections.abc import Mapping, Sequence

def load_payload(path: str):
    try:
        with open(path, "r", encoding="utf-8") as fh:
            raw = fh.read().strip()
    except OSError:
        return None
    if not raw:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        pass
    for line in raw.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            return json.loads(line)
        except json.JSONDecodeError:
            continue
    return None

def walk(node):
    if isinstance(node, Mapping):
        yield node
        for value in node.values():
            yield from walk(value)
    elif isinstance(node, Sequence) and not isinstance(node, (str, bytes, bytearray)):
        for item in node:
            yield from walk(item)

payload = load_payload(sys.argv[1])
if payload is None:
    sys.exit(0)

keys = sys.argv[2:]
for obj in walk(payload):
    for key in keys:
        if key in obj:
            value = obj[key]
            if isinstance(value, (str, int, float)):
                print(value)
                sys.exit(0)
sys.exit(0)
PY
}

print_from_json() {
  local file="$1" role="$2"
  [[ -f "$file" ]] || return 1
  local session passcode
  if [[ "$USE_JQ" == true ]]; then
    session=$( { jq -r '..|.session_id? // .sessionId? // empty' "$file" 2>/dev/null || true; } | head -n1 )
    passcode=$( { jq -r '..|.join_code? // .verify_code? // .code? // .passcode? // empty' "$file" 2>/dev/null || true; } | head -n1 )
  else
    session=$(json_value_python "$file" session_id sessionId)
    passcode=$(json_value_python "$file" join_code verify_code code passcode)
  fi
  [[ -n "$session" || -n "$passcode" ]] || return 1
  printf "%-6s %-24s %s\n" "$role" "${session:-(-)}" "${passcode:-(-)}"
}

print_from_log() {
  local file="$1" role="$2"
  [[ -f "$file" ]] || return 1
  local session passcode
  session=$(grep -Eio 'session[_ ]id[:=][[:space:]]*[A-Za-z0-9_-]+' "$file" | head -n1 | sed -E 's/.*[:=][[:space:]]*//') || true
  [[ -z "$session" ]] && session=$(grep -Eio 'sess-[A-Za-z0-9_-]+' "$file" | head -n1) || true
  passcode=$(grep -Eio '(join|verify|pass)code[:=][[:space:]]*[0-9]{4,8}' "$file" | head -n1 | sed -E 's/.*[:=][[:space:]]*//') || true
  [[ -z "$passcode" ]] && passcode=$(grep -Eo '[0-9]{6}' "$file" | head -n1) || true
  [[ -n "$session" || -n "$passcode" ]] || return 1
  printf "%-6s %-24s %s\n" "$role" "${session:-(-)}" "${passcode:-(-)}"
}

printf '%s\n' "role   session_id               passcode"
printf '%s\n' "-----  ------------------------ --------"
for role in "${roles[@]}"; do
  json_file="$LOG_DIR/bootstrap-$role.json"
  log_file="$LOG_DIR/beach-host-$role.log"
  if ! print_from_json "$json_file" "$role"; then
    if ! print_from_log "$log_file" "$role"; then
      printf "%-6s %-24s %s\n" "$role" "(-)" "(-)"
    fi
  fi
done
