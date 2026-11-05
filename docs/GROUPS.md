# Groups

Status: staged implementation. Handles and gated membership are merged through PR #39. Shared assignments and private progress are in draft PR #40; calendar integration and Discord group agendas remain subsequent phases in [GROUPS-PLAN.md](GROUPS-PLAN.md). Do not describe the whole feature as released.

## Rollout

`GROUPS_ENABLED` is a server-side boolean, currently **false by default**. All membership interfaces except the authenticated availability check reject access while disabled. Navigation remains hidden, and no group data is fetched merely by signing in. Do not enable production groups before the complete feature, privacy notices and release acceptance gates are ready. There are no new infrastructure providers or credentials.

## Membership model

- One owner and multiple leaders. The owner is always an active member and also has leader capabilities.
- Owners control group settings, roles, removal, ownership offers and group deletion.
- Leaders manage invitations and review join requests. They cannot appoint leaders, remove members, delete the group or transfer ownership.
- Members view the shared roster and manage only their own participation. Everyone's personal workspace remains private.
- Public roster fields are display name (bounded to 120 characters), handle and role. Emails, other members' progress, private preferences and invite hashes are not projected.
- Creating/joining requires an explicit roster-sharing acknowledgement with the notice version `2026-11-04` pinned to the UI copy. This is separate from the global Privacy Policy and Discord sharing versions; never automatically echo a future server version without the matching disclosure. Accepted versions and joining time are stored; approving a pending request preserves its originally accepted version rather than upgrading it to the approving server's current notice.

## Joining

1. Choose a unique @handle in Settings. Existing personal workflows do not require a handle.
2. A leader may invite an exact handle, generate a link, or generate a code.
3. Review the group and the account you are joining as. Previewing never creates membership or consumes a use.
4. Confirm the roster disclosure. Direct invitations require recipient acceptance. Links/codes require leader approval by default; the owner may choose immediate confirmed joining.

Invitations grant ordinary membership only. Addressed invitations resolve immutable user IDs, not mutable display names or email addresses. A direct recipient may decline without joining.

Links have 32 random bytes of entropy. Codes contain 12 unambiguous Crockford characters (60 bits), are case-insensitive, and accept pasted spaces/hyphens. Credentials are stored only as SHA-256 hashes, expire after seven days, and may be revoked. Usage is counted only when membership is admitted, not when it is previewed/requested. Admission rechecks revocation, expiry, capacity and member state under locks.

Invite URLs use `/join#invite=...`: the fragment is not sent to the server and is removed before further navigation. A valid link credential may be kept in tab session storage for at most 15 minutes to survive sign-in; it is never added to Google OAuth URLs. Clearing the account or expiry removes that pending credential. Storage-disabled browsers may reopen the original link after signing in. The new credential is shown only when issued; reload does not recover its plaintext.

## Role and lifecycle safety

- Mutations bind the expected signed-in user and current entity versions. A delayed form from a different account cannot silently write for the new account.
- Ownership transfer is an offer to an active member, followed by that recipient's explicit acceptance. The former owner remains a leader. Acceptance rechecks ownership/version, target membership, expiry and leader capacity.
- Leaving/removal clears group sharing preferences and personal group progress. Rejoining starts with a fresh membership identity and ordinary member privileges; it does not restore old progress or leadership.
- Removed members cannot bypass removal with an old code, link, invitation or pending request. The owner must first explicitly allow a fresh invitation; that action does not enroll the person.
- A member may leave, but an owner must transfer or delete the group first. Demotion/removal invalidates invitations issued under that member's former authority.
- Deleting a group requires explicit `DELETE GROUP` confirmation. It becomes inaccessible immediately, with a 60-second Undo window. Undo is version-bound and requires unchanged membership, including no intervening member account erasure. It never recreates an erased account. Expired groups are purged by bounded maintenance.
- Account deletion refuses owned groups before any Google revocation or object deletion. After eligibility is checked, group admissions are persistently blocked while external erasure is in progress. If cleanup fails, retry deletion to finish; do not acquire new ownership during erasure. Account erasure never silently deletes a class owned by somebody else.
- JSON exports include currently authorized group summaries/shared assignment definitions and only the requester's progress and membership/consent metadata. Shared definitions are streamed in bounded pages, rechecking active membership on each page; access revocation stops subsequent pages. Previously delivered/downloaded bytes cannot be recalled. It does not expose other members' private progress, preferences, email addresses or invite credentials.

## Initial capacity limits

Metadata has the fixed limits below. Phase 3 adds transactional shared-content/progress accounting, separate from personal per-user budgets but included in the same global budget.

| Resource                                                          | Limit                                 |
| ----------------------------------------------------------------- | ------------------------------------- |
| Hosted group records, including recoverable deletions             | 100                                   |
| Groups owned per user                                             | 5                                     |
| Active memberships per user, including recoverable deleted groups | 20                                    |
| Active members per group                                          | 100                                   |
| Leaders per group, including owner                                | 10                                    |
| Membership records per group, including departed/removed states   | 500                                   |
| Invitation records per group                                      | 50                                    |
| Join-request records per group                                    | 100                                   |
| Ownership offers                                                  | One per group, expiring after one day |

