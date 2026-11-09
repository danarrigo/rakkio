# Security policy

## Supported versions

Security fixes are applied to the latest release on `main`. rakkio has not yet committed to support windows for older releases.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Contact the repository owner privately with:

- the affected version or commit;
- reproduction steps;
- expected impact;
- any suggested mitigation.

Do not include real OAuth tokens, session cookies, personal content, or production database records. Reports will be acknowledged as soon as practical, validated privately, and disclosed after a fix or mitigation is available.

## Dependency security

There are no accepted npm or Rust advisory exceptions. All dependency advisories and Rust audit warnings are release-blocking.

Google sign-in uses `oauth2` for authorization-code exchange and PKCE, and `jsonwebtoken` with its AWS-LC backend for RS256 verification. The previous `openidconnect` dependency pulled in affected `crypto-bigint` and `rsa` versions. Removing that dependency removes both RUSTSEC-2026-0387 and the former RUSTSEC-2023-0071 exception, rather than suppressing either warning. See `docs/GOOGLE-SIGN-IN-VERIFICATION.md` for validation boundaries and tests.

SQLx is temporarily pinned to upstream commit `10462fa4414c6c7f1ed1d39bb31678e3b726878c` because its published 0.9.0 SQLite dependency cannot select the fix for RUSTSEC-2026-0459. That exact upstream change permits patched `libsqlite3-sys`; the remote-generated lock selects 0.39.1. The application uses PostgreSQL, not untrusted local SQLite/WAL files, but the lockfile advisory gate remains enforced without exceptions. Replace the Git pin with a patched published SQLx release when available. All SQLx packages use the same pinned source, and database/migration behavior must pass the full remote regression suite before release.

## Automated security gates

`.github/workflows/security.yml` audits both committed lockfiles on pushes to `main`, pull requests, manual runs, and a daily best-effort schedule so newly published advisories are checked without code changes. No production credentials or issue creation permissions are used.

- Rust runs only on GitHub-hosted CI using `cargo-audit`. An unfiltered JSON artifact records every advisory. The **mandatory subsequent gate** denies every advisory and warning (including unmaintained/unsound/yanked findings), without ignores. `rust-dependency-audit` retains both the full report and the acceptance-gate log for 14 days. A diagnostic step marked allowed-to-fail does not waive the mandatory gate.
- npm audits the complete lockfile, including development dependencies, at `--audit-level=low`. There are no npm exceptions. `npm-dependency-audit` retains the JSON report for 14 days, including failures.
- Gitleaks scans full Git history using a version/checksum-pinned binary and fully redacted output. `.gitleaksignore` contains only the exact historical fingerprints of the documented disposable OIDC signing fixture and synthetic webhook token. No whole path, rule or commit is excluded; newly committed secrets still fail. These fixture fingerprints do not waive any dependency gate. Redacted `secret-scan` evidence is retained for 14 days.

Require all three security jobs in repository merge rules and enable failure notifications. Workflow definitions alone do not configure branch protection or verify notification delivery. Network/database/tooling failures must be investigated, not treated as a clean security result. Schedules are not guaranteed to run on time; run the workflow manually against the release commit before publication.
