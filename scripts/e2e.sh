#!/usr/bin/env bash
# End-to-end check of every tool against a running core. It creates, changes and removes its own
# items, all tagged pim-e2e-<time>, so point it at accounts where that is acceptable:
#   PIM_E2E_DAV_ACCOUNT    a calendar and contacts account id that may be written to (required);
#                          ideally a throwaway account
#   PIM_E2E_MAIL_ACCOUNT   a mail account id, and
#   PIM_E2E_MAIL_ADDRESS   an address of that account that delivers to its own INBOX. The mail
#                          part sends to itself, touches only its own tagged messages and leaves
#                          them in Trash. Leave both unset to skip mail.
#   PIM_URL, INTERNAL_TOKEN   as for scripts/call.sh
# The server's [mail.policy] send_to, if set, must allow that address and not someone@example.org.
set -uo pipefail
cd "$(dirname "$0")/.."
: "${PIM_E2E_DAV_ACCOUNT:?set PIM_E2E_DAV_ACCOUNT to a calendar and contacts account that may be written to}"
export PIM_E2E_DAV_ACCOUNT
D=$PIM_E2E_DAV_ACCOUNT
c=scripts/call.sh pass=0 fail=0
tag="pim-e2e-$(date +%s)"
ok() { pass=$((pass + 1)); echo "PASS $1"; }
no() { fail=$((fail + 1)); echo "FAIL $1"; echo "     ${2:0:300}"; }
expect() { # name, pattern, output
  if grep -q -- "$2" <<<"$3" && ! grep -q '^ERROR' <<<"$3"; then ok "$1"; else no "$1" "$3"; fi
}
field() { grep -o "$1: [^ ]*" <<<"$2" | head -1 | cut -d' ' -f2-; }

# --- scopes
out=$($c create_task '{"summary":"x"}' 'pim:read'); grep -q 'not granted' <<<"$out" && ok "write refused without scope" || no "write refused without scope" "$out"

# --- tasks
cal=$($c list_calendars "$(jq -cn '{account:$ENV.PIM_E2E_DAV_ACCOUNT}')" | grep 'events and tasks' | grep -o "id $D:[^ ]*" | head -1 | cut -d' ' -f2)
calname=$($c list_calendars "$(jq -cn '{account:$ENV.PIM_E2E_DAV_ACCOUNT}')" | grep -F "id $cal " | sed -E 's/^  (.*) \| id .*/\1/')
[ -n "$cal" ] && ok "found a calendar for events and tasks ($calname)" || { no "found a calendar for events and tasks" "account $D has none"; exit 1; }
out=$($c create_task "$(jq -cn --arg c "$cal" --arg s "$tag task" '{calendar:$c,summary:$s,due:"2026-12-01",priority:5}')"); expect "create_task" "Task created" "$out"
tid=$(field id "$out"); etag=$(field etag "$out")
out=$($c list_tasks "$(jq -cn --arg c "$cal" --arg q "$tag" '{calendar:$c,query:$q}')"); expect "list_tasks finds it" "$tag task | due 2026-12-01" "$out"
out=$($c get_item "$(jq -cn --arg i "$tid" '{id:$i}')"); expect "get_item task" "SUMMARY: $tag task" "$out"; etag=$(field etag "$out")
out=$($c update_task "$(jq -cn --arg i "$tid" --arg e "stale-etag" '{id:$i,etag:$e,status:"completed"}')"); grep -q 'changed since' <<<"$out" && ok "stale etag refused" || no "stale etag refused" "$out"
out=$($c update_task "$(jq -cn --arg i "$tid" --arg e "$etag" '{id:$i,etag:$e,status:"completed",summary:"'"$tag"' task done"}')"); expect "update_task complete" "Task updated" "$out"
out=$($c list_tasks "$(jq -cn --arg c "$cal" --arg q "$tag" '{calendar:$c,query:$q,status:"completed"}')"); expect "completed task listed" "\[x\] $tag task done" "$out"
out=$($c get_item "$(jq -cn --arg i "$tid" '{id:$i}')"); etag=$(field etag "$out")
out=$($c delete_calendar_item "$(jq -cn --arg i "$tid" --arg e "$etag" '{id:$i,etag:$e}')"); expect "delete task" "Deleted" "$out"

