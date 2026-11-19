# syntax=docker/dockerfile:1
# Porter hub images. Build context: the repo root (porter's Rust workspace + hub/); docker-compose.yml sets it.
# Targets: `web` (Next standalone, runs migrations on start) and `worker` (remote runs + retention, ships `porter`).
# What goes into the context: Dockerfile.dockerignore.

# ---- porter CLI (porter-cli only; porter-desktop/GPUI is never built) ----
# Bookworm on both sides: the binary links glibc, so build and runtime share the Debian release.
FROM rust:1.97-bookworm AS porter
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p porter-cli && cp target/release/porter /usr/local/bin/porter

# ---- node: pnpm workspace with every dependency (the worker runs its TypeScript through tsx) ----
# Node >= 22.15: node:sqlite and zlib zstd.
FROM node:24-bookworm-slim AS node
ENV PNPM_HOME=/pnpm PATH=/pnpm:$PATH COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
WORKDIR /app

FROM node AS deps
COPY hub/package.json hub/pnpm-lock.yaml hub/pnpm-workspace.yaml ./
COPY hub/apps/web/package.json apps/web/
COPY hub/apps/worker/package.json apps/worker/
COPY hub/packages/db/package.json packages/db/
COPY hub/packages/porter/package.json packages/porter/
COPY hub/packages/storage/package.json packages/storage/
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile
COPY hub/ ./

FROM deps AS build
ENV NEXT_TELEMETRY_DISABLED=1
RUN pnpm -C apps/web build

# ---- web: Next standalone server; migrations run in instrumentation.ts before it serves ----
FROM node:24-bookworm-slim AS web
ENV NODE_ENV=production NEXT_TELEMETRY_DISABLED=1 HOSTNAME=0.0.0.0 PORT=3000 \
    MIGRATIONS_DIR=/app/migrations HUB_CACHE_DIR=/var/cache/porter-hub
WORKDIR /app
COPY --from=build /app/apps/web/.next/standalone ./
COPY --from=build /app/apps/web/.next/static ./apps/web/.next/static
COPY hub/packages/db/migrations ./migrations
# App files stay root-owned (read-only to the app); only its cache dirs are writable.
RUN mkdir -p apps/web/.next/cache /var/cache/porter-hub \
    && chown node:node apps/web/.next/cache /var/cache/porter-hub
USER node
EXPOSE 3000
CMD ["node", "apps/web/server.js"]

# ---- worker: claims runs and spawns porter; daily retention ----
FROM node:24-bookworm-slim AS worker
# procps: the worker reads process groups and names with `ps`.
RUN apt-get update && apt-get install -y --no-install-recommends procps && rm -rf /var/lib/apt/lists/*
# `porter`, short on purpose: Linux truncates a process name (comm) to 15 chars and the worker matches on it.
COPY --from=porter /usr/local/bin/porter /usr/local/bin/porter
ENV NODE_ENV=production PORTER_BIN=porter \
    WORKER_SCRATCH_DIR=/var/lib/porter-worker HUB_CACHE_DIR=/var/cache/porter-hub
WORKDIR /app
COPY --from=deps /app ./
RUN mkdir -p /var/lib/porter-worker /var/cache/porter-hub \
    && chown node:node /var/lib/porter-worker /var/cache/porter-hub
USER node
# In-process tsx (no wrapper process), so SIGTERM reaches the worker's own handler.
CMD ["node", "--import", "tsx", "apps/worker/src/index.ts"]
