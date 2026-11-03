-- Preserve the notice actually accepted when an OAuth flow began. Legacy
-- attempts and old API instances during rollout retain task-only consent.
ALTER TABLE discord_oauth_attempts
    ADD COLUMN consent_version TEXT NOT NULL DEFAULT '2026-10-31';
