<script setup lang="ts">
import PublicPageShell from '../components/PublicPageShell.vue'
</script>

<template>
  <PublicPageShell
    title="Privacy Policy"
    summary="How rakkio collects, uses, stores, and protects personal data, including information received through Google APIs."
    last-updated="November 3, 2026"
  >
    <section>
      <h2>1. Who operates rakkio</h2>
      <p>
        rakkio is an independently operated productivity service based in Indonesia. The data
        controller is Daniel, the independent operator of rakkio. Questions and personal-data
        requests can be sent to
        <a href="mailto:daniel.manurung.dev@gmail.com">daniel.manurung.dev@gmail.com</a>.
      </p>
    </section>

    <section>
      <h2>2. Data rakkio collects</h2>
      <h3>Account information</h3>
      <p>
        When you sign in with Google, rakkio receives your Google account identifier, email address,
        display name, and profile image. This information creates and secures your account. rakkio
        also records the accepted Terms and Privacy Policy versions, acceptance time, and
        confirmation that the account holder meets the stated age requirement.
      </p>

      <h3 id="handles">Optional public handle</h3>
      <p>
        You can choose a unique, case-insensitive handle for group invitations as Groups becomes
        available. Choosing one is optional for existing personal workflows. It is a public
        identity, not an email address or a Discord account, and does not create a public account
        directory. Handles cannot currently be renamed or transferred. After account deletion, the
        plaintext handle and its account link are removed; a one-way SHA-256 reservation remains to
        prevent someone else from claiming the same identity. This reservation contains no profile
        identifier or timestamp. It is not a promise that a hash is anonymous, and it is not used
        for advertising.
      </p>

      <h3>Workspace content</h3>
      <p>
        rakkio stores content you choose to create or import, including tasks, projects, notes,
        labels, schedules, reminders, daily plans, calendar events, recurrence information,
        attendees, locations, time zones, comments, and attachments. Content may include personal
        data about other people if you enter it.
      </p>

      <h3>Google Calendar data</h3>
      <p>
        Connecting Google Calendar is optional. If you connect it, rakkio receives your calendar
        list and access roles and reads, creates, updates, and deletes calendar events as needed to
        provide the synchronization features you request. This can include event titles,
        descriptions, dates and times, recurrence, attendees, locations, time zones, calendar
        identifiers, and synchronization metadata.
      </p>
      <p>
        rakkio stores encrypted Google access and refresh credentials, provider identifiers, and
        synchronization state so synchronization can continue while you are not actively using the
        site. Raw Google API responses are processed transiently rather than retained as duplicate
        payloads.
      </p>

      <h3>Google Tasks data</h3>
      <p>
        Google Tasks is a separate, optional connection. Granting its permission does not start
        copying tasks: you choose a list and enable synchronization in Settings. rakkio reads your
        task-list names and identifiers to let you select a list, then reads and synchronizes task
        titles, calendar dates, and completion states in that list. It may create the rakkio list
        and task copies you request. Exact deadline times and scheduled Calendar work blocks remain
        separate.
      </p>
      <p>
        rakkio stores task identifiers, synchronization baselines, recovery references, and
        conflicting versions of these shared fields to avoid duplicate creation and unintended
        overwrites. Google task notes are read transiently to recognize recovery references; rakkio
        adds a reference to newly created copies but does not synchronize your private descriptions,
        notes, or attachments to Google Tasks. These data are used only for the synchronization you
        enable, not advertising, sale, or AI training.
      </p>

      <h3>Discord integration</h3>
      <p>
        Discord is optional and requires a separate account connection and data-sharing notice in
        Settings. rakkio requests only your Discord identity, stores your Discord user identifier,
        username, chosen command time zone, connection time, and notice version, and links them to
        your existing rakkio account. It does not request access to your chat history or retain
        Discord access or refresh tokens after checking your identity.
      </p>
      <p>
        When you use a command, Discord processes the task and calendar information you request,
        including task titles, deadlines, task selections, completion actions, and calendar event
        titles and start/end times. This may include tasks synchronized with Google Tasks or
        Calendar and events from selected Google calendars. Event descriptions, locations and
        attendees are not sent. Existing task-only Discord connections do not share events until you
        accept the updated sharing notice in Settings. Responses are private to the invoking user by
        default, but Discord still processes that information under its own
        <a href="https://discord.com/privacy">privacy policy</a>. There are no automatic Discord
        reminders or AI processing. Temporary command receipts prevent duplicate changes and keep
        task selections bound to the version you saw.
      </p>

      <h3>Technical and security information</h3>
      <p>
        rakkio and its hosting providers process request information needed to deliver and secure
        the service, such as network address, browser and device metadata, request route, response
        status, timestamps, and diagnostic errors. rakkio uses short-lived network identifiers for
        rate limiting and records limited account activity for synchronization and security
        visibility. It does not use third-party advertising or behavioral analytics.
      </p>

      <h3>Browser storage</h3>
      <p>
        rakkio uses a secure session cookie for authentication and browser storage for interface
        preferences, dismissed reminders, and development-only authentication when running a local
        development build.
      </p>
    </section>

    <section>
      <h2>3. Why rakkio processes data</h2>
      <p>rakkio processes personal data to:</p>
      <ul>
        <li>authenticate you and maintain your account;</li>
        <li>
          provide tasks, projects, notes, calendars, attachments, exports, and daily planning;
        </li>
        <li>perform the Google Calendar and Google Tasks synchronization you enable;</li>
        <li>perform private task commands through Discord when you separately connect it;</li>
        <li>
          protect accounts, prevent abuse, diagnose failures, and maintain service reliability;
        </li>
        <li>respond to support, privacy, and legal requests; and</li>
        <li>comply with applicable law.</li>
      </ul>
      <p>
        Depending on the processing, the legal basis is performance of the service you request, your
        consent, compliance with legal obligations, or the legitimate interest in operating a secure
        and reliable service while respecting your rights. You may stop optional synchronization in
        Settings or revoke Google access at any time. You can independently disconnect Discord in
        Settings without affecting Google access or your tasks.
      </p>
    </section>

    <section>
      <h2>4. How Google user data is used</h2>
      <p>
        rakkio uses Google user data only to provide prominent, user-facing account, calendar, and
        task functionality. rakkio's use and transfer to any other app of information received from
        Google APIs adheres to the
        <a href="https://developers.google.com/terms/api-services-user-data-policy"
          >Google API Services User Data Policy</a
        >, including the Limited Use requirements.
      </p>
      <p>Google user data is not:</p>
      <ul>
        <li>
          sold or transferred to advertising platforms, data brokers, or information resellers;
        </li>
        <li>used for advertising, retargeting, lending, or credit decisions;</li>
        <li>used to train generalized artificial-intelligence or machine-learning models; or</li>
        <li>
          read by a human except with your specific permission, when necessary to investigate abuse
          or a security problem, or when required by law.
        </li>
      </ul>
    </section>

    <section>
      <h2>5. When data is shared</h2>
      <p>rakkio does not sell personal data. Data is shared only:</p>
      <ul>
        <li>
          with Google when you sign in or direct rakkio to synchronize calendar or task information;
        </li>
        <li>
          with infrastructure providers that host or deliver the service: Vercel for the web
          application and Discord command relay, Render for the API, Neon for PostgreSQL, and
          Cloudflare R2 for attachments;
        </li>
        <li>when you direct rakkio to place information on a shared Google calendar;</li>
        <li>
          with Discord when you connect your account or request task information and actions through
          Discord commands; this can include task information already synchronized with Google Tasks
          or Calendar, plus selected calendar event titles and times after your separate sharing
          acceptance;
        </li>
        <li>when necessary to protect the service or investigate abuse; or</li>
        <li>when required by applicable law.</li>
      </ul>
      <p>
        These providers may process data outside Indonesia. rakkio uses provider terms and
        safeguards intended to protect data transferred across borders. Where applicable law
        requires additional consent for a transfer, rakkio will request it before that transfer.
      </p>
    </section>

    <section>
      <h2>6. Retention and deletion</h2>
      <p>
        Account and workspace data is retained while your account remains active or as needed to
        provide the service. Operational logs are retained only as long as reasonably needed for
        security, diagnosis, and provider operations. Where backup copies exist, they are isolated
        from ordinary use and scheduled to expire within 30 days unless a longer period is legally
        required.
      </p>
      <p>
        You can disable Google Tasks sync in Settings without revoking Calendar access. Disabling or
        unlinking keeps existing tasks in both apps and retains mapping and recovery metadata to
        prevent unintended duplicate creation or reimport. Deleting a task in either app does not
        delete its Google Tasks counterpart.
      </p>
      <p>
        Disconnect Google in Settings revokes the shared Google authorization once the queued
        disconnection completes. This stops both Calendar and Tasks access and removes stored Google
        credentials and Google-sourced calendar records. It does not delete your Google Tasks copies
        or imported rakkio tasks. You can also revoke access from your Google Account. Retained task
        mappings and recovery data are removed when your rakkio account is deleted.
      </p>
      <p>
        Disconnecting Discord stops future commands and removes the local identity link and command
        receipts; existing rakkio tasks remain. Account deletion also removes the link. Temporary
        command receipts expire after 15 minutes and are removed by bounded maintenance or on
        disconnect. Incomplete connection attempts expire after ten minutes. Disconnecting rakkio
        cannot remove messages or other records already processed by Discord; manage those under
        Discord's controls and retention policy. You can also remove the app from Discord's
        settings.
      </p>
      <p>
        You can delete your rakkio account from Settings. Account deletion revokes stored Google
        authorization, removes attachment objects, and deletes your account and user-owned records
        from active rakkio systems. Limited security or provider records may remain until their
        normal retention period expires or when retention is legally required. The handle
        reservation described above remains solely to prevent identity reuse, without retaining the
        plaintext handle or account link.
      </p>
    </section>

    <section>
      <h2>7. Your choices and rights</h2>
      <p>Depending on applicable law, you may request to:</p>
      <ul>
        <li>access and receive a copy of your personal data;</li>
        <li>correct or update inaccurate information;</li>
        <li>delete data or end, restrict, or object to processing;</li>
        <li>withdraw consent without affecting earlier lawful processing;</li>
        <li>receive portable data in a commonly used machine-readable format; and</li>
        <li>raise a complaint with the competent personal-data protection authority.</li>
      </ul>
      <p>
        rakkio provides JSON, CSV, Markdown, and iCalendar exports in Settings. For requests not
        available in the interface, email the address above from your account email. Identity may
        need to be verified before a request is completed.
      </p>
    </section>

    <section>
      <h2>8. Security</h2>
      <p>
        rakkio uses HTTPS, tenant-scoped database access, opaque hashed sessions, CSRF protection,
        encrypted Google credentials, private object storage, signed download links, rate limiting,
        and restricted production configuration. No service can guarantee absolute security. Please
        report suspected security problems privately using the contact address above rather than
        publishing personal data in a public issue.
      </p>
    </section>

    <section>
      <h2>9. Children</h2>
      <p>
        rakkio is not directed to children under 18 and does not knowingly collect their personal
        data. If you believe a child has provided personal data, contact the operator so it can be
        investigated and removed where required.
      </p>
    </section>

    <section>
      <h2>10. Changes to this policy</h2>
      <p>
        This policy may change when the service or legal requirements change. Material changes will
        be communicated in the application or by another appropriate method before new uses of
        personal data begin. The current revision date appears at the top of this page.
      </p>
    </section>
  </PublicPageShell>
</template>
