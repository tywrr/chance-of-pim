#!/usr/bin/env bash
# Signs in through a LOCAL auth Worker the way Claude does (register, consent page, passphrase,
# PKCE code exchange), then calls the core through it. Checks that the scopes the owner ticks are
# the ones the core sees.
#   terminal 1: source scripts/dev-env.sh && <target>/debug/chance-of-pim --config dev/config.toml
#   terminal 2: cd auth && cp .dev.vars.example .dev.vars   (set CORE_URL=http://127.0.0.1:8090
#               and INTERNAL_TOKEN to the core's) && npx wrangler dev --local
#   scripts/oauth-e2e.sh
# Local only: it uses the dev passphrase from auth/.dev.vars, and a deployed Worker refuses client
# registration from outside loopback anyway.
set -uo pipefail
W=${PIM_AUTH_URL:-http://127.0.0.1:8788}
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "PASS $1"; }
no() { fail=$((fail + 1)); echo "FAIL $1"; echo "     ${2:0:300}"; }
case $W in http://127.0.0.1:* | http://localhost:*) ;; *) echo "local Workers only" >&2; exit 2 ;; esac
passphrase=$(grep '^OWNER_PASSPHRASE=' "$(dirname "$0")/../auth/.dev.vars" | cut -d= -f2-)
cb=http://127.0.0.1:9/callback
b64() { basenc --base64url | tr -d '=\n'; }

meta=$(curl -s "$W/.well-known/oauth-protected-resource/mcp")
[ "$(jq -r .resource <<<"$meta")" = "$W/mcp" ] && ok "resource metadata names this /mcp" || no "resource metadata" "$meta"
[ "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$W/mcp")" = 401 ] && ok "/mcp refuses a request without a token" || no "/mcp without token" ""

client=$(curl -s -X POST "$W/register" -H 'Content-Type: application/json' \
  -d "$(jq -cn --arg cb "$cb" '{client_name:"oauth-e2e",redirect_uris:[$cb],token_endpoint_auth_method:"none",grant_types:["authorization_code","refresh_token"],response_types:["code"]}')")
cid=$(jq -r '.client_id // empty' <<<"$client")
[ -n "$cid" ] && ok "client registered" || { no "client registered" "$client"; exit 1; }

# sign_in "space separated ticks" -> access token
sign_in() {
  local verifier challenge url page loc code ticks=()
  verifier=$(openssl rand 32 | b64)
  challenge=$(printf '%s' "$verifier" | openssl dgst -sha256 -binary | b64)
  url="$W/authorize?response_type=code&client_id=$cid&redirect_uri=$(jq -rn --arg v "$cb" '$v|@uri')&scope=pim%3Aread&state=s1&code_challenge=$challenge&code_challenge_method=S256&resource=$(jq -rn --arg v "$W/mcp" '$v|@uri')"
  page=$(curl -s "$url")
  grep -q 'name="allow" value="mail:send"' <<<"$page" || { echo "consent page lacks the tick boxes" >&2; return 1; }
  for t in $1; do ticks+=(--data-urlencode "allow=$t"); done
  loc=$(curl -s -o /dev/null -w '%{redirect_url}' -X POST "$url" --data-urlencode "passphrase=$passphrase" --data-urlencode "decision=approve" "${ticks[@]}")
  code=$(sed -n 's/.*[?&]code=\([^&]*\).*/\1/p' <<<"$loc" | python3 -c 'import sys, urllib.parse; print(urllib.parse.unquote(sys.stdin.read().strip()))')
  [ -n "$code" ] || { echo "no code in redirect: $loc" >&2; return 1; }
  curl -s -X POST "$W/token" --data-urlencode grant_type=authorization_code --data-urlencode "code=$code" \
    --data-urlencode "redirect_uri=$cb" --data-urlencode "client_id=$cid" --data-urlencode "code_verifier=$verifier" \
    --data-urlencode "resource=$W/mcp"
}
rpc() { # token, json
  curl -s -X POST "$W/mcp" -H "Authorization: Bearer $1" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d "$2"
}

tok=$(sign_in "")
at=$(jq -r '.access_token // empty' <<<"$tok")
[ -n "$at" ] && [ "$(jq -r .scope <<<"$tok")" = "pim:read" ] && ok "sign-in with nothing ticked grants pim:read only" || no "read-only sign-in" "$tok"
out=$(rpc "$at" '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"oauth-e2e","version":"0"}}}')
grep -q 'This connection may: pim:read\.' <<<"$(jq -r .result.instructions <<<"$out")" && ok "initialize through the Worker; core sees pim:read" || no "initialize" "$out"
n=$(rpc "$at" '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | jq '[.result.tools[] | select(.annotations.readOnlyHint | not)] | length')
[ "$n" = 0 ] && ok "read-only grant lists no write tools" || no "read-only tool list" "$n write tools"
out=$(rpc "$at" '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"create_task","arguments":{"summary":"x"}}}')
grep -q 'not granted' <<<"$out" && ok "write tool refused on a read-only grant" || no "write refused" "$out"
out=$(rpc "$at" '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_calendars","arguments":{}}}')
grep -q 'Account ' <<<"$(jq -r '.result.content[0].text' <<<"$out")" && ok "read tool works through the Worker" || no "list_calendars" "$out"
out=$(curl -s -X POST "$W/mcp" -H "Authorization: Bearer $at" -H 'Content-Type: application/json' -H 'X-CoP-Scopes: pim:read mail:send' -d '{"jsonrpc":"2.0","id":5,"method":"tools/list"}')
[ "$(jq '[.result.tools[] | select(.name=="send_email")] | length' <<<"$out")" = 0 ] && ok "a client cannot raise its scopes with its own X-CoP-Scopes header" || no "client-sent scopes header" "$out"

tok=$(sign_in "calendar:write mail:write")
at=$(jq -r '.access_token // empty' <<<"$tok")
[ "$(jq -r '.scope | split(" ") | sort | join(" ")' <<<"$tok")" = "calendar:write mail:write pim:read" ] && ok "ticking two boxes grants exactly those" || no "two ticks" "$tok"
names=$(rpc "$at" '{"jsonrpc":"2.0","id":6,"method":"tools/list"}' | jq -r '[.result.tools[].name] | join(" ")')
if grep -qw create_task <<<"$names" && grep -qw create_draft <<<"$names" && ! grep -qw send_email <<<"$names" && ! grep -qw create_contact <<<"$names"; then
  ok "tool list follows the ticks (tasks and drafts, no send, no contact edits)"
else no "tool list follows the ticks" "$names"; fi

echo "---- $pass passed, $fail failed"
[ "$fail" = 0 ]
