# Setting up Chance of PIM

Status: 0.1, early. This guide is accurate for what exists, but expect rough edges and read
[the roadmap](../README.md#roadmap) first. Allow about an hour.

You need: a Cloudflare account (free plan), a machine that runs containers (any Kubernetes or k3s
node; a Raspberry Pi 5 is enough), `kubectl`, Node 22+, and app passwords for your mail and
CalDAV/CardDAV accounts.

## 1. App passwords

Create one app password per account, named so you can revoke it later ("chance-of-pim"). Never
use an account's main password. The server needs IMAP and SMTP for mail, and CalDAV/CardDAV for
calendars, tasks and contacts.

## 2. config.toml

Copy [config.example.toml](../config.example.toml) and list your accounts. Each account names an
environment variable (`password_env`) instead of holding a password. While you try things out,
set `[mail.policy] send_to` to your own addresses so nothing can be sent anywhere else.

## 3. Tunnel and VPC service

From `auth/` (`npm ci` once, `npx wrangler login`):

```bash
npx wrangler tunnel create chance-of-pim
npx wrangler vpc service create chance-of-pim --type http --tunnel-id <TUNNEL_ID> --ipv4 127.0.0.1 --http-port 8080
```

Get the tunnel's token (dashboard: Networking > Tunnels > chance-of-pim, or the API's
`cfd_tunnel/<id>/token`) and save it:

```bash
mkdir -p ~/.config/chance-of-pim && chmod 700 ~/.config/chance-of-pim
# paste the token (it starts with eyJ) into:
$EDITOR ~/.config/chance-of-pim/tunnel-token
```

## 4. The pod

Write the passwords as `NAME=value` lines, one per `password_env` in your config:

```bash
$EDITOR ~/.config/chance-of-pim/accounts.env && chmod 600 ~/.config/chance-of-pim/accounts.env
```

Make an overlay directory with your `config.toml` and a `kustomization.yaml`:

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: chance-of-pim
resources:
  - https://github.com/tywrr/chance-of-pim//deploy/k8s/base?ref=v0.1.2
configMapGenerator:
  - name: chance-of-pim-config
    behavior: replace
    files: [config.toml]
```

Then, from a checkout of this repository:

```bash
deploy/k8s/apply.sh /path/to/your/overlay
```

It creates the namespace, builds two Secrets from `~/.config/chance-of-pim` (`chance-of-pim`:
tunnel token and an internal token it generates; `chance-of-pim-accounts`: your passwords),
applies the pod and waits for the tunnel and `/readyz`. The base NetworkPolicy lets the pod reach
DNS, HTTPS, the tunnel and mail ports (993, 465, 587) and nothing else; if a DAV server of yours
listens on another port, add a policy for it in your overlay.

On a plain Docker host, run the image with the same environment (`INTERNAL_TOKEN`, your
`password_env` variables, `CHANCE_OF_PIM_CONFIG`) next to a `cloudflared` container that shares
its network namespace.

## 5. The auth Worker

```bash
cd auth
cp wrangler.jsonc wrangler.personal.jsonc     # gitignored; put your VPC service id in place of <SERVICE_ID>
npx wrangler deploy -c wrangler.personal.jsonc --secrets-file <(printf 'INTERNAL_TOKEN=%s\nOWNER_PASSPHRASE=%s\nLINK_KEY=%s\n' \
  "$(cat ~/.config/chance-of-pim/internal-token)" "$(openssl rand -base64 24 | tee ~/.config/chance-of-pim/owner-passphrase)" "$(openssl rand -hex 32)")
```

Keep the passphrase somewhere safe (a password manager): it is what you type to connect a client,
and anyone who knows it can read your mail. [auth/README.md](../auth/README.md) explains the
sign-in and its limits.

## 6. Connect Claude

In Claude: Settings > Connectors > Add custom connector, URL
`https://chance-of-pim.<your-subdomain>.workers.dev/mcp`. On the sign-in page tick the permissions
you want and enter the passphrase. To change permissions later, disconnect and connect again.

## Telling Claude your conventions

`[guide] text` in config.toml is added to the instructions Claude gets when it connects. Use it
for things only you know: which calendar holds what, where finished mail goes, quirks of your
other apps.

## Checking it works

```bash
kubectl -n chance-of-pim port-forward deploy/chance-of-pim 18090:8080 &
export PIM_URL=http://127.0.0.1:18090/mcp INTERNAL_TOKEN=$(cat ~/.config/chance-of-pim/internal-token)
scripts/call.sh list                    # the tools
scripts/call.sh list_mailboxes          # your folders
```

`scripts/e2e.sh` exercises every tool; it writes to the accounts you name, so read its header
first.

## Troubleshooting

- **A tool says an account is unavailable:** the message carries the server's own error. A
  refused sign-in usually means the app password is wrong or `password_env` is missing from
  `accounts.env` (then: `deploy/k8s/apply.sh --rotate <overlay>`).
- **Claude can't connect:** the URL must end in `/mcp` and match the Worker's host exactly.
  `curl https://<host>/.well-known/oauth-protected-resource/mcp` should answer JSON.
- **502 from the Worker:** the pod or its tunnel is down: `kubectl -n chance-of-pim get pods`,
  `kubectl -n chance-of-pim logs deploy/chance-of-pim -c cloudflared`.
