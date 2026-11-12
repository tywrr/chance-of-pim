# Chance of PIM

*Cloudy, with a chance of PIM:* your mail, contacts, calendars and tasks, raining down into
Claude's context from one connector you host yourself.

Sister project of [Chance of Files](https://github.com/tywrr/chance-of-files), built the same way:

- **The core**, a Rust server (`chance-of-pim`) in a pod next to `cloudflared`. It speaks IMAP and
  SMTP for mail and CalDAV/CardDAV for calendars, tasks and contacts, to any number of accounts,
  and holds the app passwords. Nothing listens on your network.
- **The auth Worker** (`auth/`), a TypeScript Worker on Cloudflare's free plan. It is the URL you
  add to Claude; it handles OAuth and forwards requests to the core through a Workers VPC service
  and a Cloudflare Tunnel with no public hostname.

**Status: 0.1, early.** It runs daily against Runbox and Stalwart for its author, with Claude
and Claude Duet, but it is young: no local index yet (every call talks to the servers), no
attachments, and only the two providers above are tested. Other IMAP/SMTP and CalDAV/CardDAV
servers should work; reports are welcome.

Set it up: [docs/setup.md](docs/setup.md).

## Permissions

Reading is what every sign-in grants (`pim:read`). The owner ticks the rest on the sign-in page,
one box each: `mail:write` (flags, moves, Trash, drafts), `mail:send`, `contacts:write`,
`calendar:write`. Without a scope its tools are not listed. Every changing tool is marked
destructive, so Claude asks first. Mail is never deleted, only moved to Trash; calendar and
contact writes need the `etag` of the version they read.

Text written by other people (message bodies, event and contact fields) is returned inside
`<untrusted_content>` blocks, and the connector's instructions tell Claude to treat it as data.

## Tools

`pim_status` · mail: `list_mailboxes`, `search_emails`, `read_email`, `create_draft`,
`send_email`, `reply_email`, `update_email_flags`, `move_emails`, `trash_emails` · calendars and
tasks: `list_calendars`, `list_events`, `list_tasks`, `get_item`, `create_event`, `update_event`,
`create_task`, `update_task`, `delete_calendar_item`, `create_calendar`, `delete_calendar`,
`move_calendar_item` · contacts: `search_contacts`,
`create_contact`, `update_contact`, `delete_contact`.

Configuration: [config.example.toml](config.example.toml). `[guide] text` there is added to the
instructions Claude gets on connecting: say which calendar is for what, or any convention of yours.

## Roadmap

- Attachments: read small ones inline, larger ones by expiring link (the Worker's `/dl` and `/ul`
  routes are reserved for this); forward with attachments; a thread view.
- A local SQLite index with background sync, for fast search across accounts.
- Calendar depth: time zone definitions on recurring events, single-occurrence edits,
  invitations, free/busy.
- The stateless MCP revision (2026-07-28); today the server speaks the handshake revisions.
- Hidden folders and calendars, an audit log of writes, a setup page for adding accounts.
- JMAP as an optional mail backend.

## Develop

```bash
cargo test --workspace
INTERNAL_TOKEN=dev-internal-token-0123456789 MAIL_MAIN_PASSWORD=... cargo run -- --config my-config.toml
scripts/call.sh list                    # against http://127.0.0.1:8090/mcp by default: set PIM_URL
cd auth && npm ci && npm test           # the Worker; scripts/oauth-e2e.sh drives a local sign-in
deploy/build-image.sh --arch amd64      # static musl image (needs mise for cargo-zigbuild and zig)
```

Independent and self-hosted; not affiliated with Anthropic, Runbox or Stalwart Labs.
