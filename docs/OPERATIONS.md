# rakkio operations

For one owner on one private Linux VPS, follow [Personal self-hosting](PERSONAL-SELF-HOSTING.md): its separate production Compose package includes one HTTPS hostname, a private persistent local files volume, owner invite bootstrap and matched backup/isolated restore procedures. The root `compose.yaml` remains development-only.

This guide also covers development Docker Compose and the hosted Vercel/Render deployment. TLS must terminate at the hosting platform or a trusted reverse proxy.

## Production configuration

Production startup intentionally fails unless these invariants hold:

- `APP_ENV=production`
- `ALLOW_INSECURE_DEV_AUTH=false`
- all Google OAuth variables are present
- `TOKEN_ENCRYPTION_KEY` decodes to exactly 32 bytes
- all required S3 variables are present by default; the personal-server package instead explicitly sets `FILE_STORAGE_BACKEND=local` with an absolute persistent `FILE_STORAGE_PATH` and no S3 variables
- `MAX_USER_FILE_STORAGE_BYTES` is positive and does not exceed `MAX_TOTAL_FILE_STORAGE_BYTES`
- `MAX_TOTAL_FILE_STORAGE_BYTES` is set below the provider's free storage allowance
- `MAX_USER_ACCOUNTS` is a positive hard capacity when configured
- `WORKER_TRIGGER_TOKEN` contains at least 32 random characters when hosted cron is enabled
- `DATABASE_URL`, `APP_URL`, and `CORS_ALLOWED_ORIGIN` point at production services

Generate the credential-encryption key with:

```bash
openssl rand -base64 32
```

Set `INVITE_ONLY=true` for a private beta. Invite an account before first login with:

```sql
INSERT INTO account_invites (id, email)
VALUES (gen_random_uuid(), LOWER('person@example.com'));
```

Only set `TRUST_PROXY_HEADERS=true` when untrusted clients cannot bypass the hosting platform or configured reverse proxy. `TRUSTED_PROXY_IP_HEADER` selects exactly one ingress-overwritten IP header: `cf-connecting-ip` on Render, or `x-forwarded-for` behind the supplied Caddy configuration. Render appends to XFF, so its leftmost entry is not trusted. The default automatically selects CF on Render (`RENDER=true`) and sanitized single-IP XFF elsewhere. Forwarded chains, duplicate/invalid selected headers and `X-Real-IP` are not trusted; missing/invalid identity falls back to the socket peer. Do not pass an attacker-controlled selected header through your proxy.

For public registration, keep `INVITE_ONLY=true` until the Privacy Policy and Terms are live, Google OAuth production verification is complete, backup restoration has been tested, alerts are enabled, and production smoke tests pass. Then set `INVITE_ONLY=false` while retaining `MAX_USER_ACCOUNTS`. The account limit is checked under a PostgreSQL advisory transaction lock: new registrations are rejected at capacity, while existing users remain able to sign in. Hosted sign-in requires affirmative acceptance of the current Terms and Privacy Policy and stores the accepted versions and age confirmation in `legal_acceptances`.

## Hosted free-tier deployment

The supported hosted-beta topology is:

- Vercel serves the Vue application at `https://rakkio.com`.
- Render Free runs the Axum API at `https://api.rakkio.com`.
- Local mutations wake an in-process synchronization dispatcher and Google webhooks enqueue inbound changes.
- GitHub Actions is configured to call an authenticated recovery and watch-renewal endpoint every 15 minutes, but execution is best-effort and can be delayed by hours.
- Neon PostgreSQL stores all canonical and operational state.
- A private Cloudflare R2 bucket stores attachments through presigned URLs.

This topology does not require a Google Cloud billing account. Render Free is suitable for a personal beta, not a production SLA. When executed on time, the synchronization trigger can keep the API awake and consume most of the service's monthly free-instance allowance. It cannot guarantee wakefulness when GitHub delays runs. If the trigger is disabled, the API sleeps after 15 idle minutes and can take about one minute to wake. Free services can be suspended when monthly allowances are exhausted. GitHub schedules can be delayed and public-repository schedules are disabled after 60 days without repository activity.

