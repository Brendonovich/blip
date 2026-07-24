import * as Alchemy from "alchemy";
import * as Cloudflare from "alchemy/Cloudflare";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";
import { Database } from "./src/database.ts";

export default Alchemy.Stack(
	"BlipServer",
	{
		providers: Cloudflare.providers(),
		state: Cloudflare.state(),
	},
	Effect.gen(function* () {
		const database = yield* Database;
		const isDev = (yield* Alchemy.ALCHEMY_DEV) || process.env.NODE_ENV === "development" || process.argv.includes("dev");
		const publicOrigin = isDev
			? "http://localhost:1337"
			: yield* Config.nonEmptyString("BLIP_PUBLIC_ORIGIN");
		const worker = yield* Cloudflare.Website.Vite("Worker", {
			url: true,
			compatibility: { flags: ["nodejs_compat"] },
			assets: { runWorkerFirst: false },
			dev: { port: 1337, strictPort: true },
			env: {
				DB: database,
				BETTER_AUTH_SECRET: Config.redacted("BETTER_AUTH_SECRET"),
				BLIP_ENCRYPTION_KEY: Config.redacted("BLIP_ENCRYPTION_KEY"),
				BLIP_PUBLIC_ORIGIN: publicOrigin,
				BLIP_SETUP_TOKEN: Config.redacted("BLIP_SETUP_TOKEN"),
				GITHUB_CLIENT_ID: Config.nonEmptyString("GITHUB_CLIENT_ID"),
				GITHUB_CLIENT_SECRET: Config.redacted("GITHUB_CLIENT_SECRET"),
			},
		});
		return { url: worker.url };
	}),
);