# --- events
out=$($c create_event "$(jq -cn --arg c "$cal" --arg s "$tag event" '{calendar:$c,summary:$s,start:"2026-12-02T14:30",end:"2026-12-02T15:15",location:"Test Room; B",description:"line one\nline two"}')"); expect "create_event" "Event created" "$out"
eid=$(field id "$out")
out=$($c list_events "$(jq -cn --arg c "$cal" --arg q "$tag" '{calendar:$c,start:"2026-12-02",end:"2026-12-03",query:$q}')"); expect "list_events shows local time" "2026-12-02 14:30 PST to 2026-12-02 15:15 PST | $tag event | at Test Room; B" "$out"
out=$($c get_item "$(jq -cn --arg i "$eid" '{id:$i}')"); etag=$(field etag "$out")
out=$($c update_event "$(jq -cn --arg i "$eid" --arg e "$etag" '{id:$i,etag:$e,start:"2026-12-02T16:00",end:"2026-12-02T17:00",location:""}')"); expect "update_event" "Event updated" "$out"
out=$($c list_events "$(jq -cn --arg c "$cal" --arg q "$tag" '{calendar:$c,start:"2026-12-02",end:"2026-12-03",query:$q}')"); expect "event moved, location cleared" "2026-12-02 16:00 PST to 2026-12-02 17:00 PST | $tag event | $calname |" "$out"
out=$($c create_event "$(jq -cn --arg c "$cal" --arg s "$tag weekly" '{calendar:$c,summary:$s,start:"2026-12-07T09:00",rrule:"FREQ=WEEKLY;COUNT=3"}')"); expect "create recurring event" "Event created" "$out"
rid=$(field id "$out")
out=$($c list_events "$(jq -cn --arg c "$cal" --arg q "$tag weekly" '{calendar:$c,start:"2026-12-01",end:"2027-01-15",query:$q}')"); [ "$(grep -c "$tag weekly" <<<"$out")" = 3 ] && ok "recurring event expands to 3" || no "recurring event expands to 3" "$out"
for i in "$eid" "$rid"; do
  out=$($c get_item "$(jq -cn --arg i "$i" '{id:$i}')"); etag=$(field etag "$out")
  out=$($c delete_calendar_item "$(jq -cn --arg i "$i" --arg e "$etag" '{id:$i,etag:$e}')"); expect "delete event" "Deleted" "$out"
done