### Guided setup

Run the checked-in wizard from the repository root:

```bash
./scripts/deploy-render.sh
```

The wizard opens each required dashboard, captures secrets with hidden input, stores resumable values in ignored `.env.deploy`, configures the scheduled worker's GitHub secrets, and guides the Render Blueprint and Vercel deployments. Do not commit `.env.deploy`.

### Render API

`render.yaml` defines one Singapore-region Docker web service on the Free plan. Render builds `apps/api/Dockerfile` with the repository root as its build context, checks `/ready`, and deploys changes from the default branch. The Blueprint prompts for PostgreSQL, Google, encryption, and R2 credentials rather than storing them in Git.

Render must receive these canonical public values:

```text
APP_URL                         https://rakkio.com
CORS_ALLOWED_ORIGIN             https://rakkio.com,https://prosepect.com
GOOGLE_REDIRECT_URI             https://api.rakkio.com/api/v1/auth/google/callback
GOOGLE_CALENDAR_WEBHOOK_URL     https://api.rakkio.com/webhooks/google/calendar
BIND_ADDRESS                    0.0.0.0:10000
MAX_USER_ACCOUNTS               100
MAX_USER_FILE_STORAGE_BYTES     104857600
MAX_TOTAL_FILE_STORAGE_BYTES    5368709120
```

Generate `WORKER_TRIGGER_TOKEN` with `openssl rand -hex 32`, store the same value in Render and the GitHub secret `PROSEPECT_WORKER_TRIGGER_TOKEN`, and never put it in source control.

Attach `api.rakkio.com` as a Render custom domain and add the DNS record Render displays. Managed TLS provisioning can take time.

### Scheduled worker

`.github/workflows/worker.yml` calls `POST /internal/synchronization/run` at minutes 7, 22, 37, and 52 of each hour to avoid the busiest start-of-hour scheduling window. The API verifies a bearer token, enqueues stale calendar synchronization and expiring webhook watches, and processes at most one claim. This remains the durable recovery path when an immediate dispatch or Google notification is missed. Overlapping runs are serialized. Scheduled runs use `scripts/worker_request.py` (Python standard library plus `curl`): they do not compile Rust and do not start a worker container. The job remains skipped until `PROSEPECT_WORKER_ENABLED=true` and `PROSEPECT_WORKER_TRIGGER_TOKEN` is configured.

Worker POST requests retry only explicit HTTP429 responses, with 30/60/120-second backoff and a 360-second total request/retry budget (at most four attempts, each at most 180 seconds). `Retry-After` can extend a delay but cannot exceed the remaining budget. Ambiguous POST network failures/timeouts and 5xx responses are not replayed because work may already have started. Readiness GET requests also retry transient network/408/5xx errors within a separate 150-second budget. Redirects are not followed. Each attempt logs status, curl exit code and only allowlisted diagnostic headers (Retry-After, hosting/request IDs, server and content type), never credentials, cookies or response bodies. Local HTTP regression tests run in CI. Persistent throttling, exhausted budgets, malformed success responses, network, authentication, enqueue, or worker infrastructure failures still fail the workflow. A separate bounded `GET /ready` in the same job also verifies the API's PostgreSQL readiness response, even when the trigger fails. A successful trigger does **not** prove Google synchronization succeeded: `SyncService::run_once` records provider failures for retry and can still return `processed: true` with HTTP 200. Inspect the Operations queue/final-failure snapshot and authorized worker logs separately.

The repository is public, so standard GitHub-hosted runners are free. Actual recovery schedules have been observed delayed by hours despite the configured 15-minute cadence. Neither this job nor Render's health check is independent uptime monitoring, and neither verifies alert delivery. Enable Actions failure notifications and manually run the workflow after changing synchronization configuration. Before public beta, configure an independent free HTTP monitor for the frontend and `/ready`, a missed-recovery-run check, and a tested alert destination. See [the public-beta checklist](PUBLIC-BETA-CHECKLIST.md) for owner-only release gates.

