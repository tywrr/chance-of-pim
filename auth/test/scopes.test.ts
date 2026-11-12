// pim:read and mail:write: what the owner grants on the consent page, and what reaches the core.

import { afterEach, describe, expect, it, vi } from "vitest";
import { grantedScopes } from "../src/config";
import {
	type Echo,
	ORIGIN,
	PASSPHRASE,
	authorizeUrl,
	call,
	freshIp,
	openSignIn,
	pkce,
	registerPublicClient,
	signIn,
	submitSignIn,
	token,
} from "./helpers";

function mcp(accessToken: string, headers: Record<string, string> = {}, overrides: Partial<Env> = {}): Promise<Response> {
	return call(
		"/mcp",
		{
			method: "POST",
			headers: { Authorization: `Bearer ${accessToken}`, "Content-Type": "application/json", ...headers },
			body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
		},
		freshIp(),
		overrides,
	);
}

async function forwardedHeaders(accessToken: string, headers: Record<string, string> = {}): Promise<Record<string, string>> {
	const response = await mcp(accessToken, headers);
	expect(response.status).toBe(200);
	return (await response.json<Echo>()).headers;
}

describe("consent page", () => {
	it("offers write access with an unticked checkbox", async () => {
		const client = await registerPublicClient();
		const { challenge } = await pkce();
		const path = authorizeUrl({ client_id: client.client_id, code_challenge: challenge, scope: "pim:read mail:write" });
		const { response, html } = await openSignIn(path, freshIp());
		expect(response.status).toBe(200);
		expect(html).toContain("Send email as you");
		const boxes = html.match(/<input type="checkbox"[^>]*>/g) ?? [];
		expect(boxes.map((box) => box.match(/value="([^"]*)"/)?.[1])).toEqual([
			"mail:write",
			"mail:send",
			"contacts:write",
			"calendar:write",
		]);
		for (const box of boxes) {
			expect(box).toContain('name="allow"');
			expect(box).not.toContain("checked");
		}
		// The scopes listed are what Allow grants as the page stands, so no mail:write yet.
		expect(html).not.toContain("<li><code>mail:write</code></li>");
	});

	it("comes back unticked after a wrong passphrase, even if the post ticked it", async () => {
		const client = await registerPublicClient();
		const { challenge } = await pkce();
		const path = authorizeUrl({ client_id: client.client_id, code_challenge: challenge });
		const wrong = await submitSignIn(path, freshIp(), { passphrase: "not-it", decision: "approve", allow: "mail:write" });
		expect(wrong.status).toBe(401);
		const checkbox = (await wrong.text()).match(/<input type="checkbox"[^>]*>/)?.[0] ?? "";
		expect(checkbox).toContain('name="allow"');
		expect(checkbox).not.toContain("checked");
	});

	it("ignores ticked boxes on a denial", async () => {
		const client = await registerPublicClient();
		const { challenge } = await pkce();
		const path = authorizeUrl({ client_id: client.client_id, code_challenge: challenge });
		const denied = await submitSignIn(path, freshIp(), { decision: "deny", allow: "mail:write", passphrase: PASSPHRASE });
		expect(denied.status).toBe(302);
		expect(new URL(denied.headers.get("Location") ?? "").searchParams.get("error")).toBe("access_denied");
	});
});

describe("granted scopes", () => {
	it("grants read only when the owner leaves the box unticked", async () => {
		const { tokens } = await signIn();
		expect(tokens.scope).toBe("pim:read");
		expect((await forwardedHeaders(tokens.access_token))["x-cop-scopes"]).toBe("pim:read");
	});

	it("grants read and write when the owner ticks the box", async () => {
		const { tokens } = await signIn({ scope: "pim:read mail:write", allowWrite: true });
		expect(tokens.scope).toBe("pim:read mail:write");
		expect((await forwardedHeaders(tokens.access_token))["x-cop-scopes"]).toBe("pim:read mail:write");
	});

	it("gives a client that asks for mail:write only read unless the owner ticks the box", async () => {
		const { tokens } = await signIn({ scope: "pim:read mail:write" });
		expect(tokens.scope).toBe("pim:read");
		expect((await forwardedHeaders(tokens.access_token))["x-cop-scopes"]).toBe("pim:read");
	});

	it("lets the owner give write to a client that asked only for read", async () => {
		const { tokens } = await signIn({ scope: "pim:read", allowWrite: true });
		expect(tokens.scope).toBe("pim:read mail:write");
	});

	it("keeps offline_access on the grant but forwards only file scopes", async () => {
		const { tokens } = await signIn({ scope: "pim:read offline_access", allowWrite: true });
		expect(tokens.scope.split(" ").sort()).toEqual(["mail:write", "offline_access", "pim:read"]);
		expect((await forwardedHeaders(tokens.access_token))["x-cop-scopes"]).toBe("pim:read mail:write");
	});

	it("keeps the scopes across refreshes", async () => {
		const { client, tokens } = await signIn({ allowWrite: true });
		let refreshToken = tokens.refresh_token;
		for (let round = 0; round < 2; round += 1) {
			const refreshed = await token({ grant_type: "refresh_token", refresh_token: refreshToken, client_id: client.client_id });
			expect(refreshed.status).toBe(200);
			const next = await refreshed.json<{ access_token: string; refresh_token: string; scope: string }>();
			expect(next.scope).toBe("pim:read mail:write");
			expect((await forwardedHeaders(next.access_token))["x-cop-scopes"]).toBe("pim:read mail:write");
			refreshToken = next.refresh_token;
		}
	});

	it("forwards a refresh's narrower scope, and refuses a token without pim:read", async () => {
		const { client, tokens } = await signIn({ scope: "pim:read offline_access", allowWrite: true });
		const refresh = (refreshToken: string, scope: string) =>
			token({ grant_type: "refresh_token", refresh_token: refreshToken, client_id: client.client_id, scope });

		const readOnly = await refresh(tokens.refresh_token, "pim:read");
		expect(readOnly.status).toBe(200);
		const narrowed = await readOnly.json<{ access_token: string; refresh_token: string }>();
		expect((await forwardedHeaders(narrowed.access_token))["x-cop-scopes"]).toBe("pim:read");

		const noRead = await refresh(narrowed.refresh_token, "offline_access");
		expect(noRead.status).toBe(200);
		const { access_token } = await noRead.json<{ access_token: string }>();
		const refused = await mcp(access_token);
		expect(refused.status).toBe(403);
		expect(refused.headers.get("WWW-Authenticate")).toContain('error="insufficient_scope"');
		expect(refused.headers.get("WWW-Authenticate")).toContain('scope="pim:read"');
	});
});

