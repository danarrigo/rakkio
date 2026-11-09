<p align="center">
  <img src="apps/web/public/favicon.svg" alt="" width="64" height="64">
</p>

<h1 align="center">rakkio</h1>

<p align="center">
  <strong>A calmer place to plan your day.</strong><br>
  Tasks, projects, notes, calendars, and files in one open-source workspace.
</p>

<p align="center">
  <a href="https://rakkio.com">Open the app</a> ·
  <a href="#getting-started">Get started</a> ·
  <a href="docs/PERSONAL-SELF-HOSTING.md">Self-host</a> ·
  <a href="#documentation">Documentation</a>
</p>

<p align="center">
  <a href="https://github.com/danarrigo/rakkio/actions/workflows/ci.yml"><img src="https://github.com/danarrigo/rakkio/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/danarrigo/rakkio/actions/workflows/security.yml"><img src="https://github.com/danarrigo/rakkio/actions/workflows/security.yml/badge.svg?branch=main" alt="Dependency security status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-475569" alt="License: AGPL-3.0"></a>
</p>

---

rakkio brings daily planning and longer-term work together. Capture an idea in your inbox, turn it into a task or project, and decide what deserves your attention today. Keep supporting notes and attachments alongside the work instead of scattered across apps.

Use the [hosted beta](https://rakkio.com), or run your own installation. Hosted registration is capacity-limited; self-hosted installations use their own Google OAuth credentials and storage.

## Features

|                           | What you can do                                                                                                                                               |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Plan your day**         | See events, due tasks, and scheduled work together. Pick up to three focus tasks, review your day, and carry work forward deliberately.                       |
| **Capture and organize**  | Use an inbox, quick capture, saved task views, subtasks, labels, reminders, dependencies, recurring tasks, bulk actions, and deletion Undo.                   |
| **Move projects forward** | Track outcomes, progress, target dates, and lifecycle states with related tasks, notes, and files.                                                            |
| **Make time for work**    | Switch between day, week, month, and agenda views. Manage native events and selected Google calendars. Task deadlines and reserved work blocks stay distinct. |
| **Keep context close**    | Write safe Markdown notes, attach private files, and search across tasks, projects, notes, and events.                                                        |
| **Connect selectively**   | Opt into Google Calendar and Google Tasks synchronization. Link Discord for private `/add`, `/today`, and version-bound `/done` commands.                     |
| **Stay in control**       | Export JSON, CSV, Markdown, or ICS; manage integrations and account deletion; choose local or S3-compatible file storage.                                     |

The responsive interface supports keyboard navigation, a command palette, and light, dark, or system themes. Google sign-in uses secure cookie sessions, CSRF protection, rate limits, and encrypted stored Google credentials.

**Optional Groups:** handles, invitations, membership, and shared assignments with independent member progress are available behind `GROUPS_ENABLED` (disabled by default). Group events and shared-agenda integration are still in development. See [Groups](docs/GROUPS.md) for availability and privacy boundaries.

## Getting started

| Your goal                         | Start here                                                                                                                                                  |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Use rakkio**                    | Open [rakkio.com](https://rakkio.com).                                                                                                                      |
| **Run a private personal server** | Follow [Personal self-hosting](docs/PERSONAL-SELF-HOSTING.md): one owner, one Linux server, one HTTPS hostname, Google login, and persistent private files. |
| **Develop locally**               | Follow the local setup below, or expand the development Compose instructions.                                                                               |
| **Operate a hosted deployment**   | Read [Operations and deployment](docs/OPERATIONS.md) for configuration, workers, storage, and recovery.                                                     |

### Local development

**Prerequisites:** Rust 1.94+, Node.js 22.22.2+, and PostgreSQL 16+.

Install the frontend dependencies:

```bash
npm ci
```

Create a local PostgreSQL database, then configure the API's process environment using [.env.example](.env.example) as a reference. The API reads environment variables; Cargo does not automatically load a `.env` file.

**Terminal 1: API**

```bash
# Example local credentials only. Adjust for your PostgreSQL installation.
export DATABASE_URL='postgres://postgres:postgres@localhost:5432/prosepect'
cargo run -p prosepect-api --bin prosepect-api
```

**Terminal 2: frontend**

```bash
npm run dev:web
```

Open the app at **<http://localhost:5173>** and Swagger UI at **<http://localhost:3000/docs/>**.

When Google synchronization is configured, start the worker in another terminal with the same API environment:

```bash
cargo run -p prosepect-api --bin worker
```

Add `--once` to claim at most one synchronization job for diagnostics.

> [!WARNING]
> Development authentication is intentionally insecure. Keep development services private; it cannot be enabled with `APP_ENV=production`. Use the separate [personal-server package](docs/PERSONAL-SELF-HOSTING.md) for a public HTTPS installation.

<details>
<summary><strong>Alternative: Docker Compose for development</strong></summary>

With Docker and Compose installed, run:

```bash
docker compose up --build
```

- Application: <http://localhost:8080>
- OpenAPI documentation: <http://localhost:8080/docs/>

Compose starts PostgreSQL, a private object bucket, the API, frontend, and worker. The web service and development MinIO endpoint bind to `127.0.0.1`. Signed attachment URLs use `S3_PUBLIC_ENDPOINT`, defaulting to `http://localhost:9000`.

**Known caveat:** the development MinIO/mc image tags have returned registry authorization errors, and their upstream repositories are archived. The personal-server package does not use MinIO; its lifecycle tests do not establish that the development MinIO images are available.

</details>

### Connect Google and Discord

Basic Google login uses OpenID scopes. Calendar and Tasks access are separate, optional connections rather than automatic permissions at signup.

<details>
<summary><strong>Google OAuth configuration</strong></summary>

1. Create a Google OAuth web client and configure the consent screen for OpenID scopes.
2. Add `GOOGLE_REDIRECT_URI` as an authorized redirect URI, matching your installation exactly.
3. Generate `TOKEN_ENCRYPTION_KEY` with `openssl rand -base64 32` and configure all four Google variables listed together in [.env.example](.env.example).
4. Configure Calendar access when needed. In production, set `GOOGLE_CALENDAR_WEBHOOK_URL` to a public HTTPS endpoint ending in `/webhooks/google/calendar`; omit it locally without a public callback.
5. Follow [Google Tasks synchronization](docs/GOOGLE-TASKS-SYNC.md) for its API setup and separate consent.
6. Restart the API and worker with the updated environment.

Production requires Google configuration, secure cookies, and development authentication disabled. Each self-hosted installation supplies its own OAuth client. Publishing an OAuth application is not the same as Google verification; see [Google sign-in verification](docs/GOOGLE-SIGN-IN-VERIFICATION.md).

</details>

For Discord, follow [Discord integration](docs/DISCORD-INTEGRATION.md). It is disabled by default and requires explicit account linking. No bot token, Gateway connection, chat-history permission, or separate database is required. The supplied fast relay handles acknowledgement for a sleeping API; always-on installations can use the API endpoint directly. Actual provider setup and acceptance must be verified separately from automated tests.

## Keyboard shortcuts

| Shortcut                                    | Action                                                       |
| ------------------------------------------- | ------------------------------------------------------------ |
| `N`                                         | Open task capture                                            |
| `E`                                         | Create an event                                              |
| `/`                                         | Search the workspace                                         |
| `Ctrl+K` / `Cmd+K`                          | Open the command palette                                     |
| `G`, then `I` / `T` / `P` / `C` / `N` / `S` | Go to Inbox / Today / Projects / Calendar / Notes / Settings |
| `Ctrl+Enter` / `Cmd+Enter`                  | Create the task                                              |
| `?`                                         | Show shortcut help                                           |
| `Escape`                                    | Close dialogs                                                |

Single-key shortcuts apply outside editable controls. Task ordering also has pointer and keyboard-accessible controls.

## Under the hood

| Layer               | Technology                                                                                                                              |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| **API**             | Rust, Axum, Tokio, SQLx, REST, and OpenAPI                                                                                              |
| **Frontend**        | Vue 3, TypeScript, Vite, Tailwind CSS, Pinia, and Reka UI                                                                               |
| **Canonical state** | PostgreSQL for product data, jobs, conflicts, and activity                                                                              |
| **Files**           | Local filesystem or S3-compatible storage, including R2 and MinIO                                                                       |
| **Background work** | Portable long-running or one-shot worker, plus an authenticated API trigger for hosted scheduling                                       |
| **Hosted app**      | Vercel frontend and optional Discord relay, Render Free API, GitHub Actions synchronization trigger, Neon PostgreSQL, and Cloudflare R2 |
| **Personal server** | Docker Compose, PostgreSQL, persistent private files, API, worker, and HTTPS web entrypoint                                             |

```text
apps/api/          API, synchronization service, and worker entrypoints
apps/web/          Vue application, prerendered public pages, and browser tests
deploy/personal/   Private production self-hosting package
migrations/        PostgreSQL migrations
openapi/           Generated OpenAPI contract
scripts/           Deployment, worker, and operational helpers
docs/              Feature guides and operator documentation
PRD.md             Authoritative product requirements
CONTEXT.md         Shared product terminology
```

**Compatibility note:** the public name is rakkio. Legacy Rust crate, cookie, database, provider-provenance, and deployment-resource identifiers are deliberately retained. Existing data and Google mappings are not renamed or recreated.

## Documentation

| Guide                                                  | Covers                                                                       |
| ------------------------------------------------------ | ---------------------------------------------------------------------------- |
| [Personal self-hosting](docs/PERSONAL-SELF-HOSTING.md) | Private HTTPS setup, backups, isolated restore, and upgrades                 |
| [Operations](docs/OPERATIONS.md)                       | Deployment, configuration, quotas, workers, observability, and recovery      |
| [Google Tasks](docs/GOOGLE-TASKS-SYNC.md)              | Separate consent, list selection, synchronization, and conflicts             |
| [Discord](docs/DISCORD-INTEGRATION.md)                 | Account linking, private commands, hosted relay, and self-hosted credentials |
| [Groups](docs/GROUPS.md)                               | Feature gating, membership, assignments, and privacy                         |
| [Saved task views](docs/SAVED-TASK-VIEWS.md)           | Reusable task filters                                                        |
| [Bulk actions](docs/BULK-TASK-ACTIONS.md)              | Multi-task changes and safeguards                                            |
| [Recurring edits](docs/RECURRING-TASK-EDITS.md)        | Occurrence and forward-edit behavior                                         |
| [Search indexing](docs/SEARCH-INDEXING.md)             | Public canonical pages, crawl files, and private-page exclusions             |
| [Security policy](SECURITY.md)                         | Reporting vulnerabilities and security checks                                |
| [Product contract](PRD.md)                             | Requirements and intended behavior                                           |

<details>
<summary><strong>Contributor checks and API generation</strong></summary>

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
```

PostgreSQL integration tests create isolated databases and require a superuser-compatible test database URL. Use a disposable test instance, never a production database. Browser tests that exercise the backend also require the API to be running.

After changing API schemas or routes, generate the contract from the actual Rust exporter and regenerate frontend types:

```bash
npm run generate:api
```

Commit both `openapi/openapi.json` and `apps/web/src/api/schema.d.ts`. Do not hand-edit generated contracts.

</details>

## License

[GNU Affero General Public License v3.0](LICENSE). Network users must be offered the corresponding source for modified deployed versions. The license does not prohibit third-party paid hosting.