### Domains and OAuth

Use these canonical values:

```text
Frontend                  https://rakkio.com
API                       https://api.rakkio.com
Google redirect           https://api.rakkio.com/api/v1/auth/google/callback
Google Calendar webhook   https://api.rakkio.com/webhooks/google/calendar
CORS allowed origins      https://rakkio.com,https://prosepect.com
```

The Google OAuth client must use the redirect URI exactly. The Vercel production environment must set `VITE_API_URL=https://api.rakkio.com`.

A public Google OAuth application must configure:

```text
Application homepage     https://rakkio.com
Privacy Policy           https://rakkio.com/privacy
Terms of Service         https://rakkio.com/terms
Authorized domain        rakkio.com
```

Verify domain ownership using a Google Cloud project owner or editor in Google Search Console. Submit brand and sensitive-scope verification with an English-language video showing the full OAuth consent screen, the exact requested Calendar scopes, and the user-facing synchronization workflow. Keep production in Testing and `INVITE_ONLY=true` until Google approves it.

## Docker Compose (development)

For an Internet-accessible personal server, use the [separate production package](PERSONAL-SELF-HOSTING.md), not these development defaults. Validate and start the local development stack:

```bash
docker compose config --quiet
docker compose up --build -d
docker compose ps
docker compose logs --tail=100 api worker
```

The stack includes PostgreSQL, MinIO, API, worker, and web services. Caddy proxies `/api`, `/docs`, and the OpenAPI document to the API. Configure TLS and the public hostname in an upstream reverse proxy or replace the local Caddy listener.

`S3_ENDPOINT` is the API-to-object-storage address. `S3_PUBLIC_ENDPOINT` is placed into signed browser download URLs. Compose uses `http://minio:9000` internally and publishes MinIO on loopback as `http://localhost:9000`. MinIO and R2 use path-style requests in the supplied configuration, so `S3_VIRTUAL_HOSTED_STYLE=false`.

Do not publish PostgreSQL directly. For S3 deployments a production reverse proxy should expose only the web application, API paths, and the configured object-storage hostname. The separate personal-server package has no object-storage hostname or MinIO service: files are served only through authenticated, tenant-checked API downloads. Existing development MinIO/mc image availability is a separate unresolved dependency issue.

## Request and content budgets

The hosted beta has bounded API capacity (`apps/api/src/request_limits.rs`): 32 active API requests and two large operations (uploads, imports, exports, note-list reads and downloads). Each account has eight active requests, at most four writes and one large operation, plus 240 writes/600 reads per minute. Existing narrower action limits still apply. Normal request bursts wait briefly for capacity; saturation returns 429, never an automatic mutation replay. Public authentication has four separate slots so a slow provider does not occupy normal account capacity. Health/readiness remain independent of API capacity; CSRF-protected logout remains available at an account's write cap. A 50-task batch and its independent Undo operations fit within the write budget.

Ordinary JSON bodies are limited to 1 MiB; Todoist imports have an 8 MiB encoded-body limit as well as their existing task-count limit. Upload requests allow only the configured file maximum plus 16 KiB of multipart overhead. File bytes and UUID metadata are bounded as they stream in, before storage reservation; duplicate/unknown fields are rejected. JSON and multipart body reading each have a 60-second deadline before mutation processing. Oversized bodies return 413. The configured attachment-storage quotas are unchanged.

Client-identity rate-limit maps hard-cap at 10,000 active buckets and 128-byte keys, failing closed instead of evicting live identities. Google authorization attempts are serialized at admission and capped at five per browser and 5,000 globally; expired attempts are cleaned before admission. Consuming a flow frees its slot.

