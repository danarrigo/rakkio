#!/usr/bin/env python3
"""Create (never replace) a personal installation's private Compose env file."""
import argparse
import base64
import getpass
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ENV = ROOT / ".env.personal"
COMPOSE = ROOT / "deploy/personal/compose.yaml"
PROJECT = "prosepect-personal"
REQUIRED_KEYS = {"APP_HOST", "OWNER_EMAIL", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET",
                 "POSTGRES_PASSWORD", "TOKEN_ENCRYPTION_KEY", "ADMIN_USER_IDS"}
DISCORD_KEYS = {"DISCORD_APPLICATION_ID", "DISCORD_CLIENT_SECRET", "DISCORD_PUBLIC_KEY", "DISCORD_REDIRECT_URI"}
KEYS = REQUIRED_KEYS | DISCORD_KEYS


def validate(values):
    if not REQUIRED_KEYS.issubset(values) or not set(values).issubset(KEYS):
        raise ValueError("configuration keys missing or unexpected")
    for name, value in values.items():
        if not re.fullmatch(r"[a-zA-Z0-9_@.+/=,:\-]*", value):
            raise ValueError(f"unsupported characters in {name}")
    for name in ("APP_HOST",):
        host = values[name]
        if len(host) > 253 or "." not in host or not all(
            re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label)
            for label in host.split(".")
        ) or re.fullmatch(r"[0-9.]+", host):
            raise ValueError(f"{name} must be a lowercase DNS hostname, without scheme/port/path")
    if not re.fullmatch(r"[a-z0-9._+\-]+@[a-z0-9.\-]+\.[a-z]{2,}", values["OWNER_EMAIL"]):
        raise ValueError("OWNER_EMAIL must be one lowercase Google account email")
    if not re.fullmatch(r"[a-zA-Z0-9_-]+\.apps\.googleusercontent\.com", values["GOOGLE_CLIENT_ID"]):
        raise ValueError("expected a Google web OAuth client ID")
    if not re.fullmatch(r"[a-zA-Z0-9_-]{16,}", values["GOOGLE_CLIENT_SECRET"]):
        raise ValueError("invalid Google client secret format")
    if not re.fullmatch(r"[0-9a-f]{64}", values["POSTGRES_PASSWORD"]):
        raise ValueError("POSTGRES_PASSWORD must retain its generated 32-byte hex value")
    if len(base64.b64decode(values["TOKEN_ENCRYPTION_KEY"], validate=True)) != 32:
        raise ValueError("encryption key must decode to 32 bytes")
    if values["ADMIN_USER_IDS"]:
        uuid.UUID(values["ADMIN_USER_IDS"])
    if any(values.get(key) for key in DISCORD_KEYS):
        if not all(values.get(key) for key in DISCORD_KEYS):
            raise ValueError("configure all four Discord values together")
        if not re.fullmatch(r"[1-9][0-9]{0,19}", values["DISCORD_APPLICATION_ID"]):
            raise ValueError("invalid Discord application ID")
        if not re.fullmatch(r"[a-fA-F0-9]{64}", values["DISCORD_PUBLIC_KEY"]):
            raise ValueError("invalid Discord verification key")
        if len(values["DISCORD_CLIENT_SECRET"]) < 16:
            raise ValueError("invalid Discord client secret")
        if values["DISCORD_REDIRECT_URI"] != f"https://{values['APP_HOST']}/api/v1/auth/discord/callback":
            raise ValueError("Discord callback must use this installation's HTTPS hostname")


def load_env(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600 or info.st_uid != os.getuid():
        raise ValueError("env must be an owned regular file with mode 0600 (not a symlink)")
    values = {}
    for line in path.read_text().splitlines():
        key, sep, value = line.partition("=")
        if not sep or key in values:
            raise ValueError("invalid or duplicate env assignment")
        values[key] = value
    validate(values)
    return values


def write_env(path, values):
    validate(values)
    # O_EXCL also rejects dangling symlinks; interruption never replaces old keys.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as output:
            output.write("".join(f"{key}={value}\n" for key, value in values.items()))
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink()
        raise


def docker_env():
    # Shell exports must not silently override this installation's env file.
    return {key: value for key, value in os.environ.items()
            if key not in KEYS and not key.startswith("COMPOSE_")}


def compose(env, project=PROJECT, file=COMPOSE):
    if not re.fullmatch(r"[a-z][a-z0-9-]{2,50}", project):
        raise ValueError("invalid project name")
    load_env(env)
    return ["docker", "compose", "--env-file", str(env.resolve()),
            "-p", project, "-f", str(file)]


def preflight():
    if not shutil.which("docker"):
        raise ValueError("install Docker Engine and the Compose v2 plugin first")
    subprocess.run(["docker", "compose", "version"], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["docker", "info"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", type=Path, default=DEFAULT_ENV)
    parser.add_argument("--check", action="store_true", help="validate without changing secrets")
    args = parser.parse_args()
    if args.check:
        load_env(args.env)
        preflight()
        subprocess.run(compose(args.env) + ["config", "--quiet"], env=docker_env(), check=True)
        print("Configuration valid. DNS, firewall, certificates and Google consent still need live checks.")
        return
    if args.env.exists() or args.env.is_symlink():
        raise ValueError("env already exists; refusing to overwrite or rotate any key (use --check)")
    preflight()
    if not sys.stdin.isatty():
        raise ValueError("configuration requires a terminal; do not pipe secrets")
    print("Requires one DNS hostname pointing to this server, public TCP 80/443, and your own Google web OAuth client.")
    values = {
        "APP_HOST": input("App hostname: ").strip().lower(),
        "OWNER_EMAIL": input("Only invited Google email: ").strip().lower(),
        "GOOGLE_CLIENT_ID": input("Google web client ID: ").strip(),
        "GOOGLE_CLIENT_SECRET": getpass.getpass("Google client secret (hidden): ").strip(),
        "POSTGRES_PASSWORD": secrets.token_hex(32),
        "TOKEN_ENCRYPTION_KEY": base64.b64encode(secrets.token_bytes(32)).decode(),
        "ADMIN_USER_IDS": "",
    }
    write_env(args.env, values)
    print("Private env created (0600). No secrets printed. Next: build, start API privately, bootstrap invite; see guide.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        # Avoid exception strings from tools or malformed values exposing secrets.
        print(f"Configuration failed ({type(error).__name__}); existing installation was not rekeyed.", file=sys.stderr)
        sys.exit(1)
