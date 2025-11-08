#!/usr/bin/env bash
set -euo pipefail

LOG_DIR=${LOG_DIR:-"$HOME/beach-debug"}
roles=(lhs rhs agent)

print_from_json() {
  local file="$1" role="$2"
  [[ -f "$file" ]] || return 1
  local session passcode
  session=$( { jq -r '..|.session_id? // .sessionId? // empty' "$file" 2>/dev/null || true; } | head -n1 )
  passcode=$( { jq -r '..|.join_code? // .verify_code? // .code? // .passcode? // empty' "$file" 2>/dev/null || true; } | head -n1 )
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
  [[ -z "$passcode" ]] && passcode=$(grep -Eio '[[:<:]][0-9]{6}[[:>:]]' "$file" | head -n1) || true
  [[ -n "$session" || -n "$passcode" ]] || return 1
  printf "%-6s %-24s %s\n" "$role" "${session:-(-)}" "${passcode:-(-)}"
}

if ! command -v jq >/dev/null 2>&1; then
  echo "Error: jq is required to parse bootstrap JSON files." >&2
  exit 1
fi

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