The existing `MAX_USER_ACCOUNTS=100` hosted admission cap is unchanged. Group invitations never create accounts or bypass signup consent, age requirements or registration capacity. Credential attempts are authenticated and rate-limited; invitation records do not contain raw codes/links. Cleanup removes bounded batches of expired/revoked metadata. Consumed/revoked invitations are retained briefly for reconciliation, not indefinitely as chat history.

## Shared assignments and private progress (phase 3)

- Leaders publish one canonical assignment, not personal task copies. Titles are limited to 240 characters and descriptions to 10,000. Shared instructions allow sanitized Markdown formatting and explicitly opened links, not automatic external media or interactive HTML. Assignments do not reserve work time and do not sync to Google Tasks.
- A date-only deadline uses 23:59. The deadline timezone defaults to the group's timezone at publication; an explicit IANA timezone can be supplied. The UI sends the displayed choice, so concurrent group-setting changes cannot silently reinterpret a draft. The saved timezone is retained through later group-setting changes; an explicit deadline-timezone edit is needed to change it. Nonexistent DST local times are rejected; ambiguous times use PostgreSQL's standard-time interpretation.
- Open/closed lifecycle is distinct from each member's completion. Closing preserves history and progress without marking anyone complete. Leader edits preserve completion and expose that the definition changed; members can reopen their own work.
- Progress is sparse and bound to the current membership identity. Writes check both assignment and individual-progress versions. A stale account, definition, progress version or previous membership cannot silently overwrite current work.
- Sharing is off initially. Enabling it requires the separate notice `2026-11-04-progress`: the owner and all current/future leaders can see progress for that group, never peers. Turning it off removes live leader access; leaving/removal purges the progress and sharing state.
- Only opted-in members appear in leader reports. No private-progress counts, activity, roster versions or public membership epochs reveal unshared changes. Exports never include anybody else's progress, even when the exporter is a leader.
- Assignment deletion is confirmed and recoverable for 60 seconds, with a version/membership-epoch-bound receipt. No permanent-delete fallback exists. Recoverable deletions still consume capacity until bounded purge; membership changes prevent stale Undo.

### Assignment interface

From a selected group, open **View assignments**. Open work and closed history are independently paginated, newest first. Leaders can publish/edit/close shared definitions and view only opted-in progress; all members can complete/reopen their own work. Sharing requires a separate, initially unchecked acknowledgement and explicit action. Refresh clears that acknowledgement and any open leader report; it never copies a server notice version into consent.

Conflicts retain the assignment draft. Refresh fetches the edited definition by exact ID, even when it is outside the current list page, and adopting a changed version requires explicit confirmation after reviewing current values. Lost access retains a local draft for copying but disables saving. Account changes clear private state. An unconfirmed write is never retried automatically; refresh first to reconcile. A confirmed save followed by refresh failure is distinguished from an unknown result. Navigation warns about unsaved drafts or in-flight changes.

### Shared content capacity

Each group has **16 MiB / 20,000 logical records**. Existing hosted global limits remain **256 MiB / 100,000 logical records**, shared with personal content. Group-owned content is not charged to an individual user's personal budget and does not become that person's property.

An assignment costs one definition record plus one reserved progress record for every active member. Each progress slot reserves 512 bytes; the definition uses serialized variable-content size plus 256 fixed lifecycle bytes. These are logical reservations, not pre-created personal tasks or progress rows. New members reserve slots for retained assignments, including history and recoverable deletions. Admission/publication fails atomically if group/global capacity is insufficient; it does not consume an invitation or partially enroll a member.

Reserved capacity allows existing members to complete/reopen work when the budget is full, without quota changes revealing private activity. Closing, deleting and restoring unchanged content also need no new lifecycle capacity. Leaving/erasure/purge releases reservations. Deferred accounting applies final transaction-net deltas across personal/group scopes, locking the global counter before scoped counters. Future shared-content migrations must preserve this accounting and recount changed tracked row shapes.

## Engineering / verification

Canonical definitions are group-owned, not copied into personal tables and not owned by a fake user. Personal task/calendar ownership constraints remain unchanged. Composite foreign keys bind group memberships, invitations, join requests and ownership offers to the same group; a deferred constraint ensures an active owner at commit.

Group writes lock affected users in UUID order before the group. Admission/capacity, ownership, removal and account-erasure races must be tested against actual PostgreSQL. Group detail uses one repeatable-read snapshot and selects no private progress fields. Static SQL literal composition retains SQLx's query-safety check; do not introduce dynamic data interpolation or bypass it with an audit assertion.

Required release gates remain backend, frontend, containers, worker-http, end-to-end, personal-self-hosting, rust-audit, npm-audit and secret-scan. Backend/container tests run remotely only; use actual Rust exporter artifacts for OpenAPI and normal TypeScript generation. Browser/mocked/disposable tests do not establish real-account acceptance or hosted recovery.
