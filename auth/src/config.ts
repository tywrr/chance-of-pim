// Fixed OAuth settings shared by the provider and the sign-in page.

import type { OAuthHelpers } from "@cloudflare/workers-oauth-provider";

/** The Worker's bindings plus the helpers the OAuth library adds before calling our handlers. */
export type AppEnv = Env & { OAUTH_PROVIDER: OAuthHelpers };

/** Read access to mail, contacts, calendars and tasks: what every grant carries. */
export const READ_SCOPE = "pim:read";

/**
 * The permissions the owner may add on the consent page, one tick box each, in the order shown.
 * Granted only when ticked; the core hides and refuses the matching tools without them.
 */
export const WRITE_SCOPES: readonly { scope: string; label: string; detail: string }[] = [
	{
		scope: "mail:write",
		label: "Organize mail and save drafts",
		detail: "Mark, move, send to Trash, and save drafts for you to send yourself. Nothing is deleted for good.",
	},
	{
		scope: "mail:send",
		label: "Send email as you",
		detail: "Send and reply from your addresses. Claude asks before each one.",
	},
	{
		scope: "contacts:write",
		label: "Change contacts",
		detail: "Create, edit and delete contacts.",
	},
	{
		scope: "calendar:write",
		label: "Change calendars and tasks",
		detail: "Create, edit and delete events and tasks.",
	},
];

const WRITE_SCOPE_NAMES = WRITE_SCOPES.map((w) => w.scope);

/** The scopes the core understands; only these are forwarded to it in X-CoP-Scopes. */
export const CORE_SCOPES = [READ_SCOPE, ...WRITE_SCOPE_NAMES];

/**
 * Everything this server can grant. offline_access is listed because Claude adds it to its
 * request when advertised; refresh tokens are issued either way.
 */
export const SCOPES_SUPPORTED = [...CORE_SCOPES, "offline_access"];

/** One user: the owner. Grants are stored under this ID. */
export const OWNER_USER_ID = "owner";

export const ACCESS_TOKEN_TTL = 60 * 60;
/** A grant lasts 30 days from sign-in and is extended by 30 days on each refresh. */
export const REFRESH_TOKEN_TTL = 30 * 24 * 60 * 60;
export const REFRESH_TOKEN_IDLE_TTL = 30 * 24 * 60 * 60;

/**
 * The scopes approving a request grants: the read scope always, each write scope exactly when the
 * owner ticked it (whatever the client asked for, so a client can't get one by asking and the
 * owner can give one to a client that only asked for read), and any other supported scope asked
 * for.
 */
export function grantedScopes(requested: readonly string[], ticked: readonly string[]): string[] {
	const others = requested.filter((scope) => SCOPES_SUPPORTED.includes(scope) && !CORE_SCOPES.includes(scope));
	const writes = WRITE_SCOPE_NAMES.filter((scope) => ticked.includes(scope));
	return [READ_SCOPE, ...writes, ...new Set(others)];
}
