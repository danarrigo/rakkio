# Discord task integration

Optional, user-installed Discord commands for the **same private tasks** shown in the web app. No chat history, message-content intent, AI, automatic reminders, or separate task database. No desktop download is needed.

Code being present does not mean an installation has enabled or verified Discord. Configuration and a real-provider acceptance check are separate operator steps.

## Hosted versus self-hosted

- **rakkio.com:** one official application, operated by the hosted service. Each person separately installs the Discord app and links their own existing rakkio account.
- **Self-hosting:** create a separate Discord application for that installation and supply its own credentials. The official hosted app does not connect to arbitrary third-party servers.
- Source, migrations, commands and setup instructions are public under AGPL-3.0. Credentials, account links and task data are not public. No production credentials belong in GitHub, client bundles, screenshots, issue reports or chat.

## User flow

1. Open **Settings > Discord**, check the command timezone and accept the separate data-sharing notice.
2. **Connect Discord** opens Discord's identity authorization. Return in the same browser while the initiating rakkio account is still signed in. Only `identify` is requested; not email, guilds or chat access. Access and refresh tokens are discarded after identity lookup.
3. Use **Install Discord app** and choose **Add to my apps**. Installation enables `applications.commands`, not server administration or the `bot` scope. Linking and installation are separate steps.
4. Choose rakkio's commands in Discord's command picker:
   - `/add title:Finish report due:tomorrow`: create one unfinished, projectless Inbox task. `due` is optional and accepts `today`, `tomorrow`, or `YYYY-MM-DD`. A date becomes **23:59 in the linked command timezone**, not a Calendar work block. Titles are literal: `#project`, labels, priorities and natural-language capture syntax inside the title are not parsed.
   - `/today`: up to 25 unfinished focus, due, overdue or scheduled-start-today tasks, using that timezone. This is a bounded summary, not the entire calendar or workspace.
   - `/done`: privately select one of up to 25 unfinished tasks, due tasks first. The menu expires after 15 minutes. Completion uses the version shown; changed/deleted tasks require a fresh selection. Recurring successors and saved forward defaults follow the normal task completion rules.
5. **Disconnect Discord** confirms and removes this installation's identity link and command receipts. Tasks and Google connections remain. It cannot erase messages or records already processed by Discord. Removing the app in Discord stops access to its commands but does not itself erase the local identity link.

An already-open web workspace may need a refresh to show Discord changes; save any drafts before reloading. Discord messages are snapshots, not live-updating task views.

Responses are ephemeral/private to the invoking user, including when invoked in a server, but are still processed by Discord. Explicitly requested task fields synchronized with Google Tasks or Calendar can be included. Descriptions, notes and attachments are not sent. Task titles are rendered as literal text, and automatic mentions are disabled.

A rakkio account can have one Discord identity, and a Discord identity can belong to only one rakkio account per installation. Disconnect before changing either account or the command timezone. Delayed events from before a new connection are rejected rather than executed against the newly linked account.

## Operator setup

Use a **dedicated** application, not an existing unrelated bot. These are optional settings; leave all four API values empty to disable the integration. Partial configuration fails startup.

| Setting | Where | Secret? |
| --- | --- | --- |
| `DISCORD_APPLICATION_ID` | API; Vercel relay if used | Public identifier |
| `DISCORD_PUBLIC_KEY` | API; Vercel relay if used | Public Ed25519 verification key, 64 hex characters |
| `DISCORD_CLIENT_SECRET` | API only; hidden registration prompt | **Secret** OAuth client credential |
| `DISCORD_REDIRECT_URI` | API and Discord OAuth2 redirect allowlist | Public exact callback URL |
| `DISCORD_BACKEND_URL` | Vercel relay only | Public full HTTPS endpoint ending in `/webhooks/discord` |

A bot token is **not required** for this command-only integration. Do not add `VITE_` to any of these names. CI uses generated signing keys and synthetic fixtures, not live Discord credentials.

### Discord Developer Portal

