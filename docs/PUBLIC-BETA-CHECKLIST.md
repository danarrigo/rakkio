# Public-beta release checklist

Reviewed 2026-09-11. This is a release gate, not evidence of Google approval, legal adequacy, uptime, or recoverability. Keep `INVITE_ONLY=true` and all existing account/storage limits unchanged until the owner has recorded the evidence below. No production changes, provider accounts, backup copies, or notification delivery were performed by this review.

## Implemented and locally verified

- Dependency security workflow: PR/main, daily best-effort, and manual audits of both lockfiles, with failure artifacts. Only the verification-only RSA advisory is temporarily accepted; see [SECURITY.md](../SECURITY.md).
- npm audit found and then cleared `GHSA-82fw-gwwq-j7x9` (Vitest/mocker) and `GHSA-2883-xcg3-v3hh` (js-yaml). Targeted updates use Vitest 4.1.11 and Redocly 1.34.20 -> js-yaml 4.3.2. Node 22 remains supported. Full frontend format, typecheck, lint, 66 unit tests, and build pass. No app or test migration was needed.
- Existing recovery workflow now checks `/ready` and its response status after the authenticated trigger. This only checks API/PostgreSQL when the job actually runs. Provider job errors can still produce HTTP 200 from the trigger.
- Source implements incremental Google connection, synchronization status/conflicts, portable exports, disconnect, account deletion, and tenant-scoped authorization. Source review and frontend tests are not a hosted end-to-end smoke test.

## Still unverified: owner release gates (in order)

### 1. Release and dependency evidence

- [ ] Run/review remote Rust formatting, clippy, workspace tests with PostgreSQL, generated OpenAPI/schema consistency, container checks, and end-to-end tests on the exact candidate commit. No Rust build/test/check/clippy or Docker was run locally for this review. Operations contracts were already reconciled in `8efa3fb` and merged in `c54e78b`; main CI run `34222767710` passed. That existing evidence does not validate this readiness change candidate.
- [ ] Run the new dependency workflow remotely, inspect **both** Rust reports and the npm JSON artifact, and require `rust-audit` and `npm-audit` in merge rules. Any finding blocks release; there are no dependency advisory exceptions. Test Actions failure notification receipt; a checked-in workflow does not enable merge rules or prove delivery.

### 2. Google OAuth brand, domain, scopes, and submission

Use the production Google Cloud project and OAuth client, not a new client created solely for the video. Consult Google's [brand verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/brand-verification) and [sensitive-scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification) requirements. Console labels can change; these are values and evidence, not guessed dashboard click paths.

- [ ] Confirm the app name/logo, monitored support email, and developer contact emails. Confirm public homepage `https://prosepect.com`, Privacy Policy `https://prosepect.com/privacy`, and Terms `https://prosepect.com/terms` load without sign-in and accurately identify the operator.
- [ ] Verify ownership of `prosepect.com` in Google Search Console using an account with the required project role, and configure the authorized domain. Confirm the production web-client redirect is exactly `https://api.prosepect.com/api/v1/auth/google/callback`; confirm Calendar API access is enabled in the intended project.
- [ ] Compare the configured and observed consent scopes with `apps/api/src/google_auth.rs::begin_authorization` and Google's [Calendar scope descriptions](https://developers.google.com/workspace/calendar/api/auth):

  | Flow                         | Exact requested scopes                                                       | User-facing purpose                                                                                                     |
  | ---------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
  | Sign-in                      | `openid`, `email`, `profile`                                                 | Verify identity and obtain verified email, display name, and avatar.                                                    |
  | Optional Calendar connection | Above identity scopes plus `https://www.googleapis.com/auth/calendar.events` | Read, create, update, and delete events for calendar and scheduled-task synchronization, subject to Google permissions. |
  | Optional Calendar connection | `https://www.googleapis.com/auth/calendar.calendarlist.readonly`             | Discover the user's calendar list and access roles; no calendar-list writes.                                            |

  Calendar authorization requests offline access and consent. Explain background synchronization and encrypted refresh-token retention. Do not request broader `calendar` access or claim that only sign-in scopes are used. Justify each scope and assess Google's current classification in the production project.

- [ ] Record and submit an English-language video accessible to Google reviewers, with the full consent flow and each requested scope visible. Use only separate test accounts and disposable calendars. Confirm the video matches the submitted app/client, not a mock. Do not expose tokens, OAuth client secrets, private user data, or owner dashboards.
- [ ] Submit the requested brand/scope materials, respond to Google's follow-up requests, and record actual approval. Keep access restricted and follow Google's publishing-status instructions during review; do not treat submission, a test-user login, or removal of an unverified warning as approval.

#### Suggested demo sequence and submission text

1. Show the public homepage, Privacy Policy and Terms, then the sign-in acceptance UI and Google identity consent in a fresh test-account session.
2. Open Settings. Show the Google Calendar disclosure before selecting **Connect Google**. Show the entire additional Calendar consent screen, expanding permissions so both scope uses are visible.
3. Return to Settings and **Discover calendars**. Show the calendar list in the app and the role-appropriate calendars available. Demonstrate an event flowing Google -> Prosepect, and a disposable local event or scheduled task flowing Prosepect -> Google. Update and remove the disposable time block; verify the result in both apps.
4. Show **Sync now**, queued/running/completed status, recent activity, and the conflict choices if reproducibly available. Wait for completion and compare actual data. A queued request or HTTP 200 alone is not a successful demo.
5. Show Settings exports (JSON, tasks CSV, notes Markdown, calendars ICS), **Disconnect Google Calendar**, and completed disconnection. Use a disposable account for account deletion, after exporting; confirm reauthentication is required afterward.