Migration `0032_pending_file_uploads.sql` makes new attachment reservations durable and hidden until PUT confirmation. Pending/uncertain writes remain quota-charged and excluded from listings, downloads and exports. PUT has a 60-second deadline; unconfirmed uploads have a five-minute grace period before bounded cleanup, which releases their records/quota only after storage deletion succeeds. Storage failures back off and retain the reservation. Cleanup runs while the API is awake, through the standalone worker and before upload/erasure requests; sleeping hosts are not guaranteed an exact cleanup time. Account erasure blocks new uploads and returns 409 while active/uncertain writes remain. Retry erasure after cleanup; do not delete reservation rows manually to bypass it. Existing ready files and export/Undo formats are unchanged. This prevents new lifecycle leaks; it does not inventory or remove historical orphan objects.

Migration `0026_workspace_content_limits.sql` adds transactional logical-content counters covering projects, tasks, calendars, events, notes, labels, attachment metadata and saved views, including import/provider writers. Defaults are 16 MiB/20,000 records per account and 256 MiB/100,000 records shared. These are serialized-row budgets, **not physical database-size or R2-byte measurements**; indexes, history, jobs and database bloat still require monitoring and capacity planning. Trusted operators can review the singleton `workspace_content_limits` row before a deliberate capacity change. Do not raise limits automatically to silence errors.

Existing content is backfilled, not removed. Growth over budget returns an actionable `content_quota_exceeded` error; deletion and shrinking remain possible. Private per-transaction deltas are applied to shared counters only at commit, preserving unrelated writers during an open deletion or Undo; failed commits roll back content and counters together. Backfill holds a brief write lock while installing triggers (a ten-second lock-acquisition timeout fails the migration safely instead of silently missing writes). Future migrations changing tracked row shapes must recount the derived counters in the same migration. Database serialization/deadlock aborts return 409 rather than exposing internal errors; preserve drafts and refresh, never blindly replay a write.

Google authorization uses a ten-minute HttpOnly browser-binding cookie (`__Host-prosepect_oauth` with Secure in production). Keep the API hostname stable through authorization; do not strip its cookies at a proxy. In-flight authorizations started before migration `0025` must be restarted. Existing login sessions are not invalidated. Incremental consent also requires the originating account to remain signed in.

Vercel and the supplied Caddy frontends apply CSP, framing, MIME and referrer protections. Vite production preview uses the Vercel policy for browser regression tests. If deploying a separate custom API origin, update CSP `connect-src` alongside `VITE_API_URL` and CORS. The default personal Caddy deployment uses same-origin API paths. Inline scripts and framing are denied; inline styles remain allowed for existing dynamic layouts, and HTTPS Markdown images remain supported. API responses are non-cacheable, use `no-referrer`, and add HSTS in production. Spreadsheet exports prefix potentially executable cells with an apostrophe; canonical task content and JSON exports remain unchanged.

## Optional Discord integration

[Discord setup](DISCORD-INTEGRATION.md) documents the dedicated application, four API settings, Vercel's three public relay settings, direct self-hosting endpoint, explicit data-sharing notice and live acceptance gates. Leave all API Discord settings empty to disable it. OAuth uses the existing browser-binding cookie plus a separate owner-bound attempt table. Migration `0027_discord.sql` adds optional identity links and bounded short-lived receipts; no Google credentials or existing task data are renamed.

The webhook has four dedicated admission slots, a 64 KiB signed raw-body cap and a ten-second read deadline, then uses ordinary per-account limits. The relay acknowledges privately before waiting for Render, with a 120-second execution budget; it is not a durable queue or an uptime guarantee. Never log interaction-token webhook URLs or payloads. Command receipts are capped at 100 per connection, expire after 15 minutes, and are removed by bounded maintenance; pending OAuth attempts are capped at five and expire after ten minutes. Keep unrelated hosting integrations and account/storage caps unchanged.

## Health, metrics, and logs

- `GET /health` checks the API process.
- `GET /ready` checks PostgreSQL.
- `GET /metrics` returns Prometheus text metrics only to a session-authenticated Operations owner configured in `ADMIN_USER_IDS` (401 anonymously, 403 for ordinary accounts). It is not a public scrape endpoint. Keep any metrics collector private and do not put session credentials in URLs or logs.
- API logs carry request IDs and structured production logs.

Important metrics include:

