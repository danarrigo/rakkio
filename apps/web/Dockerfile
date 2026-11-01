FROM node:22-bookworm-slim AS builder

WORKDIR /workspace
COPY package.json package-lock.json ./
COPY apps/web/package.json apps/web/package.json
COPY packages/minimatch-compat packages/minimatch-compat
RUN npm ci --workspace @rakkio/web --include-workspace-root \
    --fetch-timeout=600000 --fetch-retries=5

COPY apps/web apps/web
RUN npm run build:web

FROM caddy:2.10-alpine AS runtime
COPY apps/web/Caddyfile /etc/caddy/Caddyfile
COPY --from=builder /workspace/apps/web/dist /srv
EXPOSE 80
