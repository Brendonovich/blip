# Blip Server Cloudflare

The Cloudflare deployment of Blip Server. It builds the shared
[`@blip/server-app`](../../packages/blip-server-app) with a D1-backed runtime
and deploys the Worker using Alchemy v2.

## Configure

Copy `.env.example` to `.env` and set:

```sh
openssl rand -base64 32    # BLIP_ENCRYPTION_KEY
openssl rand -base64 48    # BETTER_AUTH_SECRET
```

Set `BLIP_PUBLIC_ORIGIN` to the canonical HTTPS Worker or custom-domain origin.
Keep `BLIP_ENCRYPTION_KEY` stable because it encrypts every user's S3 credentials
with AES-256-GCM.
Keep `BETTER_AUTH_SECRET` stable so sessions remain valid across deploys.

Create a GitHub OAuth app and set `GITHUB_CLIENT_ID` and
`GITHUB_CLIENT_SECRET`. Its authorization callback URL must be:

```text
https://your-domain.example/api/auth/callback/github
```

Any GitHub user can sign in. Better Auth owns dashboard sessions and revocable
Capture API keys; each user's storage and recordings remain isolated by their
Better Auth user ID.

## Deploy

```sh
pnpm install
pnpm deploy
```

Alchemy provisions D1, builds the SolidStart app, and deploys its SSR pages,
Effect API routes, and client assets as one Worker. Use `pnpm dev` for local
development and `pnpm destroy` to remove the Cloudflare deployment.

## Database Migrations

Define schema changes in
[`packages/blip-server-app/src/schema.ts`](../../packages/blip-server-app/src/schema.ts).
Alchemy uses Drizzle to generate and apply pending D1 migrations from the shared
package during deployment. If Drizzle needs an interactive rename or data-loss
decision, generate and review the migration first:

```sh
pnpm db:generate
```

Commit the generated migration and snapshot in
`packages/blip-server-app/migrations` together. Never rename or modify an
applied migration; add a new migration by changing the shared schema instead.

When the public domain uses Vercel DNS instead of a Cloudflare zone, deploy
`vercel.json` as a Vercel project and assign the domain to that project. The
rewrite terminates TLS at Vercel and forwards requests to the Worker.

After deployment, follow the S3 setup instructions in the
[Node server documentation](../blip-server/README.md#configure-storage). Both apps
expose exactly the same API and Blip Capture URL format.