- `prosepect_http_requests_total`
- `prosepect_http_request_duration_seconds`
- `prosepect_api_errors_total`
- `prosepect_synchronization_duration_seconds`
- `prosepect_sync_jobs_total`
- `prosepect_sync_job_failures_total`
- `prosepect_notification_deliveries_total`

Alert on sustained readiness failure, API 5xx responses, failed synchronization jobs, and a growing `sync_jobs` pending/failed backlog. PostgreSQL contains job errors, synchronization conflicts, and tenant-scoped activity history.

## Owner Operations dashboard

`/operations` is a read-only service-wide snapshot, not a monitoring platform. It does not change invite-only access, retry jobs, or perform destructive actions.

### Grant access

1. Identify the intended owner's **existing immutable account UUID**. The signed-in account's `user.id` is available in its own authenticated `GET /api/v1/session` response; verify that it belongs to the intended account using your trusted account-management process. Never use an email, the first registered user, a Google subject, or a client-supplied admin flag as the allowlist identity.
2. Set `ADMIN_USER_IDS` in the API runtime configuration to that UUID. Comma-separated UUIDs are supported if more than one owner must be explicitly authorized. Whitespace is trimmed; duplicates are harmless.
3. Restart/redeploy the API through your normal reviewed release process. Removing a UUID and restarting revokes access. No migration or production configuration is performed by this implementation.

Unset or whitespace-only `ADMIN_USER_IDS` denies everyone. Malformed members, including a trailing comma, fail configuration/startup instead of being ignored. Do not put this variable in frontend `VITE_*` configuration. Keep `ALLOW_INSECURE_DEV_AUTH=false` in hosted environments: existing development authentication permits impersonation and is only for trusted local development. Production configuration already rejects that mode.

### Security boundary

- `GET /api/v1/operations` uses the existing `CurrentUser` session authentication, then checks the server-side UUID allowlist through `OperationsAdmin` **before** database probing or any cross-tenant aggregate query. Missing authentication returns 401; a valid non-owner session returns 403. Development identity headers are ignored when development authentication is disabled.
- `GET /api/v1/operations/capability` is authenticated and returns only a JSON boolean for navigation. A hidden link is not authorization; direct API and page access are still checked.
- Successful responses use `Cache-Control: no-store`. The ordinary session and Google status contracts are not extended with administrative aggregates. No account lists, emails, task titles, raw job errors, provider responses, credentials, or storage keys are returned.
- Session authentication itself requires PostgreSQL. If it fails, the API fails closed with the existing sanitized error; it cannot safely return an authenticated degraded snapshot. If the database fails **after** authorization, a bounded probe produces an unavailable database status and null metrics. If the probe succeeds but the aggregate query fails/times out, database remains responding while metrics are null/unknown. Neither case becomes a zero count.

### What the snapshot means

- **API response** means this authorized snapshot request was answered, not an uptime percentage or historical error rate. **Database probe** is `SELECT 1`. Probe and aggregate query each have a five-second timeout; there is no external provider probe.
- **As of** is the response observation time. Account, file, and job counters are read in one PostgreSQL statement/snapshot after the probe.
- **Accounts** counts all currently registered user rows. A null account cap accurately means unlimited/not configured; it is not zero. Existing accounts may sign in when a configured cap is reached.
- **File usage** sums database attachment metadata across all tenants, using the same definition as quota enforcement. It is not an R2 bucket inventory, orphan-object count, proof of readable objects, or provider billing measurement. Global, per-account, and per-file limits are the API's actual configured limits.
- **Current queue**: pending, running, and failed jobs with fewer than 8 attempts (retryable, including future backoff). Running can include a stale lease pending recovery. Oldest waiting uses original creation time among pending/retryable jobs, not time overdue; null means no waiting jobs.
- **All time** means all retained job rows, not an immutable historical ledger. Final failures have at least 8 attempts; they remain counted even after later successes. Account/calendar deletion can remove job history. Completed counts use `status = 'succeeded'`; the latest completed timestamp is `updated_at`, written by `complete_sync_job`. Null means no retained completed job. Jobs include discovery, watch, synchronization, and credential revocation, so a completion never proves all calendars/providers healthy. A GitHub worker HTTP trigger returning 200 is not provider success.
- **Unknown/not verified**: live R2 health, backups and restore evidence, external alert delivery, budget, provider billing. The dashboard does not collect those signals or claim they work.

