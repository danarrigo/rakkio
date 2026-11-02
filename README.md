# rakkio

rakkio is an open-source, self-hostable personal productivity workspace that combines daily planning, tasks, outcome-oriented projects, notes, calendars, files, and priorities in one calm command center.

The application is designed both as a useful personal system and as a production-oriented reference project. PostgreSQL owns product and operational state, Google Calendar synchronization is conflict-aware and retryable, and files use local or S3-compatible object storage.

## Features

- Google OpenID Connect login with encrypted credentials, secure cookie sessions, CSRF protection, capped public-beta hosted access, and authentication rate limits
- Today view with events, due and scheduled tasks, up to three focus tasks, automatic or manual daily review, and carry-forward decisions
- Standalone and project tasks with subtasks, labels, reminders, recurrence, sparse manual ordering, quick capture, and optimistic concurrency
- Outcome-oriented projects with progress, target dates, lifecycle states, archival, notes, tasks, and files
- Native and selected Google calendars with day, week, month, and agenda views
- Near-real-time Google Calendar synchronization, renewable webhook channels, durable recovery jobs, and scheduled-task time blocks
- Event recurrence, attendees, locations, timezones, editing, deletion, drag rescheduling, and conflict policies
- Safe Markdown notes with project, task, or event links and private attachments
- Tenant-scoped global search across tasks, projects, notes, and events
- Authorized file upload and download using local storage, MinIO, Cloudflare R2, or another S3-compatible service
- Portable JSON, CSV, Markdown, and ICS exports plus complete account and integration deletion
- Responsive keyboard-accessible UI with system-aware and manual light or dark themes
- Optional private Discord `/add`, `/today`, and version-bound `/done` commands linked to the same workspace

Hosted app: <https://rakkio.com>. Source: <https://github.com/danarrigo/rakkio>.

See [PRD.md](PRD.md) for the product contract. The public name is rakkio; legacy Rust crate, cookie, database, provider-provenance and deployment-resource identifiers are deliberately retained for compatibility. Existing data and Google mappings are not renamed or recreated.

## Architecture

- **API:** Rust, Axum, Tokio, SQLx, REST, and OpenAPI
- **Frontend:** Vue 3, TypeScript, Vite, Tailwind CSS, Pinia, and Reka UI
- **Data:** PostgreSQL for canonical product state, jobs, conflicts, and activity
- **Files:** local filesystem in development or S3-compatible object storage
- **Background work:** portable long-running or one-shot worker, plus an authenticated API trigger for hosted cron
- **Hosted deployment:** Vercel SPA and optional fast Discord acknowledgement function, Render Free API, GitHub Actions synchronization trigger, Neon PostgreSQL, and Cloudflare R2
- **Personal self-hosting:** Docker Compose with PostgreSQL, private persistent files, API, worker, and one HTTPS web hostname

## Repository layout

```text
apps/api/          Axum API, synchronization service, and worker entrypoints
apps/web/          Vue single-page application and Playwright tests
render.yaml        Render Free API Blueprint
migrations/        PostgreSQL migrations
scripts/           Guided deployment setup
openapi/           Generated OpenAPI contract
PRD.md             Authoritative product requirements
```

## Private personal server (HTTPS)

For **one person on one Linux VPS**, with one HTTPS hostname, Google login and a private persistent files volume, use [Personal self-hosting](docs/PERSONAL-SELF-HOSTING.md) for phone access, backups, isolated restore and upgrades. It uses a separate production Compose package; the development defaults below are not safe for public exposure.

## Run with Docker Compose (development)

Prerequisites: Docker with Compose support.

```bash
docker compose up --build
```

Open:

- Application: <http://localhost:8080>
- OpenAPI documentation: <http://localhost:8080/docs/>

Compose binds the web service and development MinIO endpoint to `127.0.0.1`, bootstraps PostgreSQL and a private object bucket, and starts the portable worker. Signed attachment URLs use `S3_PUBLIC_ENDPOINT`, which defaults to `http://localhost:9000` for Compose. The default development login is intentionally insecure and cannot be enabled with `APP_ENV=production`. The existing development MinIO/mc image tags have returned registry authorization errors and their upstream repositories are archived; that development dependency issue is not changed by the personal package, which does not use MinIO.

