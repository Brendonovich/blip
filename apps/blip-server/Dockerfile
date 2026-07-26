FROM node:24-bookworm-slim AS build

WORKDIR /app
RUN corepack enable && corepack prepare pnpm@11.1.1 --activate

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches ./patches
COPY apps/blip-server/package.json ./apps/blip-server/package.json
COPY packages/blip-server-app/package.json ./packages/blip-server-app/package.json
COPY packages/blip-server-domain/package.json ./packages/blip-server-domain/package.json
RUN pnpm install --frozen-lockfile --filter blip-server...

COPY apps/blip-server ./apps/blip-server
COPY packages/blip-server-app ./packages/blip-server-app
COPY packages/blip-server-domain ./packages/blip-server-domain
RUN pnpm --filter blip-server build

FROM node:24-bookworm-slim

ENV NODE_ENV=production
WORKDIR /app
COPY --from=build /app/apps/blip-server/.output ./.output
COPY --from=build /app/packages/blip-server-app/migrations ./migrations
VOLUME ["/var/lib/blip"]
EXPOSE 3000
CMD ["node", ".output/server/index.mjs"]
