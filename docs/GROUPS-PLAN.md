# Groups implementation plan

Status: **implementation approved** by the operator after reviewing the plan. Deliver in reviewable phases; do not advertise the complete groups feature until all required phases pass their acceptance gates.

Product vocabulary: [CONTEXT.md](../CONTEXT.md). Scope: college-class-style shared planning, not a learning-management system or a copy of a leader's account.

Delivery: phase 1 is merged as PR #38 and phase 2 as PR #39. Phase 3 shared assignments/private progress is implemented in PR #40, with server rollout still disabled by default; see [GROUPS.md](GROUPS.md) for concrete limits, invitations, consent and lifecycle behavior. Calendar composition, Discord group sharing and final release remain subsequent phases, not completed features.

## 1. Agreed product behavior

- Users choose unique, case-insensitive `@handles`; display names may repeat and emails remain private.
- Groups support direct invitations by handle, invite links and join codes. Joining requires acceptance or confirmation, never automatic enrollment.
- One owner controls leader appointments, membership, ownership transfers and group deletion. Multiple leaders manage shared assignments, events and invitations.
- Shared content is explicitly created for the group. Personal content remains private, including the owner's and leaders' content.
- Members see the same shared definitions and subsequent leader edits, not disconnected personal copies.
- Each member completes assignments independently. Personal progress is private by default; members can optionally share it with the group's leaders.
- Group items appear with a group label alongside personal items in the web agenda/calendar and Discord `/today`.

## 2. Initial defaults

Proceed with the recommended defaults following the operator's instruction to create the feature. Any material change to privacy or membership behavior needs explicit review:

| Question                          | Recommended first-release behavior                                                                                                                                                                                          |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| What does a new member receive?   | Currently open assignments, including overdue work still marked open; upcoming/ongoing events and future recurring occurrences. Past events remain browsable in the group, but do not flood Today. No inherited completion. |
| Can anyone with a link/code join? | Owner chooses immediate confirmed joining or leader approval. Default to approval required. Neither route grants leader privileges.                                                                                         |
| How does progress sharing work?   | One opt-in per member per group, clearly described as visible to the owner and all current/future leaders. Turning it off immediately removes live leader access.                                                           |
| Who sees the roster?              | Members see other members' display names, handles and roles, not emails. No progress visibility to peers. Explain roster exposure before joining.                                                                           |
| Can a leader close an assignment? | Yes. Closing hides it from active work for everyone without marking members complete; history and personal progress remain.                                                                                                 |
| What if completed work is edited? | Preserve completion, show that the assignment changed, and let the member reopen it. A materially new assignment should be published separately.                                                                            |
| Recurring assignments?            | Defer to a later release. Initial assignments are one-off; recurring class events are supported. Never reuse personal recurrence completion, which advances a single user's task.                                           |
| Can handles change?               | Initially claim once; defer renaming and handle recycling until a safe rename/quarantine policy is agreed. Invitations always bind to user IDs, not mutable strings.                                                        |

## 3. Concrete experience

1. Daniel chooses `@daniel`, creates **Computer Science 2026**, and becomes owner.
2. He appoints a second leader. They add a recurring lecture and publish an assignment with a deadline.
3. A classmate accepts a handle invitation, confirms a join link/code, or awaits approval if required.
4. Group items appear with the **Computer Science 2026** label in that classmate's agenda. The lecture is one shared event, not a copied appointment.
5. A leader moves the lecture. Members see the new time on refresh/live refresh without duplicated events or overwritten personal plans.
6. A member completes the assignment. Other members still see their own unfinished work. Leaders cannot see that completion until the member opts in.
7. Leaving/removal removes future access and agenda inclusion. It does not delete or modify personal tasks/events.

## 4. Existing code and constraints

- `apps/api/src/models.rs`: `UserProfile` currently has no handle. Tasks have a single status/completion timestamp, which cannot represent each classmate's progress.
- `migrations/0001_foundation.sql` and `0008_calendars_and_events.sql`: tasks/calendars/events are user-owned, with composite ownership constraints. Do not weaken those constraints or make a group a fake user.
- `migrations/0011_invites.sql`: account-registration invitations are email-based and are not group invitations. Keep these concepts separate; group invites never bypass account limits.
- `apps/api/src/discord_store.rs` and `discord_agenda.rs`: `/today` is currently owner-scoped; `/done` uses version/link-bound selections and ordinary personal-task recurrence. Preserve atomic operation/receipt commits and replay safety.
- `apps/api/src/export_store.rs`: account deletion and exports currently assume personal ownership. Group ownership needs an explicit policy before any memberships ship.
- `migrations/0026_workspace_content_limits.sql`: existing counters track specific personal tables. New group records need explicit bounded accounting, not a quota bypass or a charge for every projected agenda copy.
- Frontend integration points: `router.ts`, `AppSidebar.vue`, `SettingsView.vue`, `TodayView.vue`, `CalendarView.vue`, and `stores/workspace.ts`.
- Use new forward migrations after deployed `0028`; never rewrite deployed migrations. No local Rust builds/tests/clippy or Docker. Backend/container tests run remotely; contracts come from real remote Rust exporter artifacts and normal TypeScript generation.

