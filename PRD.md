# rakkio Product Requirements Document

## 1. Product summary

rakkio is an open-source, self-hostable personal productivity web application that combines calendar planning, tasks, projects, notes, files, and daily prioritization in one workspace.

The product serves two goals:

1. Be genuinely useful as the creator's daily productivity system.
2. Demonstrate production-quality product, backend, frontend, integration, data-modeling, testing, and deployment skills.

The official product domain is `rakkio.com`. The public brand is lowercase `rakkio`.

## 2. Product principles

1. **One actionable view:** events, scheduled work, deadlines, and daily priorities belong in one command center.
2. **User ownership:** users can self-host rakkio and export their data in portable formats.
3. **Reliable synchronization:** external calendar synchronization must not silently destroy changes.
4. **Focused defaults:** built-in workflows should be useful without extensive configuration.
5. **Portable architecture:** core product behavior must not depend on the official hosted infrastructure.
6. **No artificial complexity:** infrastructure and storage technologies must have clear product responsibilities.

## 3. Target users

### Primary user

An individual who wants one place to plan their day across tasks, projects, notes, and calendars.

### Initial audience

The creator first, followed by invited individual beta users.

### Excluded from v1

- Teams and collaborative workspaces
- Enterprise organizations
- Native mobile or desktop applications

## 4. Platforms

- Responsive web application
- Online-first
- Desktop and mobile browser layouts
- No native application in v1
- No offline editing in v1
- No installable PWA requirement in v1

## 5. Core user journey

1. The user signs in with Google.
2. The user optionally connects selected Google calendars.
3. The daily command center shows today's events, scheduled tasks, due tasks, and chosen focus tasks.
4. The user captures work through forms or deterministic command input.
5. The user organizes tasks and notes into outcome-oriented projects.
6. The user schedules a task into a calendar time block.
7. rakkio synchronizes linked Google calendar data and surfaces true conflicts.
8. The user reviews unfinished focus tasks during the next daily review.

## 6. Functional requirements

### 6.1 Authentication and tenancy

- Google OAuth is the only v1 authentication method.
- Each self-hosted installation configures its own Google OAuth credentials.
- One installation can support multiple private user accounts.
- Every user-owned record must be tenant-isolated.
- Generic OpenID Connect and local authentication are deferred.

### 6.2 Daily command center

The command center must:

- Show today's calendar events.
- Show tasks due today.
- Show tasks scheduled today.
- Allow tasks without a due date to be selected for today.
- Allow up to three highlighted focus tasks.
- Permit additional non-focus tasks in today's list.
- Start a daily review automatically on first visit or manually, according to user settings.
- Prompt users to carry forward, reschedule, or remove unfinished focus tasks during the next review.

### 6.3 Tasks

Tasks must support:

- Title and optional description
- Optional project membership
- Due date
- Optional scheduled start and end time
- Status: `todo`, `in_progress`, `blocked`, or `completed`
- Priority: `low`, `medium`, `high`, or `urgent`
- User-defined global labels
- Subtasks
- Optional in-app reminders
- Daily, weekly, monthly, or yearly recurrence anchored to the prior scheduled deadline
- Manual ordering with pointer and keyboard controls
- Up to three daily focus selections
- Created and updated timestamps

Scheduled tasks must create a distinct linked calendar event. The task and event remain separate records. If the chosen calendar is connected to Google, the linked event synchronizes automatically.

Deferred task capabilities:

- Task dependency graphs
- Custom statuses

### 6.4 Projects

Projects must represent outcomes rather than simple folders. They support:

- Name
- Description or outcome statement
- Target date
- Status: `planned`, `active`, `paused`, `completed`, or `archived`
- Calculated task progress
- Associated tasks, notes, and files

### 6.5 Notes

Notes must:

- Use Markdown source
- Render Markdown safely
- Stand alone or attach to a project, task, or calendar event
- Support file attachments
- Participate in global search

A rich-text document editor and wiki-style backlinks are outside v1.

### 6.6 Calendars and events

rakkio must support:

- rakkio-native calendars
- Selected connected Google calendars
- Two-way synchronization
- Day, week, month, and agenda views
- Recurring calendar events
- Event creation, editing, deletion, dragging, and resizing
- Timezones
- Event attendees and locations where provided by Google
- Links between scheduled tasks and calendar events

Task recurrence and calendar-event recurrence are modeled separately. Completing a recurring task creates its next occurrence from the prior scheduled deadline so late completion does not shift the series.

### 6.7 Synchronization conflicts