### Use and limitations

The page polls every 60 seconds after a request finishes, only while mounted and visible. Requests do not overlap; hiding/unmounting cancels in-flight requests, and late results are ignored. Requests time out after 15 seconds. Failures retain the previous snapshot with a stale warning and a retry button; authorization denial clears it. A degraded response replaces old numbers with unknowns. Last successful refresh records receipt of a snapshot (which can itself be degraded), not overall service health.

An independent **External uptime monitoring** panel displays the owner-configured public UptimeRobot badge for the hosted prosepect website/API and links to its uptime/outage history: <https://stats.uptimerobot.com/xz23PeH80u>. It uses the badge's SVG image, not third-party scripts or a secret API key. Its public identifiers live in `apps/web/src/components/OperationsUptime.vue`; self-hosted installations must replace them with their own monitor before treating the panel as their installation's uptime. The badge refreshes every 60 seconds while visible, follows the provider's actual check/cache intervals and switches with the app theme. A failed/15-second-timed-out image reports unknown, never Up inferred from a healthy snapshot. Receipt time is not monitor-check time. The panel remains independent of snapshot failures for an already-authorized owner; if authentication/API failure prevents Operations opening, use the external status-page link directly. External monitoring continues while this page is closed; configuring the panel does not verify email alert delivery or guarantee uptime. No monitor creation, pause, billing or provider-write controls are added.

For final failures or persistent waiting work, inspect worker logs through your normal authorized operational workflow and verify Google integration configuration. Do not share raw provider errors or tokens. For capacity issues, review the relevant provider console before changing limits. This page has no job controls and sends no alerts when closed.

Known static links (authentication and provider permissions still apply):

- API hosting and logs: <https://dashboard.render.com/>
- Hosted database and recovery settings: <https://console.neon.tech/>
- Cloudflare/R2 storage and billing: <https://dash.cloudflare.com/>
- Google integration configuration and quotas: <https://console.cloud.google.com/>
- Worker workflow runs: <https://github.com/danarrigo/rakkio/actions>
- Independent public uptime and outage history: <https://stats.uptimerobot.com/xz23PeH80u>

No historical metrics pipeline, billing integration, backup verification or custom control plane is included. Aggregate scans may need revisiting if retained job volume grows substantially; this first slice intentionally adds no schema/index migration.

### Validation / generated contract

Backend tests cover allowlist parsing, session authorization, spoofed development-header rejection, empty deny-all, privacy, cross-tenant counts, empty data, and database-down fail-closed/degraded behavior. Run them in the normal remote CI PostgreSQL environment. Do not infer backend validation from frontend mocks.

The Operations OpenAPI paths/schemas are declared in Rust source and reconciled in generated `openapi/openapi.json` and `apps/web/src/api/schema.d.ts`. The frontend uses the shared typed client; no temporary Operations adapter remains. This reconciliation shipped in commit `8efa3fb`, merged in `c54e78b`, with main CI run `34222767710` passing. Continue to check generation consistency remotely for each release candidate; prior main validation does not validate later changes.

## Worker behavior

The worker:

1. enqueues selected Google calendars that have not synchronized recently and writable-calendar watches expiring within 24 hours;
2. claims one job with `FOR UPDATE SKIP LOCKED`;
3. leases it for two minutes;
4. refreshes credentials when required;
5. applies the configured conflict policy;
6. mirrors scheduled tasks through linked Google events while keeping tasks and events distinct;
7. records user-visible activity in PostgreSQL;
8. retries transient failures with bounded exponential backoff.

The API uses the same durable queue immediately after local calendar or scheduled-task mutations. Completing a task retains its historical time block; unscheduling or deleting it removes the Google event. Authenticated Google webhook notifications enqueue the affected calendar without trusting provider payload data.

