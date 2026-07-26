# Blip Server

A single-process Node deployment of the Blip SolidStart application. It uses
local SQLite and the shared
[`@blip/server-app`](../../packages/blip-server-app).

## Configuration

Copy `.env.example` to `.env` and generate an encryption secret:

```sh
openssl rand -base64 32    # BLIP_ENCRYPTION_KEY
```

`BLIP_ENCRYPTION_KEY` encrypts S3
credentials with AES-256-GCM and must remain stable for the lifetime of the
database. Set `BLIP_PUBLIC_ORIGIN` to the canonical HTTPS origin users will open,
such as `https://blip.example.com`.

Set `BETTER_AUTH_SECRET` to another stable random value and configure
`GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` for the public origin's OAuth
callback URL.

## Run

Use Node 24 or newer. From the repository root:

```sh
pnpm install
pnpm --filter blip-server build
DATABASE_PATH=/var/lib/blip/blip.sqlite pnpm --filter blip-server start
```

Nitro emits a self-contained Node deployment in `.output`; the start command
runs `.output/server/index.mjs` and serves the generated client assets itself.

`HOST` defaults to `0.0.0.0`, `PORT` to `3000`, and the SQLite parent directory
is created automatically. Put the process behind an HTTPS reverse proxy. Do not
run multiple Node replicas against the same SQLite file.

## Docker

Build from the repository root and mount a persistent data directory:

```sh
docker build -f apps/blip-server/Dockerfile -t blip-server .
docker run --rm -p 3000:3000 \
  --env-file apps/blip-server/.env \
  -e DATABASE_PATH=/var/lib/blip/blip.sqlite \
  -v blip-data:/var/lib/blip \
  blip-server
```

## Configure Storage

After logging in with GitHub, connect that user's S3-compatible bucket:

```sh
curl https://blip.example.com/api/storage \
  -X PUT \
  -H "Authorization: Bearer $BLIP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint":"https://ACCOUNT.r2.cloudflarestorage.com",
    "region":"auto",
    "bucket":"recordings",
    "accessKeyId":"...",
    "secretAccessKey":"...",
    "forcePathStyle":true
  }'
```

The credentials need bucket read and write access plus multipart create, upload,
complete, and abort permissions. The endpoint must be reachable by both the
server and the Mac running Blip Capture.

Add this single capture URL to the user's Blip recording profile:

```text
https://blip.example.com#BLIP_API_KEY
```

The fragment is never sent as part of a URL request. Capture uses it as the
Bearer credential.

## API

The contract is defined with Effect's typed `HttpApi`. Scalar is served from
`/api`, and the OpenAPI document is available at `/openapi.json`.

- `GET|PUT|DELETE /api/storage` manages the authenticated user's bucket.
- `POST /api/uploads` creates a video and starts its multipart upload.
- `POST /api/uploads/:id/parts` signs one upload part.
- `POST /api/uploads/:id/assets` signs an HLS initialization or media segment.
- `POST /api/uploads/:id/complete` completes the upload.
- `DELETE /api/uploads/:id` aborts the upload.
- `GET /v/:id` validates a private capability and renders a full-page
  video using a fresh one-hour presigned GET URL.
- `GET /v/:id/playlist.m3u8` serves an HLS playlist with fresh presigned
  asset URLs.

Storage configuration is snapshotted into each video record so changing a
user's bucket cannot move an in-progress or existing recording.

For the Alchemy/D1 deployment, see
[`apps/blip-server-cloudflare`](../blip-server-cloudflare).