rakkio must detect when an event changed in both rakkio and Google after the last synchronized version.

The default global conflict policy is `ask`. The user may change it to:

- Keep the most recently edited version
- Prefer Google
- Prefer rakkio

Conflict policy is global in v1, not per-calendar. Every synchronization decision and failure must be recorded in activity history.

### 6.8 Quick capture

rakkio must provide deterministic command capture for inputs such as:

```text
Submit report tomorrow #work
```

The parser should recognize, at minimum:

- Task title
- Common relative dates
- Time where provided
- Labels prefixed with `#`

Quick capture must not depend on an AI model.

### 6.9 Search

Global search covers:

- Tasks
- Projects
- Notes
- Calendar events

Extracting and searching uploaded file contents is deferred.

### 6.10 Files

Users may upload files to projects, tasks, and notes.

Requirements:

- Object storage rather than database blobs
- File metadata stored in PostgreSQL
- Configurable file-size limits
- Content-type validation
- Tenant-aware authorization
- Signed upload and download operations where supported

### 6.11 Reminders and notifications

v1 supports in-app reminders while the application is open. Browser notifications and Web Push are deferred. The product does not require PWA installation.

### 6.12 Export and deletion

Users must be able to export:

- Complete structured data as JSON
- Tasks as CSV
- Notes as Markdown
- Calendars as ICS

Account deletion must revoke integrations and remove or schedule deletion of user-owned data and files.

### 6.13 Appearance and accessibility

- Minimal, calm visual design
- Responsive layouts
- System-aware light and dark themes
- Manual theme override
- Keyboard-accessible interactions
- Accessible headless components
- Visible focus states and semantic markup

### 6.14 Demo, collaboration, and AI

The following are not part of v1:

- Recruiter-specific demo mode
- Shared projects or real-time collaboration
- AI-generated plans, summaries, or capture

### 6.15 Optional Discord commands

- One official application for hosted rakkio; self-hosters supply a separate application and credentials.
- Explicit browser/account-bound linking and a separate data-sharing notice; only identity and application-command permissions.
- `/add` creates a literal Inbox task with an optional local-date deadline; `/today` provides a bounded private summary; `/done` completes a selected saved version using normal recurrence rules.
- Replies are private by default. No chat scraping, AI, automatic reminders or arbitrary third-party-instance routing.
- Signed interactions, per-account limits, replay-safe task changes, explicit disconnect and account-deletion cleanup.
- The sleeping hosted API uses a fast acknowledgement relay; always-on self-hosters can use the direct endpoint. Real provider acceptance is separate from automated coverage.
- See [Discord integration](docs/DISCORD-INTEGRATION.md) for setup, constraints and release gates.

## 7. Technical architecture

### 7.1 Repository

rakkio uses one monorepo containing:

- Rust backend and shared crates
- Vue frontend
- Database migrations
- Docker self-hosting configuration
- Deployment configuration
- Product and contributor documentation

### 7.2 Backend

- Language: Rust
- HTTP framework: Axum
- Async runtime: Tokio
- API style: REST
- API contract: OpenAPI
- PostgreSQL access: SQLx with explicit SQL
- Hosted runtime: Render container
- Self-hosted runtime: long-running HTTP server
- Background work: portable one-shot or long-running worker entrypoint

Business logic must not depend directly on Render, GitHub Actions, Vercel, Neon, or a specific S3-compatible provider.

### 7.3 Frontend

- Vue 3
- TypeScript
- Vite
- Tailwind CSS
- Accessible headless Vue components
- Generated TypeScript client from OpenAPI

The frontend is a single-page application because rakkio is an authenticated dashboard and does not require search-engine rendering.

### 7.4 Storage responsibilities

#### PostgreSQL

PostgreSQL is the canonical source for:

- Users
- Settings
- Projects
- Tasks and subtasks
- Labels
- Notes and links
- Calendars and canonical events
- External event mappings
- Reminders
- File metadata
- Daily reviews and focus selections
- Synchronization jobs and conflicts
- Tenant-scoped activity history

Raw provider payloads are processed transiently and are not retained as duplicate personal data.

#### Object storage

- Official hosted service: Cloudflare R2 private bucket
- Self-hosted service: any S3-compatible storage
- Docker Compose default: MinIO or another compatible local implementation

### 7.5 API conventions

- REST resources under `/api/v1`
- JSON request and response bodies
- OpenAPI-generated frontend types and client
- Consistent structured error envelope
- Cursor pagination for unbounded collections
- Idempotency for synchronization and retryable commands
- Optimistic concurrency where records can conflict