Draft to adapt only after the demo succeeds:

> Prosepect is a personal task, project, note, and calendar planner. Google sign-in uses OpenID Connect identity scopes. Users separately choose to connect Google Calendar in Settings after seeing the data-use disclosure. Calendar-list read access discovers calendars and access roles. Calendar-event access supports bidirectional event synchronization and Google time blocks for scheduled tasks, including updates and deletions. Offline access supports synchronization when the user is not actively browsing; stored OAuth credentials are encrypted. Users can inspect synchronization status, resolve conflicts, export their data, disconnect Calendar, or delete their account in Settings. The attached recording demonstrates the requested permissions and their implemented user-facing uses.

This draft is not a claim of Google approval or compliance. The owner must validate it against the deployed behavior and [Google API Services User Data Policy, including Limited Use](https://developers.google.com/terms/api-services-user-data-policy).

### 3. Independent encrypted backups and restore rehearsal

**Blocked prerequisites:** the saved `DATABASE_URL` is missing (already checked by the owner session). No database backup is possible from the available configuration. No independent encrypted destination or isolated restore target has been approved. Do not read `.env.deploy`, capture secrets into this checklist, or run the example backup commands until those prerequisites are resolved.

- [ ] Supply an authorized PostgreSQL connection securely, choose a compatible `pg_dump`/restore tool version, and approve read access to the R2 bucket. Approve an independent encrypted disk/storage destination, available free capacity, retention/rotation, and separately protected recovery keys. Include a recoverable copy of the credential-encryption key without storing it beside the encrypted archive in plaintext.
- [ ] Define a consistent capture window for database metadata and R2 objects, including treatment of concurrent writes/deletions. Back up both PostgreSQL and all referenced attachment objects into the encrypted destination. Provider-local recovery history and the production R2 bucket are not independent backups. See [Neon backup guidance](https://neon.com/docs/postgres/backup-restore/backups) and the direct commands in [OPERATIONS.md](OPERATIONS.md#backups-and-migration-to-a-server).
- [ ] Restore into an explicitly approved isolated database and object store, with outbound Google/webhook/notification traffic blocked and all synchronization dispatchers/workers disabled. Do not start a restored production dataset with live provider access. Verify decryption, migrations, record counts, representative object checksums/downloads, exports, and tenant isolation using authorized test identities.
- [ ] Record backup timestamp, encrypted archive inventory/checksums, restore duration, observed data-loss window, operator, and next rehearsal date in a private owner record. Agree recovery time/data-loss objectives. No real data or keys belong in GitHub artifacts or this public checklist.

### 4. External uptime, alert delivery, and free-tier budgets

- [ ] Choose/configure an independent free monitor for `https://rakkio.com` and `https://api.rakkio.com/health` (process liveness, not database readiness; frequent `/ready` probes prevent Neon idle suspension). Account creation and alert destinations require owner approval. Allow for Render cold starts, define repeated-failure thresholds, and test a controlled failure and recovery without deliberately taking production down.
- [ ] Verify receipt at the intended alert destination, including when the owner is not browsing `/operations`. Record the test time and recipient privately. Configure a missed recovery-run signal: the previous 15-minute Actions schedule was delayed by hours, and the current hourly schedule is also best-effort, and a job that never starts cannot report its own failure. [GitHub schedule caveats](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule) include delays/drops and inactivity disabling.
- [ ] Review failed/retrying synchronization jobs and oldest waiting work in Operations plus authorized logs. Define who investigates persistent backlog/provider errors. The worker's successful HTTP response and readiness check do not measure provider success or historical uptime.
- [ ] Review current Render instance hours, Neon storage/compute, R2 storage/operations, Vercel usage, and GitHub Actions usage in each provider's authenticated console. Configure available free budget/usage alerts and supported caps, document any provider with no hard cap, and verify delivery. No paid monitor or provider billing integration is required by this change. Existing account/file limits are not billing caps and are unchanged. Consult [Render Free limits](https://render.com/docs/free) and [R2 pricing](https://developers.cloudflare.com/r2/pricing/).

### 5. Legal review and separate-account hosted smoke

- [ ] Have the operator and appropriate legal reviewer assess the actual public Terms/Privacy text: operator identity/contact, jurisdiction, age requirements, Google Limited Use disclosures, subprocessors/transfers, retention (including backup expiry), rights requests, deletion behavior, and beta limitations. Legal pages existing in source is not legal adequacy.
- [ ] With two independently authenticated disposable Google accounts in separate browser profiles, test initial sign-in/acceptance, optional Calendar consent, inbound/outbound event edits, scheduled-task creation/update/unscheduling, and conflict resolution. Use private disposable calendars only, without real attendees. Observe actual completed data in both apps.
- [ ] Download and inspect JSON/CSV/Markdown/ICS exports; upload/download a disposable attachment. Confirm one account cannot read/change the other's task, project, note, calendar/event, file, synchronization status, or exports by substituting known test resource IDs. Confirm an ordinary user cannot access the owner Operations API. Keep cookies/CSRF tokens private.
- [ ] Disconnect Calendar and wait for revocation completion. Export then delete a disposable account, confirm session invalidation and removal of its data/objects through an authorized check, and verify the other account is unaffected. Record any deferred provider work or failures rather than assuming immediate success.
- [ ] Record commit, environment, date, tester, pass/fail evidence, and remaining issues privately. Only after all gates pass may the owner separately approve public registration, retaining the existing capacity limits. This checklist does not authorize that configuration change.
