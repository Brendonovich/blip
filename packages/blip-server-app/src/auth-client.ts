import { apiKeyClient } from "@better-auth/api-key/client";
import { createAuthClient } from "better-auth/solid";
import { getRequestEvent, isServer } from "solid-js/web";
import { getInMemoryAuthHandler } from "./in-memory-api.ts";

export const authClient = createAuthClient({
	plugins: [apiKeyClient()],
	fetchOptions: {
		customFetchImpl: async (input: RequestInfo | URL, init?: RequestInit) => {
			if (isServer) {
				const handler = getInMemoryAuthHandler();
				if (handler) {
					const url =
						typeof input === "string"
							? input
							: input instanceof URL
								? input.toString()
								: input.url;
					const event = getRequestEvent();
					const headers = new Headers(
						init?.headers ||
							(typeof input === "object" && "headers" in input
								? input.headers
								: undefined),
					);
					if (event?.request.headers) {
						for (const name of ["authorization", "cookie"]) {
							const val = event.request.headers.get(name);
							if (val && !headers.has(name)) headers.set(name, val);
						}
					}
					const reqUrl = url.startsWith("http")
						? url
						: new URL(
								url,
								event?.request.url || "http://localhost:3000",
							).toString();
					return handler(new Request(reqUrl, { ...init, headers }));
				}
			}
			return fetch(input, init);
		},
	},
});
