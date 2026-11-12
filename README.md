# Chance of PIM

*Cloudy, with a chance of PIM:* your mail, contacts, calendars and tasks, raining down into
Claude's context from one connector you host yourself.

It was built mainly for **Claude Duet**: Duet runs on an ephemeral cloud computer that can't reach
your home network, keep mail/DAV credentials, or run local CLI tools (like `himalaya`) and stdio
MCP servers—and its outbound network only reaches HTTP/HTTPS allowlisted hosts, not IMAP or SMTP
ports. Hosting a remote MCP connector behind an OAuth Worker bridges that gap, and works anywhere
remote MCP over Streamable HTTP is supported: tested with Claude Duet, claude.ai, the Claude apps,
and Claude Code, and compatible in principle (untested) with other MCP clients and ambient agents
like ChatGPT / ChatGPT Dots, Gemini Tasks, Grok Bot, Cursor, VS Code, or Codex (local loopback
clients work out of the box; hosted services need their callback URL added to
`ALLOWED_REDIRECT_URIS` in `auth/`).

Sister project to [Chance of Files](https://github.com/tywrr/chance-of-files) (pCloud files,
drive audits, and expiring transfer links), built the same way:

- **The core**, a Rust server (`chance-of-pim`) in a pod next to `cloudflared`. It speaks IMAP and
  SMTP for mail and CalDAV/CardDAV for calendars, tasks and contacts, to any number of accounts,
  and holds the app passwords. Nothing listens on your network.
- **The auth Worker** (`auth/`), a TypeScript Worker on Cloudflare's free plan. It is the URL you
  add to Claude; it handles OAuth and forwards requests to the core through a Workers VPC service
  and a Cloudflare Tunnel with no public hostname.

**Status: 0.1, early.** It runs daily against Runbox and Stalwart for its author, with Claude
and Claude Duet, but it is young: no local index yet (every call talks to the servers),
attachments can be read but not sent, and only the two providers above are tested. Other IMAP/SMTP and CalDAV/CardDAV
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

`pim_status` · mail: `list_mailboxes`, `search_emails`, `read_email`, `read_attachment`,
`create_draft`,
`send_email`, `reply_email`, `update_email_flags`, `move_emails`, `trash_emails` · calendars and
tasks: `list_calendars`, `list_events`, `list_tasks`, `get_item`, `create_event`, `update_event`,
`create_task`, `update_task`, `delete_calendar_item`, `create_calendar`, `delete_calendar`,
`move_calendar_item` · contacts: `search_contacts`,
`create_contact`, `update_contact`, `delete_contact`.

### Attachments

`read_attachment` opens one attachment of a message in place:

- **Pictures** (JPEG, PNG, GIF, WebP) come back as an image Claude can look at, turned upright
  and scaled down to 1568 px on the long side when larger.
- **Spreadsheets** (xlsx, xlsm, xls, xlsb, ods) come back as tab-separated rows per sheet. A
  formula the sending program never calculated is shown as the formula, not as 0.
- **PDFs** come back as text, page by page. A scan with no text layer comes back one page at a
  time as a picture (when its pages are stored as JPEG, as most scanners do).
- **Word documents** (.docx), **CSV and other text files** come back as text.

Long text is paged with `offset`. Nothing is written anywhere; other formats (HEIC, archives,
legacy .doc, slides) wait for download links.

Configuration: [config.example.toml](config.example.toml). `[guide] text` there is added to the
instructions Claude gets on connecting: say which calendar is for what, or any convention of yours.

## Roadmap

- Attachments by expiring link, for files too big or of a kind `read_attachment` can't open
  (the Worker's `/dl` and `/ul` routes are reserved for this); sending and forwarding with
  attachments; a thread view.
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