1. [Create an application](https://discord.com/developers/applications?new_application=true). For the hosted service, name it **rakkio**; self-hosters should identify their own installation clearly.
2. **General Information:** copy Application ID and Public Key. These identify the same application on the relay and API.
3. **OAuth2:** obtain its Client Secret privately and register the exact redirect URI:
   - Hosted: `https://api.rakkio.com/api/v1/auth/discord/callback`.
   - Same-origin self-host: `https://YOUR_HOST/api/v1/auth/discord/callback`.
4. **Installation:** enable **User Install**. This version registers user-install commands only; Guild Install is not needed. Set **Discord Provided Link** and the User Install default scope `applications.commands`.
5. Leave privileged Gateway intents disabled. No server administrator, member-list or message-content permissions are needed.
6. Deploy/configure the chosen endpoint below, then set **General Information > Interactions Endpoint URL** and save. Discord must verify the signed PING. Do not set this to the web SPA's HTML fallback.

For a hosted feature, obtain deployment approval **before** applying production environment changes or starting a deployment. Store the secret using the provider's private environment controls; never paste it into a public conversation.

### Sleeping API: Vercel relay

For the current Vercel/Render arrangement:

- Set the four API values on the API service.
- On the Vercel web project, set only `DISCORD_APPLICATION_ID`, `DISCORD_PUBLIC_KEY` and `DISCORD_BACKEND_URL=https://api.rakkio.com/webhooks/discord`.
- Discord's Interactions Endpoint URL is `https://rakkio.com/api/discord`.
- The supplied Node function verifies the original raw-body signature, timestamp and application, then returns a private deferred response. Backend work continues through `waitUntil`, bounded by the configured 120-second function duration.
- The relay forwards the original signed bytes. The API independently verifies them again, looks up the actual invoking Discord identity, applies the account's normal limits and executes the command.
- The API edits the original private Discord response after the transaction commits. Forwarding retries reuse the exact interaction ID and bytes. Task changes and successful response receipts commit atomically, preventing duplicate creation/completion.
- Failures are not turned into invented success. An uncertain delivery tells the user to inspect rakkio before submitting another command.

Discord requires an initial response within **three seconds** and permits interaction follow-ups for 15 minutes. The relay is designed to acknowledge without waiting for Render to wake; serverless cold starts, provider outages, quotas and networking can still cause failures. `waitUntil` is bounded background execution, **not a durable queue**. This is not an always-on or free-tier uptime guarantee.

### Always-on self-host: direct endpoint

Set the four API values in the installation's private environment file. For the personal Compose package, `.env.personal` accepts these optional values together and validates that the Discord callback uses its own HTTPS hostname. Keep the existing file owner/mode 0600 and all existing database/Google/encryption settings unchanged. The Discord secret is passed to the API, not the worker.

The supplied Caddy routes expose `/webhooks/discord`. Set Discord's Interactions Endpoint URL to `https://YOUR_HOST/webhooks/discord`. This direct mode does not need Vercel, a public bot process, a Gateway connection or another database. The API must be awake and fast enough to complete the initial command response within Discord's three-second deadline. Use a fast relay if that cannot be met.

Restart only the reviewed/configured services through the normal deployment procedure. Do not run parallel production instances against a restored database, and do not contact Discord during an isolated outbound-blocked restore drill.

### Register the commands

After the dedicated application's installation context is configured:

```bash
python3 scripts/discord/register_commands.py --application-id YOUR_PUBLIC_APPLICATION_ID
```

The script states the target and requires typing `REGISTER YOUR_PUBLIC_APPLICATION_ID` before sending requests. It then prompts for the **OAuth Client Secret** with hidden input. It obtains a short-lived `applications.commands.update` client-credentials token and registers only the three commands. It does not save or print credentials and refuses redirects.

**This replaces that application's complete global command list.** Do not run it against another bot. It is an operator-requested provider write, not a CI step; CI runs only offline safety tests. Registration does not install the app or link any rakkio account. Discord command availability and installation permissions are controlled by Discord.

## Safety and capacity

- Signed requests have a 64 KiB raw-body limit, a ten-second API read deadline and a five-minute timestamp window. The relay's body-read deadline is one second. Invalid signatures are rejected before JSON is trusted.
- Four dedicated webhook admission slots do not consume the public Google authentication pool. Linked commands also use the ordinary account concurrency, read/write and action limits.
- Browser-bound, owner-bound, single-use OAuth attempts expire after ten minutes; at most five pending attempts per account.
- At most 100 retained command receipts per connection; new commands are refused at capacity. Receipts expire after 15 minutes and are removed in bounded maintenance batches or on disconnect/account deletion. Receipts contain bounded response data, never Discord interaction/access/refresh tokens.
- Cached responses and task updates are one transaction; completion never silently rebases an old menu. Selection IDs and versions must have been offered to the same linked account. Account changes invalidate delayed commands and old menus.
- JSON account exports include the user's own Discord connection metadata, not OAuth state, tokens or command receipts. Account deletion cascades all integration records.
- Do not log raw requests, interaction tokens, OAuth codes, access tokens or Discord webhook URLs containing interaction tokens. Provider log retention and infrastructure limits still need operator review.

## Verification and release gates

Automated coverage includes signed HTTP commands against disposable PostgreSQL, owner isolation, replay protection, changed-task rejection, OAuth browser/owner/CSRF binding, mocked cold-backend acknowledgement, Settings opt-in/disconnect/account races, mobile/light/dark browser layouts and offline command-registration safety.

These tests do **not** prove a real Discord OAuth grant, account installation, command propagation, provider privacy behavior, Render cold-start recovery or real Discord reply delivery. Before describing an installation as working:

1. Verify the endpoint's PING in Discord.
2. Complete a real account link and user installation.
3. Run `/add`, check the same task in rakkio, run `/today`, then complete it with `/done`.
4. Change a task in the web app after opening `/done`; the stale selection must not overwrite it.
5. Verify private replies in the intended Discord contexts; no public task messages or unintended mentions.
6. Disconnect and verify commands no longer access the workspace.
7. For hosted Render Free, repeat a command after the API has slept and observe the acknowledgement and final result.

Only use an explicitly authorized test account/task for live acceptance. Never create production test data silently.

## Dependency compatibility

`packages/minimatch-compat` adapts patched upstream minimatch's modern named exports to the legacy callable CommonJS interface still required by Redocly 1.x through `openapi-typescript`. Replacing Redocly with 2.x directly breaks the generator's private import paths. The small adapter preserves both APIs, has a regression test, and keeps normal contract generation byte-stable. Root overrides also select patched glob and the Vercel SDK's compatible patched Zod. These are real dependency replacements, not advisory exceptions.

## Primary references

- [Discord application setup and user installation](https://docs.discord.com/developers/quick-start/getting-started)
- [Application commands and registration](https://docs.discord.com/developers/interactions/application-commands)
- [Interaction signatures, deferred responses and deadlines](https://docs.discord.com/developers/interactions/receiving-and-responding)
- [Discord OAuth2](https://docs.discord.com/developers/topics/oauth2)
- [Vercel function background work](https://vercel.com/docs/functions/functions-api-reference/vercel-functions-package)