describe("headers to the core", () => {
	it("strips every client-sent X-CoP-* header and sets X-CoP-Scopes and X-CoP-Public-Url", async () => {
		const { tokens } = await signIn();
		const headers = await forwardedHeaders(tokens.access_token, {
			"X-CoP-Scopes": "pim:read mail:write",
			"X-CoP-Public-Url": "https://evil.example",
			"x-cop-client-ip": "203.0.113.9",
			"X-COF-Anything": "1",
			"X-Other": "kept",
		});
		const cof = Object.keys(headers)
			.filter((name) => name.startsWith("x-cop-"))
			.sort();
		expect(cof).toEqual(["x-cop-public-url", "x-cop-scopes"]);
		expect(headers["x-cop-scopes"]).toBe("pim:read");
		expect(headers["x-cop-public-url"]).toBe(ORIGIN);
		expect(headers["x-other"]).toBe("kept");
	});

	it("sends /mcp to the core's own address through the binding", async () => {
		const { tokens } = await signIn();
		const response = await call(`/mcp?probe=1`, { method: "GET", headers: { Authorization: `Bearer ${tokens.access_token}` } });
		const echo = await response.json<Echo>();
		expect(echo.url).toBe("http://127.0.0.1:8080/mcp?probe=1");
		expect(echo.method).toBe("GET");
	});
});

describe("CORE_URL", () => {
	afterEach(() => {
		vi.restoreAllMocks();
	});

	it("sends /mcp to a loopback core instead of the binding", async () => {
		const { tokens } = await signIn();
		const seen: string[] = [];
		vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
			const request = new Request(input, init);
			seen.push(`${request.url} ${request.headers.get("Authorization")} ${request.headers.get("X-CoP-Scopes")}`);
			return new Response("local core");
		});
		const response = await mcp(tokens.access_token, {}, { CORE_URL: "http://127.0.0.1:18080" });
		expect(await response.text()).toBe("local core");
		expect(seen).toEqual(["http://127.0.0.1:18080/mcp Bearer test-internal-token pim:read"]);
	});

	it.each(["https://core.example.com", "http://core.example.com", "http://127.0.0.1:8080/base", "not a url"])(
		"refuses %s, which isn't a loopback http origin",
		async (coreUrl) => {
			const { tokens } = await signIn();
			const fetchSpy = vi.spyOn(globalThis, "fetch");
			const response = await mcp(tokens.access_token, {}, { CORE_URL: coreUrl });
			expect(response.status).toBe(500);
			expect(await response.text()).toContain("CORE_URL must be a loopback origin");
			expect(fetchSpy).not.toHaveBeenCalled();
		},
	);
});

describe("grantedScopes", () => {
	it("grants each write scope only when ticked, in a fixed order, and ignores unknown ticks", () => {
		expect(grantedScopes(["mail:send"], ["calendar:write", "mail:send", "admin"])).toEqual([
			"pim:read",
			"mail:send",
			"calendar:write",
		]);
	});

	it("always has pim:read, mail:write only when allowed, and keeps other supported scopes", () => {
		expect(grantedScopes([], [])).toEqual(["pim:read"]);
		expect(grantedScopes(["mail:write"], [])).toEqual(["pim:read"]);
		expect(grantedScopes([], ["mail:write"])).toEqual(["pim:read", "mail:write"]);
		expect(grantedScopes(["offline_access", "offline_access", "admin", "mail:write"], ["mail:write"])).toEqual([
			"pim:read",
			"mail:write",
			"offline_access",
		]);
	});
});