### 7.6 Security

- OAuth tokens encrypted at rest
- Secrets supplied through environment or secret managers
- Tenant ownership checked for every operation
- Secure, HTTP-only session cookies
- CSRF protection where required
- Strict CORS configuration
- Rate limits for authentication, capture, upload, and synchronization endpoints
- File validation and signed object access
- No provider tokens in browser storage
- Audit-sensitive activity retained with tenant isolation in PostgreSQL

## 8. Deployment and self-hosting

### 8.1 Official hosted beta

- Domain: `rakkio.com`
- Frontend: Vercel
- Backend: Axum container on Render Free
- Scheduled work: GitHub Actions invokes an authenticated one-shot synchronization endpoint every 15 minutes without compiling Rust per run
- PostgreSQL: Neon free tier during beta
- Files: private Cloudflare R2 bucket with a 5 GiB application-level storage quota
- Access: invite-only
- Price: free during beta
- Target infrastructure cost: approximately $0 within provider free allowances

The beta must not require an upfront payment authorization. Render may sleep the API after idle periods, GitHub schedules may be delayed, and free services may be suspended when provider allowances are exhausted.

### 8.2 Self-hosting

v1 officially supports Docker Compose on one machine. The deployment must include or configure:

- rakkio web frontend
- rakkio API
- Background worker
- PostgreSQL
- S3-compatible object storage

Kubernetes and manual binary installation are not officially supported in v1.

### 8.3 Feature parity

The managed and self-hosted editions expose the same product features. Future managed revenue comes from operating the service rather than withholding product capabilities.

## 9. Licensing and business model

- Intended license: GNU Affero General Public License v3.0
- Final license choice should receive legal review before public release.
- Users may self-host rakkio.
- The official managed service will eventually charge for hosting convenience.
- The first hosted beta has no payments.

AGPL requires source availability for modified versions offered over a network. It does not prohibit third parties from selling hosted rakkio services.

## 10. Observability and quality

The product must include:

- Structured logs with request and correlation IDs
- Health and readiness endpoints
- Metrics for API errors, synchronization latency, job failures, and notification delivery
- Error reporting without leaking tokens or personal content
- PostgreSQL migration checks
- Unit tests for domain behavior and parsing
- Integration tests against PostgreSQL and S3-compatible storage
- API contract tests
- Frontend component and interaction tests
- End-to-end tests for critical user journeys
- Formatting, linting, type checking, and tests in CI

## 11. Delivery phases

### Phase 1: executable foundation

- Monorepo and developer tooling
- Axum API and Vue shell
- PostgreSQL migrations
- Projects and tasks vertical slice
- OpenAPI contract
- Docker-based local development
- CI quality gates

### Phase 2: personal workflow

- Notes
- Labels and subtasks
- Daily command center and review
- Local calendars and event views
- Quick capture
- Global search

### Phase 3: integrations

- Google OAuth
- Google Calendar connection
- Two-way synchronization
- Conflict handling
- PostgreSQL activity history
- In-app reminder engine and delivery validation

### Phase 4: files and portability

- S3-compatible uploads
- JSON, CSV, Markdown, and ICS exports
- Account deletion

### Phase 5: distribution

- Complete Docker Compose self-hosting
- Vercel frontend deployment
- Render Free API and GitHub Actions synchronization-trigger deployment
- Invite-only official beta
- Operational documentation

## 12. v1 acceptance criteria

v1 is complete when:

1. A user can sign in with Google and connect selected Google calendars.
2. The user can create projects, tasks, subtasks, labels, notes, calendars, and events.
3. The daily command center combines today's events, due tasks, scheduled tasks, and up to three focus tasks.
4. Scheduling a task creates a linked calendar event.
5. Google changes synchronize in both directions without silently overwriting a detected conflict.
6. Day, week, month, and agenda calendar views work on desktop and mobile layouts.
7. Quick capture creates correctly parsed tasks for the supported grammar.
8. Search finds tasks, projects, notes, and events belonging to the current user.
9. File access is authorized and tenant-isolated.
10. In-app reminders are delivered while the application is open.
11. Users can export their data in the required portable formats.
12. The complete product runs through the documented Docker Compose setup.
13. The official hosted beta runs the same product behavior using the selected managed providers.
14. Automated tests cover critical domain rules, API behavior, synchronization, and user journeys.
15. No known critical security, data-loss, accessibility, lint, type-check, or test failures remain.
