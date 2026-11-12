#!/usr/bin/env bash
# Calls one tool on a running core and prints the text result.
#   scripts/call.sh TOOL ['{"json":"arguments"}'] [SCOPES]
# URL (default http://127.0.0.1:8090/mcp) and INTERNAL_TOKEN come from the environment.
set -euo pipefail
tool=$1 args=${2:-'{}'} scopes=${3:-pim:read mail:write mail:send contacts:write calendar:write}
url=${PIM_URL:-http://127.0.0.1:8090/mcp}
if [ "$tool" = list ]; then
  body='{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
elif [ "$tool" = init ]; then
  body='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"call.sh","version":"0"}}}'
else
  body=$(jq -cn --arg n "$tool" --argjson a "$args" '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:$n,arguments:$a}}')
fi
curl -sS -X POST "$url" -H "Authorization: Bearer ${INTERNAL_TOKEN:-dev-internal-token-0123456789}" \
  -H 'Content-Type: application/json' -H "X-CoP-Scopes: $scopes" -d "$body" |
  jq -r 'if .result.content then (if .result.isError then "ERROR: " else "" end) + .result.content[0].text
         elif .result.tools then (.result.tools[] | "\(.name) [\(.annotations.readOnlyHint|if . then "read" else "write" end)]")
         elif .result.instructions then .result.instructions else . end'