## 5. Data ownership and module design

### Shared definitions, personal progress

Keep personal tasks/calendars unchanged. Add group-owned content separately:

- Users: optional normalized handle during migration, case-insensitive database uniqueness, explicit claiming. No generated handles based on private email addresses.
- Groups: name, timezone, owner, version and lifecycle state.
- Memberships: unique `(group, user)`, leader/member role, personal progress-sharing choice and version. Owner capabilities derive from the group's owner, avoiding contradictory owner-role records.
- Invitations: addressed user or link/code credential, issuer, expiry, revocation, usage limits and intended member role. Store link/code credential hashes, not plaintext credentials.
- Join requests when approval is required: unique active request per group/user, with explicit approve/reject transitions.
- Shared assignments: group, title, description, deadline, open/closed lifecycle and version. A deadline does not reserve work time.
- Personal progress: unique `(assignment, member)` completion and version; absence means incomplete. Content edits and progress edits have separate concurrency checks.
- Group events: group, title, details, start/end or all-day dates, event timezone, supported recurrence and version.

Composite constraints must prevent progress or invitations from referencing content/memberships in another group. Index membership lookups, open assignments and event ranges. Bound groups per user, members, invitations, join attempts, content and pagination; choose numerical limits before implementation. Preserve the existing 100-account cap.

### Small interfaces, centralized rules

Build a deep groups module around authenticated-user operations for membership, invitations, shared content and personal progress. Authorization, locking, quotas and lifecycle transitions belong behind its interface, not repeated in Vue, Discord and route handlers.

Add a read-only agenda composition seam that merges authorized personal and group projections. Return explicit origin, stable source identity, group label and edit/completion capabilities. Do not serialize a shared assignment as a personal `Task`, invent an owner ID, or expose a writable personal-task endpoint for group items.

Reuse existing date/recurrence behavior through focused internal helpers where two real callers benefit. Avoid a generic collaboration framework, event bus, separate database, background copy fan-out or new infrastructure.

## 6. Security and lifecycle requirements

### Handles and invitations

- Proposed handle syntax: 3-30 ASCII letters/digits/underscores, canonical lowercase, optional leading `@` stripped for entry. Reserve administrative/impersonation-prone names. Confirm the policy before exposing claims.
- Claim atomically; case variants and concurrent claims cannot create duplicates. Existing accounts retain login and personal access until they choose a handle.
- Signed-in, exact-handle invitation only; no public user directory, email lookup, autocomplete harvesting or unauthenticated enumeration. Responses expose only needed public identity; rate-limit lookup and invitation attempts.
- Recipient-bound invitations resolve to immutable user IDs. A guessed handle never grants membership.
- Links use high-entropy secrets; short codes need sufficient entropy, strict attempt limits, expiration and rotation. Neither is an account authentication credential.
- GET previews never join, consume or approve. Acceptance/approval is authenticated, CSRF-protected, transactional and idempotent, including concurrent joins, revocation and capacity races.
- Keep invite credentials out of logs, referrers, analytics and OAuth redirect URLs. Joining after signup must still satisfy normal legal consent and account admission limits.

### Roles, privacy and removal

- Enforce permissions on every read, mutation, search, export, agenda and Discord operation, not only in navigation.
- Leaders may invite ordinary members but cannot appoint leaders, transfer ownership, delete the group or remove/promote the owner. Role changes are owner-only and version-bound.
- Require an active member's explicit acceptance for ownership transfer. Serialize transfer, departure, account deletion and demotion so no group becomes ownerless or gains two owners.
- Account deletion must require transfer or separately confirmed group deletion for owned groups. Never cascade-delete classmates' shared content merely because its creator deletes an account.
- Private progress must not leak through counts, activity feeds, exports, roster ordering, notifications or cached responses. Sharing with leaders is not permission to expose it to peers or the public.
- On leave/removal, purge the member's group-specific progress/sharing state and reject stale completion selections. Rejoining begins fresh unless a different retention policy is explicitly approved. Do not pretend previously downloaded/exported content can be recalled.
- Group deletion needs clear impact confirmation and an explicit recovery/Undo policy. Use bounded, version/dependency-checked receipts consistent with existing deletion behavior; no silent destructive fallback.
- A personal export includes the user's own progress and currently authorized shared definitions, never other members' private progress. Group export and account-erasure rules must be documented and tested separately.
- Group content is native to rakkio initially. No automatic Google Tasks/Calendar copies, leader-account imports, email delivery, chat or file sharing.

