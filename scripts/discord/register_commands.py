#!/usr/bin/env python3
"""Register the three commands on an operator-owned, dedicated Discord app."""
import argparse
import base64
import getpass
import json
import re
import sys
import urllib.parse
import urllib.request

API = "https://discord.com/api/v10"
USER_AGENT = "DiscordBot (https://github.com/danarrigo/rakkio, 0.1.0)"
COMMANDS = [
    {
        "name": "add", "description": "Add a task to your rakkio Inbox", "type": 1,
        "integration_types": [1], "contexts": [0, 1, 2],
        "options": [
            {"name": "title", "description": "Literal task title (capture syntax is not parsed)",
             "type": 3, "required": True, "min_length": 1, "max_length": 240},
            {"name": "due", "description": "Deadline: today, tomorrow, or YYYY-MM-DD in your linked timezone",
             "type": 3, "required": False, "min_length": 1, "max_length": 10},
        ],
    },
    {"name": "today", "description": "Privately show today's tasks and selected calendar events",
     "type": 1, "integration_types": [1], "contexts": [0, 1, 2]},
    {"name": "done", "description": "Privately choose a task to complete",
     "type": 1, "integration_types": [1], "contexts": [0, 1, 2]},
]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


def exchange(application_id, client_secret, opener):
    body = urllib.parse.urlencode({"grant_type": "client_credentials", "scope": "applications.commands.update"}).encode()
    credentials = base64.b64encode(f"{application_id}:{client_secret}".encode()).decode()
    request = urllib.request.Request(
        API + "/oauth2/token", data=body, method="POST",
        headers={"Authorization": "Basic " + credentials, "Content-Type": "application/x-www-form-urlencoded", "User-Agent": USER_AGENT},
    )
    with opener.open(request, timeout=20) as response:
        data = response.read(65537)
    if len(data) > 65536:
        raise ValueError("oversized response")
    token = json.loads(data)
    if (not isinstance(token, dict) or not isinstance(token.get("access_token"), str) or not token["access_token"]
            or len(token["access_token"]) > 4096
            or str(token.get("token_type", "")).lower() != "bearer"
            or "applications.commands.update" not in str(token.get("scope", "")).split()):
        raise ValueError("unexpected authorization response")
    return token["access_token"]


def register(application_id, client_secret, opener):
    if not re.fullmatch(r"[1-9][0-9]{0,19}", application_id):
        raise ValueError("invalid application ID")
    token = exchange(application_id, client_secret, opener)
    request = urllib.request.Request(
        f"{API}/applications/{application_id}/commands", data=json.dumps(COMMANDS).encode(), method="PUT",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json", "User-Agent": USER_AGENT},
    )
    with opener.open(request, timeout=20) as response:
        if response.status != 200:
            raise ValueError("registration not confirmed")
        # Do not print response bodies, access tokens, or provider diagnostics.
        response.read(65537)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application-id", required=True, help="public ID from your Discord Developer Portal")
    args = parser.parse_args()
    if not re.fullmatch(r"[1-9][0-9]{0,19}", args.application_id):
        raise ValueError("invalid application ID")
    if not sys.stdin.isatty():
        raise ValueError("run interactively; do not pipe credentials")
    print(f"Target Discord application: {args.application_id}")
    print("This REPLACES its complete global command list with /add, /today, /done.")
    print("Use a dedicated application for this rakkio installation, not another existing bot.")
    if input(f"Type REGISTER {args.application_id} to continue: ").strip() != f"REGISTER {args.application_id}":
        print("Cancelled. No requests sent.")
        return
    secret = getpass.getpass("Discord OAuth client secret (hidden; NOT the bot token): ")
    if not secret.strip():
        raise ValueError("missing client secret")
    register(args.application_id, secret, urllib.request.build_opener(NoRedirect()))
    print("Three global commands registered. No credentials saved or printed. Command availability may take time to propagate.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError):
        print("Registration not confirmed. Check the application and configuration before rerunning. No credentials printed.", file=sys.stderr)
        sys.exit(1)