Run one claim for diagnostics:

```bash
cargo run -p prosepect-api --bin worker -- --once
```

Run the long-lived worker without `--once` for Docker Compose or a future server. Jobs are idempotent per tenant. While the API is awake, its dispatcher checks for eligible queued jobs after a five-second wait or a wake signal, processing at most 32 jobs per drain. This resumes due failed retries without a manual Sync now action, including after startup; long jobs and backlog can delay them. It does not keep Render Free awake or guarantee cron timing.

Interrupted jobs left in `running` status are not automatically reclaimed, even after their lease expires. Safe recovery requires live-worker exclusion and claim fencing; lease expiry alone does not prove that processing stopped. Do not bulk-reset these rows or replay exhausted jobs without inspecting their state.

## Backups and migration to a server

Back up both persistent storage classes together:

1. PostgreSQL, which owns product data, jobs, conflicts, and activity.
2. S3-compatible objects referenced by PostgreSQL file metadata.

Example local backup commands:

```bash
docker compose exec -T postgres pg_dump -U prosepect -Fc prosepect > prosepect-postgres.dump
docker compose run --rm --entrypoint /bin/sh -v "$PWD/backups:/backup" minio-init -c \
  'mc alias set local http://minio:9000 "$S3_ACCESS_KEY_ID" "$S3_SECRET_ACCESS_KEY" && mc mirror local/prosepect /backup/objects'
```

For the hosted beta, use `pg_dump` against Neon and copy R2 objects to an independent encrypted disk or storage account. The database provider's short point-in-time recovery window and R2 itself are not independent backups. Example direct commands are:

```bash
pg_dump "$DATABASE_URL" --format=custom --file=prosepect-postgres.dump
aws s3 sync "s3://$S3_BUCKET" prosepect-r2-backup \
  --endpoint-url "$S3_ENDPOINT" \
  --region auto
```

These examples are not an encrypted backup procedure by themselves. Run them only inside an approved encrypted destination, with authorized credentials supplied securely, and arrange a consistent database/object capture (for example, an owner-approved write pause). Do not put credentials in command history or commit dumps. Retain independently protected recovery keys, including `TOKEN_ENCRYPTION_KEY` if restoring stored integration credentials, with access separate from the encrypted backup.

Readiness review: the saved `DATABASE_URL` is missing, so no database backup can be made from the available configuration. No independent encrypted destination or isolated restore target has been approved. No secrets were read and no backup was attempted. The [public-beta checklist](PUBLIC-BETA-CHECKLIST.md) records the prerequisites and restore evidence required.

For a migration, disable the GitHub synchronization workflow, restore PostgreSQL and objects to the approved destination, configure the same environment contract, and switch DNS only after reviewed validation. For a restore rehearsal, keep outbound Google access, webhook registration, notifications, and all workers/dispatchers disabled or blocked. A restored production database contains live integration state: never start it with unrestricted production-provider access. Validate attachment contents and tenant isolation without contacting real users or calendars before deciding whether a real migration may enable synchronization.

## Google integration recovery

- A `410 Gone` sync-token response triggers one bounded full pull and stores the replacement token only after successful processing.
- `429` and provider `5xx` responses retry and honor `Retry-After`.
- Unresolved `ask` conflicts remain visible in Settings and block silent overwrite for that mapping.
- Webhook channels use random verification tokens stored only as hashes and are renewed before expiration.
- Disconnecting Google queues provider revocation, which stops known webhook channels before deleting encrypted credentials and Google-backed canonical calendars.

If credentials become invalid, reconnect Google Calendar from Settings. Never inspect or copy encrypted token columns into logs.

## Upgrades

Before deployment:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
npm run format:check:web
npm run typecheck:web
npm run lint:web
npm run test:web
npm run build:web
docker compose config --quiet
```

Take a backup, deploy the reviewed release, and monitor API and worker logs. The API and worker apply forward-only SQLx migrations during startup. Never edit an already deployed migration or `CHANGELOG.md`.