## 7. Delivery phases and acceptance gates

Each phase should be a reviewable PR, not one large merge that temporarily exposes unprotected shared data. Keep incomplete behavior inaccessible behind a server-controlled rollout gate.

### Phase 1: handles

Implementation in PR #38 uses a separate authenticated own-handle projection, preserving existing session/profile consumers. Claims include the expected signed-in user ID to reject stale-account writes. The registry is capped at 10,000 lifetime reservations; account deletion clears the plaintext handle and profile link, retaining only its SHA-256 reservation. These metadata records have their own fixed cap rather than bypassing an unbounded personal-content budget. Handle claims alone do not enable group membership or shared content.

- Profile claim controls, uniqueness/reserved-name validation, minimal public identity projection and session/profile contracts.
- Acceptance: case collision/concurrent claim tests, no email leakage, unchanged login/personal workflows, old account migration, mobile/desktop keyboard-accessible controls.

### Phase 2: groups, roles and joining

- Create/list groups, membership controls, multiple leaders, ownership transfer, direct invitations, links/codes and approval workflow.
- Establish group-scoped authorization, quotas, deletion/account-erasure/export rules and lifecycle tests before exposing the group surface.
- Acceptance: owner/leader/member/outsider permission matrix; guessed IDs; concurrent last-slot joins; duplicate acceptance; revoked/expired credentials; approval/removal races; owner deletion/transfer; no registration-cap bypass.

### Phase 3: shared assignments and private progress

- Publish/edit/close assignments; complete/reopen only one's own progress; explicit progress sharing and restricted leader view.
- Acceptance: two members can have different completion states; leader edits propagate without resetting completion; stale edits fail safely; private progress absent from every unauthorized projection; removal revokes access.

### Phase 4: group events and web agenda

- Timed/all-day and recurring events; group labels/visibility controls; personal-plus-group Today/calendar composition; read-only group details for ordinary members.
- Use the group's/event's timezone for scheduling and the viewer's timezone for display/day intersection. Date-only deadlines remain tied to the group's calendar date, not browser UTC guesses.
- Keep group assignment personal work blocks out of this first release rather than silently treating deadlines as reserved time.
- Acceptance: edit propagation, personal/group ID collisions, selected-group filtering, DST 23/25-hour days, overnight spans, all-day exclusive ends, recurrence scope/Undo, loading/empty/error states and 320px/light/dark layouts.

### Phase 5: Discord

- `/today` includes labeled group assignments/events only after explicit group-data sharing acceptance. New joining groups do not automatically opt their content into Discord: show group identity/audience notice and use per-group inclusion controls, initially off.
- Keep `/add` personal Inbox-only. Extend `/done` using typed personal/group targets; completing a group assignment updates only the invoking member's progress.
- Do not expose descriptions, locations, attendees, rosters or others' progress. Preserve private responses, signature/actor/link checks, mention suppression, UTF-16/byte bounds and atomic operation/receipt commits.
- Recheck membership and sharing before serving group-derived cached receipts or processing old selections. Revoke undelivered group replies on removal where possible; previously delivered Discord messages cannot be recalled reliably. Preserve committed-operation idempotency without replaying now-unauthorized shared text.
- Acceptance: signed PostgreSQL HTTP tests for mixed agendas, independent completion, stale membership/role/version, consent, bounded output, duplicate interactions and delivery retries. Do not mutate production classwork as a test.

### Phase 6: release

- Update privacy/sharing notices, group lifecycle documentation and self-host instructions; verify exports, quota accounting, deletion/Undo and recovery on disposable data.
- Generate contracts from exact remote Rust exports, then normal TypeScript generation. Run full backend, frontend, container, worker, self-host, browser and security gates through ordinary protected PRs.
- Verify exact intended Render/Vercel deployments and live schema. With consenting test users, verify a real invitation, event update, private independent completion, removal and fresh Discord agenda.
- Do not claim hosted backup recovery, real-account acceptance or durable Discord delivery from mocks/CI alone. Preserve unrelated calendar test changes, credentials and absence of local `target/` throughout.

## 8. Explicitly outside this first release

Grading, assignment submissions, peer-visible completion, attendance, recurring assignments, per-member deadline overrides, personal work blocks for shared assignments, Google group sync, billing, institutional SSO, email invitations/notifications, public group discovery, chat, attachments and AI.

## 9. Implementation order

Start with phase 1, then proceed through the remaining phases. Initial defaults are **approval-required link/code joins** and **per-group opt-in progress sharing visible to all leaders**. Document numerical capacity limits and the concrete group deletion recovery policy before phase 2 exposes membership/content. Each phase must preserve existing personal workflows and pass its release gates; partial delivery is not completion of the whole feature.
