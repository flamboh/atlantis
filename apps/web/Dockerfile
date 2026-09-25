# syntax=docker/dockerfile:1

ARG BUN_VERSION=1.3.11
ARG NODE_VERSION=24.18.1

FROM oven/bun:${BUN_VERSION}-slim AS bun

FROM node:${NODE_VERSION}-bookworm-slim AS workspace

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install --yes --no-install-recommends g++ make python3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun

WORKDIR /app
COPY package.json bun.lock ./
COPY patches patches
COPY apps/web/package.json apps/web/package.json
COPY apps/landing/package.json apps/landing/package.json
COPY infra/package.json infra/package.json

FROM workspace AS build

RUN bun install --frozen-lockfile --ignore-scripts --filter @atlantis/web

COPY apps/web apps/web

RUN cd apps/web && ATLANTIS_DB_DRIVER=sqlite bun run build

FROM workspace AS production-deps

RUN bun install --frozen-lockfile --ignore-scripts --production --filter @atlantis/web \
    && npm run build-release --prefix node_modules/better-sqlite3 \
    && rm -rf node_modules/better-sqlite3/build/Release/obj node_modules/better-sqlite3/deps \
    && node -e "new (require('better-sqlite3'))(':memory:').prepare('select 1').get()"

FROM node:${NODE_VERSION}-bookworm-slim AS runtime

ARG RUNTIME_UID=1000
ARG RUNTIME_GID=1000

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=3000 \
    LOCAL_DATA_DIR=/data

WORKDIR /app
COPY --from=production-deps /app/node_modules node_modules
COPY --from=build /app/apps/web/package.json package.json
COPY --from=build /app/apps/web/build build

RUN mkdir -p /data

USER ${RUNTIME_UID}:${RUNTIME_GID}
EXPOSE 3000
CMD ["node", "build"]