To exercise real Google login and Calendar synchronization, copy `.env.example`, configure the four Google credential variables, and use the callback URL shown there. Each self-hosted installation supplies its own Google OAuth client.

## Run locally

Prerequisites:

- Rust 1.94 or newer
- Node.js 22.22.2 or newer
- PostgreSQL 16 or newer

```bash
cp .env.example .env
npm install

cargo run -p prosepect-api --bin prosepect-api
npm run dev:web
```

Run the background worker in another terminal when Google synchronization is configured:

```bash
cargo run -p prosepect-api --bin worker
```

Use `--once` to claim at most one synchronization job for diagnostics.

The frontend runs at <http://localhost:5173>. Swagger UI runs at <http://localhost:3000/docs/>.

## Google setup

1. Create a Google OAuth web client.
2. Add `GOOGLE_REDIRECT_URI` as an authorized redirect URI.
3. Generate `TOKEN_ENCRYPTION_KEY` with `openssl rand -base64 32`.
4. Configure the consent screen for OpenID scopes. Calendar scopes are requested later, only when a signed-in user connects Google Calendar.
5. In production, set `GOOGLE_CALENDAR_WEBHOOK_URL` to a public HTTPS endpoint ending in `/webhooks/google/calendar`. Omit it locally when no public HTTPS callback is available.
6. Restart the API and worker.

Production requires Google configuration, secure cookies, and development authentication disabled. S3-compatible storage remains the default production requirement for hosted deployments; the personal-server package explicitly opts into local storage with `FILE_STORAGE_BACKEND=local` and an absolute persistent `FILE_STORAGE_PATH`. Set `INVITE_ONLY=true` for private access and insert lowercase emails into `account_invites` before first sign-in. For open registration, set a hard `MAX_USER_ACCOUNTS` capacity; existing accounts can still sign in after capacity is reached. `MAX_USER_FILE_STORAGE_BYTES` limits each account independently, while `MAX_TOTAL_FILE_STORAGE_BYTES` remains the deployment-wide ceiling. Hosted deployments should set a random `WORKER_TRIGGER_TOKEN`; the scheduled GitHub workflow renews watches and recovers missed synchronization without compiling Rust on every run.

## Optional Discord setup

See [Discord integration](docs/DISCORD-INTEGRATION.md) for the hosted/shared-app model, separate self-hosted credentials, explicit account linking, private commands, and operator setup. Disabled by default. No bot token, Gateway connection, chat-history permission, or separate database is required. A sleeping API needs the supplied fast relay; always-on self-hosters can use the API endpoint directly. Actual Discord installation and provider acceptance must be checked separately from automated tests.

## Keyboard shortcuts

- `N` opens global task capture outside editable controls.
- `Ctrl+Enter` or `Cmd+Enter` creates the task.
- `Escape` closes dialogs.
- Task ordering includes pointer and keyboard-accessible controls.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo test --workspace --all-targets

npm run format:check:web
npm run typecheck:web
npm run lint:web
npm run test:web
npm run build:web
npm run test:e2e
npm run generate:api
```

PostgreSQL integration tests create isolated databases and require a superuser-compatible test database URL.

## API contract

Regenerate the OpenAPI document and frontend types after changing schemas or routes:

```bash
npm run generate:api
```

Commit both `openapi/openapi.json` and `apps/web/src/api/schema.d.ts`.

## Operations and deployment

Production configuration, backups, worker behavior, observability, recovery, and Vercel/Render deployment are documented in [docs/OPERATIONS.md](docs/OPERATIONS.md). Run `scripts/deploy-render.sh` for the guided hosted setup.

## License

rakkio is licensed under the [GNU Affero General Public License v3.0](LICENSE). Network users must be offered the corresponding source for modified deployed versions. The license does not prohibit third-party paid hosting.
