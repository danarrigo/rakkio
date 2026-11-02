"""Safe local tests: temporary synthetic files only; never invokes Docker."""
import base64
import io
import json
import os
from pathlib import Path
import secrets
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import configure
import snapshot


def synthetic():
    return {
        "APP_HOST": "app.prosepect.test",
        "OWNER_EMAIL": "owner@example.test",
        "GOOGLE_CLIENT_ID": "synthetic.apps.googleusercontent.com",
        "GOOGLE_CLIENT_SECRET": "synthetic-never-google-secret",
        "POSTGRES_PASSWORD": secrets.token_hex(32),
        "TOKEN_ENCRYPTION_KEY": base64.b64encode(secrets.token_bytes(32)).decode(), "ADMIN_USER_IDS": "",
    }


class PersonalSafetyTests(unittest.TestCase):
    def test_exclusive_private_env_and_no_rotation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env.personal"
            config = synthetic()
            configure.write_env(path, config)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(configure.load_env(path), config)
            with self.assertRaises(FileExistsError):
                configure.write_env(path, synthetic())
            self.assertEqual(configure.load_env(path), config)
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                configure.load_env(path)

    def test_env_rejects_injection_bad_hosts_and_implicit_admin(self):
        for key, bad in [("APP_HOST", "https://app.example.com"), ("APP_HOST", "127.0.0.1"),
                         ("APP_HOST", "$(touch /tmp/not-run)"), ("APP_HOST", "a.example.com\nX=y"),
                         ("GOOGLE_CLIENT_SECRET", "abc$unsafe"), ("OWNER_EMAIL", "a'; DROP TABLE users;--"),
                         ("ADMIN_USER_IDS", "owner@example.test"), ("TOKEN_ENCRYPTION_KEY", "YWJj")]:
            config = synthetic()
            config[key] = bad
            with self.subTest(key=key), self.assertRaises(ValueError):
                configure.validate(config)
        config = synthetic()
        config["UNEXPECTED_SETTING"] = "rejected"
        with self.assertRaises(ValueError):
            configure.validate(config)

    def test_env_symlinks_and_duplicate_keys_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "private"
            configure.write_env(target, synthetic())
            link = Path(directory) / "link"
            link.symlink_to(target)
            with self.assertRaises(ValueError):
                configure.load_env(link)
            with target.open("a") as output:
                output.write("APP_HOST=other.example.com\n")
            with self.assertRaises(ValueError):
                configure.load_env(target)

    def test_shell_exports_cannot_override_file(self):
        with patch.dict(os.environ, {"APP_HOST": "evil.example.com", "COMPOSE_FILE": "evil.yaml"}):
            self.assertNotIn("APP_HOST", configure.docker_env())
            self.assertNotIn("COMPOSE_FILE", configure.docker_env())

    def test_optional_discord_config_is_complete_private_and_same_origin(self):
        config = synthetic()
        configure.validate(config)
        config.update({
            "DISCORD_APPLICATION_ID": str(123456789012345678),  # Synthetic public identifier, not a credential.
            "DISCORD_CLIENT_SECRET": "synthetic-discord-secret",
            "DISCORD_PUBLIC_KEY": "ab" * 32,
            "DISCORD_REDIRECT_URI": "https://app.prosepect.test/api/v1/auth/discord/callback",
        })
        configure.validate(config)
        for key, value in [("DISCORD_CLIENT_SECRET", ""), ("DISCORD_PUBLIC_KEY", "bad"),
                           ("DISCORD_REDIRECT_URI", "https://other.example.com/api/v1/auth/discord/callback")]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                configure.validate({**config, key: value})
        with patch.dict(os.environ, {"DISCORD_APPLICATION_ID": "unexpected", "DISCORD_CLIENT_SECRET": "unexpected"}):
            self.assertNotIn("DISCORD_APPLICATION_ID", configure.docker_env())
            self.assertNotIn("DISCORD_CLIENT_SECRET", configure.docker_env())

    def test_archive_paths_links_and_devices_rejected(self):
        for name, kind in [("../escape", tarfile.REGTYPE), ("/absolute", tarfile.REGTYPE),
                           ("link", tarfile.SYMTYPE), ("hard", tarfile.LNKTYPE), ("device", tarfile.CHRTYPE)]:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "bad.tar"
                with tarfile.open(path, "w") as archive:
                    entry = tarfile.TarInfo(name)
                    entry.type = kind
                    entry.linkname = "../../escape"
                    archive.addfile(entry)
                with self.subTest(name=name), self.assertRaises(ValueError):
                    snapshot.check_tar(path)

    def test_stopped_container_collision_fails_before_restore_resources(self):
        project = "prosepect-rehearsal-stopped"
        for labeled in (True, False):
            with self.subTest(labeled=labeled), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "restored"
                args = SimpleNamespace(directory=Path(directory) / "snapshot", project=project, output=output)

                def listed(command):
                    if command[:3] != ["docker", "container", "ls"] or "--all" not in command:
                        return ""
                    if labeled and "--filter" in command:
                        return "stopped-container-id"
                    if not labeled and "--format" in command:
                        return project + "-postgres-1"
                    return ""

                with patch.object(snapshot, "verify", return_value={}), \
                     patch.object(snapshot, "text", side_effect=listed), \
                     patch.object(snapshot, "run") as run:
                    with self.assertRaisesRegex(ValueError, "already exists|name collision"):
                        snapshot.restore(args)
                    self.assertFalse(output.exists())
                    run.assert_not_called()

    def test_incomplete_and_corrupt_snapshots_fail_before_docker(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(snapshot, "run") as run:
            path = Path(directory)
            with self.assertRaises(ValueError):
                snapshot.verify(path)
            configure.write_env(path / "installation.env", synthetic())
            (path / "database.dump").write_bytes(b"synthetic")
            for volume in snapshot.VOLUMES:
                with tarfile.open(path / (volume + ".tar"), "w") as archive:
                    entry = tarfile.TarInfo("./object")
                    entry.size = 3
                    archive.addfile(entry, io.BytesIO(b"abc"))
            manifest = {"format": 2, "sha256": {name: snapshot.digest(path / name) for name in snapshot.ARTIFACTS},
                        "images": {"postgres": {"restore": "postgres@sha256:" + "a" * 64}}}
            (path / "manifest.json").write_text(json.dumps(manifest))
            snapshot.verify(path)
            # Old object-storage snapshots must never be mistaken for local files.
            manifest["format"] = 1
            (path / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                snapshot.verify(path)
            manifest["format"] = 2
            (path / "manifest.json").write_text(json.dumps(manifest))
            (path / "database.dump").write_bytes(b"corrupt")
            with self.assertRaises(ValueError):
                snapshot.verify(path)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