# --- calendars: create, move a task in, refuse to delete while it holds something, delete
out=$($c create_calendar "$(jq -cn --arg n "$tag list" '{account:$ENV.PIM_E2E_DAV_ACCOUNT,name:$n,holds:"tasks"}')"); expect "create_calendar" "created in account $D" "$out"
newcal=$(field id "$out")
out=$($c list_calendars "$(jq -cn '{account:$ENV.PIM_E2E_DAV_ACCOUNT}')"); expect "new calendar listed as a task list" "$tag list | id $newcal | tasks" "$out"
out=$($c create_calendar "$(jq -cn --arg n "$tag list" '{account:$ENV.PIM_E2E_DAV_ACCOUNT,name:$n}')"); grep -q 'already has a calendar named' <<<"$out" && ok "duplicate calendar name refused" || no "duplicate calendar name refused" "$out"
out=$($c create_task "$(jq -cn --arg c "$cal" --arg s "$tag movable" '{calendar:$c,summary:$s}')"); mv_id=$(field id "$out")
out=$($c move_calendar_item "$(jq -cn --arg i "$mv_id" --arg t "$newcal" '{id:$i,to_calendar:$t}')"); expect "move_calendar_item" "Moved to $tag list" "$out"
moved=$(grep -o 'New id: [^ ]*' <<<"$out" | cut -d' ' -f3)
out=$($c list_tasks "$(jq -cn --arg c "$newcal" '{calendar:$c}')"); expect "task is in the new list" "$tag movable" "$out"
out=$($c list_tasks "$(jq -cn --arg c "$cal" --arg q "$tag movable" '{calendar:$c,query:$q}')"); grep -q '^0 task' <<<"$out" && ok "task left the old calendar" || no "task left the old calendar" "$out"
out=$($c create_event "$(jq -cn --arg c "$newcal" '{calendar:$c,summary:"x",start:"2026-12-02"}')"); grep -q '^ERROR' <<<"$out" && ok "tasks-only list refuses an event" || no "tasks-only list refuses an event" "$out"
out=$($c delete_calendar "$(jq -cn --arg i "$newcal" '{id:$i}')"); grep -q 'still holds 0 event(s) and 1 task(s)' <<<"$out" && ok "delete_calendar refused while not empty" || no "delete_calendar refused while not empty" "$out"
out=$($c delete_calendar "$(jq -cn --arg i "${newcal%/*/}/" '{id:$i}')"); grep -q 'is not one of account' <<<"$out" && ok "delete_calendar refuses a path that is not a calendar" || no "delete_calendar refuses the home" "$out"
out=$($c move_calendar_item "$(jq -cn --arg i "$moved" --arg t "$cal" '{id:$i,to_calendar:$t}')"); expect "move back" "Moved to" "$out"
back=$(grep -o 'New id: [^ ]*' <<<"$out" | cut -d' ' -f3)
out=$($c delete_calendar "$(jq -cn --arg i "$newcal" '{id:$i}')"); expect "delete_calendar when empty" "Deleted calendar $tag list with 0 event(s) and 0 task(s)" "$out"
out=$($c get_item "$(jq -cn --arg i "$back" '{id:$i}')"); etag=$(field etag "$out")
out=$($c delete_calendar_item "$(jq -cn --arg i "$back" --arg e "$etag" '{id:$i,etag:$e}')"); expect "tidy moved task" "Deleted" "$out"

# --- contacts
out=$($c create_contact "$(jq -cn --arg n "$tag Person" '{account:$ENV.PIM_E2E_DAV_ACCOUNT,full_name:$n,emails:["a@example.com","b@example.com"],phones:["+1 555 0100"],org:"Test, Inc.",note:"first note",categories:["PIM Test"]}')"); expect "create_contact" "Contact created" "$out"
cid=$(field id "$out")
out=$($c search_contacts "$(jq -cn --arg q "$tag" '{account:$ENV.PIM_E2E_DAV_ACCOUNT,query:$q}')"); expect "search_contacts finds it" "$tag Person | email a@example.com, b@example.com | phone +1 555 0100 | org Test, Inc." "$out"
out=$($c get_item "$(jq -cn --arg i "$cid" '{id:$i}')"); etag=$(field etag "$out")
out=$($c update_contact "$(jq -cn --arg i "$cid" --arg e "$etag" '{id:$i,etag:$e,emails:["c@example.com"],title:"Tester"}')"); expect "update_contact" "Contact updated" "$out"
out=$($c get_item "$(jq -cn --arg i "$cid" '{id:$i}')"); etag=$(field etag "$out")
if grep -q 'EMAIL.*c@example.com' <<<"$out" && ! grep -q 'a@example.com' <<<"$out" && grep -q 'NOTE: first note' <<<"$out" && grep -q 'TEL: +1 555 0100' <<<"$out"; then ok "update kept other fields"; else no "update kept other fields" "$out"; fi
out=$($c delete_contact "$(jq -cn --arg i "$cid" --arg e "$etag" '{id:$i,etag:$e}')"); expect "delete_contact" "Deleted" "$out"

# --- mail: one mailbox writing to itself. It touches only the messages it creates, which all
# carry $tag in the subject and end in Trash.
A=${PIM_E2E_MAIL_ACCOUNT:-} me=${PIM_E2E_MAIL_ADDRESS:-}
if [ -z "$A" ] || [ -z "$me" ]; then
  echo "SKIP mail (PIM_E2E_MAIL_ACCOUNT and PIM_E2E_MAIL_ADDRESS are not set)"
  echo "---- $pass passed, $fail failed"
  [ "$fail" = 0 ]; exit
fi
find_in() { # mailbox, subject, extra-json -> first id
  $c search_emails "$(jq -cn --arg a "$A" --arg b "$1" --arg s "$2" --argjson x "${3:-{\}}" '{account:$a,mailbox:$b,subject:$s} + $x')" |
    grep -o "id $A:$1:[0-9]*" | head -1 | cut -d' ' -f2-
}
boxes=$($c list_mailboxes "$(jq -cn --arg a "$A" '{account:$a}')")
sent=$(grep '\[sent\]' <<<"$boxes" | sed -E 's/^  (.*) \[sent\].*/\1/'); trash=$(grep '\[trash\]' <<<"$boxes" | sed -E 's/^  (.*) \[trash\].*/\1/')
drafts=$(grep '\[drafts\]' <<<"$boxes" | sed -E 's/^  (.*) \[drafts\].*/\1/'); archive=$(grep '\[archive\]' <<<"$boxes" | sed -E 's/^  (.*) \[archive\].*/\1/')
[ -n "$sent" ] && [ -n "$trash" ] && [ -n "$drafts" ] && ok "special folders found ($sent, $drafts, $trash)" || no "special folders found" "$boxes"
out=$($c send_email "$(jq -cn --arg a "$A" --arg s "$tag hello" '{account:$a,to:["someone@example.org"],subject:$s,body:"x"}')"); grep -q 'send policy' <<<"$out" && ok "send policy blocks outsiders" || no "send policy blocks outsiders" "$out"
out=$($c send_email "$(jq -cn --arg a "$A" --arg s "$tag hello" --arg t "$me" '{account:$a,from:"evil@example.org",to:[$t],subject:$s,body:"x"}')"); grep -q 'may only send as' <<<"$out" && ok "foreign From refused" || no "foreign From refused" "$out"
out=$($c send_email "$(jq -cn --arg a "$A" --arg s "$tag hello" --arg t "$me" '{account:$a,to:[$t],subject:$s,body:"First line.\nSecond line with unicode: café ✓"}')"); expect "send_email" "Sent from" "$out"
mid=""
for _ in $(seq 1 30); do mid=$(find_in INBOX "$tag hello"); [ -n "$mid" ] && break; sleep 2; done
[ -n "$mid" ] && ok "message delivered and found by search" || no "message delivered and found by search" "not in INBOX after 60 s"
out=$($c read_email "$(jq -cn --arg i "$mid" '{id:$i}')"); expect "read_email" "café ✓" "$out"
grep -q '<untrusted_content>' <<<"$out" && ok "body marked untrusted" || no "body marked untrusted" "$out"
out=$($c read_attachment "$(jq -cn --arg i "$mid" '{id:$i,index:0}')"); grep -q 'has no attachments' <<<"$out" && ok "read_attachment says a plain message has none" || no "read_attachment on a plain message" "$out"
[ -n "$(find_in "$sent" "$tag hello")" ] && ok "copy filed in $sent" || no "copy filed in $sent" ""
out=$($c update_email_flags "$(jq -cn --arg i "$mid" '{ids:[$i],read:true,flagged:true}')"); expect "update_email_flags" "Updated 1" "$out"
[ -n "$(find_in INBOX "$tag hello" '{"flagged_only":true}')" ] && ok "flag visible in search" || no "flag visible in search" ""
out=$($c create_draft "$(jq -cn --arg i "$mid" '{reply_to_id:$i,subject:"",body:"draft reply"}')"); expect "create_draft (reply)" "Draft saved in $drafts" "$out"
[ -n "$(find_in "$drafts" "Re: $tag hello")" ] && ok "draft is in $drafts with a threaded subject" || no "draft is in $drafts with a threaded subject" ""
out=$($c reply_email "$(jq -cn --arg i "$mid" '{id:$i,body:"Thanks, got it."}')"); expect "reply_email" "Sent from" "$out"
rid=""
for _ in $(seq 1 30); do rid=$(find_in INBOX "Re: $tag hello" '{"unread_only":true}'); [ -n "$rid" ] && break; sleep 2; done
out=$($c read_email "$(jq -cn --arg i "$rid" '{id:$i}')"); expect "reply arrives quoted" "> First line." "$out"
out=$($c search_emails "$(jq -cn --arg a "$A" --arg s "$tag hello" '{account:$a,subject:$s}')"); grep -q 'answered' <<<"$out" && ok "original marked answered" || no "original marked answered" "$out"
if [ -n "$archive" ]; then
  out=$($c move_emails "$(jq -cn --arg i "$rid" --arg d "$archive" '{ids:[$i],to_mailbox:$d}')"); expect "move_emails" "Moved 1" "$out"
  [ -n "$(find_in "$archive" "Re: $tag hello")" ] && ok "moved message is in $archive" || no "moved message is in $archive" ""
fi
out=$($c trash_emails "$(jq -cn --arg i "$mid" '{ids:[$i]}')"); expect "trash_emails" "to $trash" "$out"
# tidy: everything this run made goes to Trash
for box in INBOX "$archive" "$sent" "$drafts"; do
  [ -n "$box" ] || continue
  ids=$($c search_emails "$(jq -cn --arg a "$A" --arg b "$box" --arg s "$tag" '{account:$a,mailbox:$b,subject:$s}')" | grep -o "id $A:$box:[0-9]*" | cut -d' ' -f2- | jq -Rcn '[inputs]')
  [ "$ids" != "[]" ] && $c trash_emails "$(jq -cn --argjson i "$ids" '{ids:$i}')" >/dev/null
done
left=0
for box in INBOX "$archive" "$sent" "$drafts"; do [ -n "$box" ] && [ -n "$(find_in "$box" "$tag")" ] && left=$((left + 1)); done
n=$($c search_emails "$(jq -cn --arg a "$A" --arg b "$trash" --arg s "$tag" '{account:$a,mailbox:$b,subject:$s}')" | grep -c "id $A:$trash")
[ "$left" = 0 ] && [ "$n" -ge 5 ] && ok "all $n test messages ended in $trash, none left elsewhere" || no "test messages tidied" "left in $left folder(s), $n in trash"

echo "---- $pass passed, $fail failed"
[ "$fail" = 0 ]
