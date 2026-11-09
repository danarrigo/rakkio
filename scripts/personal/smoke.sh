#!/usr/bin/env bash
# DESTRUCTIVE ONLY TO THIS SCRIPT'S RANDOM, SYNTHETIC CI PROJECTS.
# Run on a disposable remote Docker runner, never an operator installation.
set -euo pipefail
[[ ${PROSEPECT_DISPOSABLE_CI:-} == yes ]] || { echo 'Requires PROSEPECT_DISPOSABLE_CI=yes on a disposable runner.' >&2; exit 1; }
cd "$(dirname "$0")/../.."
umask 077
work=$(mktemp -d)
project="prosepect-ci-$(openssl rand -hex 6)"
recovery="prosepect-rehearsal-$(openssl rand -hex 6)"
export PYTHONDONTWRITEBYTECODE=1
export PYTHONPATH="$PWD/scripts/personal"
python3 - "$work" <<'PY'
from pathlib import Path
import hashlib, secrets, sys
from configure import write_env
from test_personal import synthetic
p = Path(sys.argv[1])
write_env(p / 'installation.env', synthetic())
token, csrf = secrets.token_hex(32), secrets.token_hex(32)
other_token = secrets.token_hex(32)
(p / 'other.curl').write_text(f'header = "Cookie: prosepect_session={other_token}"\n')
(p / 'auth.curl').write_text(f'header = "Cookie: prosepect_session={token}"\nheader = "x-csrf-token: {csrf}"\n')
(p / 'fixture.sql').write_text(f"""
INSERT INTO users (id,email,display_name) VALUES
('00000000-0000-4000-8000-000000000001','owner@example.test','Synthetic recovery marker'),
('00000000-0000-4000-8000-000000000003','other@example.test','CI-only tenant isolation fixture');
INSERT INTO sessions (token_hash,user_id,csrf_token,expires_at) VALUES
(decode('{hashlib.sha256(token.encode()).hexdigest()}','hex'),
'00000000-0000-4000-8000-000000000001','{csrf}',NOW()+INTERVAL '1 hour'),
(decode('{hashlib.sha256(other_token.encode()).hexdigest()}','hex'),
'00000000-0000-4000-8000-000000000003','{csrf}',NOW()+INTERVAL '1 hour');
""")
(p / 'payload.bin').write_bytes(b'prosepect synthetic recovery bytes\x00\x01\xff\n')
PY
base=(docker compose --env-file "$work/installation.env" -p "$project" -f deploy/personal/compose.yaml)
stack=("${base[@]}" -f deploy/personal/tests/compose.yaml)
cleanup() {
  # No general-purpose cleanup helper: only randomly named test-owned projects.
  docker rm -f "${project}-http-client" >/dev/null 2>&1 || true
  "${stack[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  if [[ -f "$work/restored/compose.yaml" ]]; then
    docker compose --env-file "$work/restored/installation.env" -p "$recovery" -f "$work/restored/compose.yaml" down --volumes >/dev/null 2>&1 || true
  fi
  for name in files-data caddy-data caddy-config; do docker volume rm "${recovery}_${name}" >/dev/null 2>&1 || true; done
  rm -rf "$work"
}
trap cleanup EXIT
"${stack[@]}" config --quiet
"${stack[@]}" config --format json > "$work/config.json"
python3 - "$work/config.json" <<'PY'
import json, sys
c = json.load(open(sys.argv[1]))
assert set(c['services']) == {'postgres','api','worker','web'}
assert not any(v['target'] == '/data/files' for v in c['services']['web']['volumes'])
for name, service in c['services'].items():
    if name != 'web': assert not service.get('ports'), name
assert {int(p['published']) for p in c['services']['web']['ports']} == {80,443}
for name in ('api','worker'):
    e = c['services'][name]['environment']
    for key,value in {'APP_ENV':'production','ALLOW_INSECURE_DEV_AUTH':'false','INVITE_ONLY':'true','MAX_USER_ACCOUNTS':'1','FILE_STORAGE_BACKEND':'local','FILE_STORAGE_PATH':'/data/files'}.items():
        assert e[key] == value, (name,key)
    assert not any(key.startswith('S3_') for key in e)
    for key in ('GOOGLE_CLIENT_ID','TOKEN_ENCRYPTION_KEY'):
        assert e[key], (name,key)
assert c['networks']['default']['internal'] is True
PY
"${stack[@]}" build
"${stack[@]}" up -d --wait --wait-timeout 180 api
python3 scripts/personal/invite.py --env "$work/installation.env" --project "$project"
python3 scripts/personal/invite.py --env "$work/installation.env" --project "$project"
sql() { "${stack[@]}" exec -T postgres psql -X -U prosepect -d prosepect -v ON_ERROR_STOP=1 -Atq; }
[[ $(printf "SELECT COUNT(*) FROM account_invites WHERE email='owner@example.test';" | sql) == 1 ]]
# No owner is created by the invite helper. Synthetic session ONLY in this CI DB.
[[ $(printf 'SELECT COUNT(*) FROM users;' | sql) == 0 ]]
sql < "$work/fixture.sql"
"${stack[@]}" up -d --wait --wait-timeout 180 web
for attempt in {1..60}; do
  if docker cp "$("${stack[@]}" ps -q web):/data/caddy/pki/authorities/local/root.crt" "$work/root.crt" 2>/dev/null; then break; fi
  sleep 1
done
[[ -s "$work/root.crt" ]]
# Internal-only Docker networks need an in-network client; published host ports
# are not reachable on all Docker versions. Keep application egress blocked.
web_address=$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$("${stack[@]}" ps -q web)")
client_image=$(docker inspect --format '{{.Image}}' "$("${stack[@]}" ps -q api)")
# One persistent client also keeps the real source IP stable for rate-limit tests.
docker run -d --name "${project}-http-client" --network "${project}_default" \
  --user "$(id -u):$(id -g)" --volume "$work:$work" \
  --entrypoint sleep "$client_image" infinity >/dev/null
http_client=(docker exec "${project}-http-client" curl)
https=("${http_client[@]}" --silent --show-error --max-time 10 --noproxy '*' --cacert "$work/root.crt" --resolve "app.prosepect.test:443:$web_address")
status() {
  local expected=$1; shift
  local actual
  actual=$("${https[@]}" -D "$work/headers" -o "$work/body" -w '%{http_code}' "$@")
  [[ "$actual" == "$expected" ]] || { echo "Expected HTTP $expected; got $actual" >&2; exit 1; }
}
status 200 https://app.prosepect.test/ready
grep -iq '^Strict-Transport-Security: max-age=31536000' "$work/headers"
grep -iq '^X-Content-Type-Options: nosniff' "$work/headers"
status 200 https://app.prosepect.test/health
status 200 https://app.prosepect.test/api-doc/openapi.json
status 200 https://app.prosepect.test/
grep -q 'Plan what matters without scattering your attention.' "$work/body"
grep -q 'rel="canonical" href="https://rakkio.com/"' "$work/body"
for path in privacy terms delete-account; do
  status 200 "https://app.prosepect.test/$path"
  grep -q "rel=\"canonical\" href=\"https://rakkio.com/$path\"" "$work/body"
done
status 200 https://app.prosepect.test/robots.txt
grep -iq '^Content-Type: text/plain' "$work/headers"
grep -q 'Sitemap: https://rakkio.com/sitemap.xml' "$work/body"
status 200 https://app.prosepect.test/sitemap.xml
grep -q '<urlset' "$work/body"
for path in home home/; do
  status 308 "https://app.prosepect.test/$path"
  grep -iq '^Location: /' "$work/headers"
done
status 308 https://app.prosepect.test/delete-account.html
grep -iq '^Location: /delete-account' "$work/headers"
status 404 https://app.prosepect.test/blog.html
grep -q 'Page not found' "$work/body"
grep -iq '^X-Robots-Tag: noindex, nofollow' "$work/headers"
status 200 https://app.prosepect.test/settings
grep -iq '^X-Robots-Tag: noindex, nofollow' "$work/headers"
! grep -q 'rel="canonical"' "$work/body"
grep -iq "^Content-Security-Policy: .*script-src 'self'.*frame-ancestors 'none'" "$work/headers"
grep -iq '^X-Frame-Options: DENY' "$work/headers"
grep -iq '^X-Content-Type-Options: nosniff' "$work/headers"
grep -iq '^Referrer-Policy: strict-origin-when-cross-origin' "$work/headers"
[[ $("${http_client[@]}" --silent --max-time 10 --noproxy '*' --resolve "app.prosepect.test:80:$web_address" -o /dev/null -w '%{http_code}' http://app.prosepect.test/ready) == 308 ]]
status 401 https://app.prosepect.test/api/v1/session
status 401 https://app.prosepect.test/api/v1/operations
status 401 -X POST https://app.prosepect.test/api/v1/development/session
status 401 -H 'x-prosepect-user-id: 00000000-0000-4000-8000-000000000001' https://app.prosepect.test/api/v1/session
status 403 'https://app.prosepect.test/api/v1/auth/google/callback?error=access_denied'
status 403 -X POST https://app.prosepect.test/webhooks/google/calendar
# Invalid consent is rejected before OAuth discovery. Changing spoofed XFF must
# NOT evade the ten-attempt limiter: the eleventh request still returns 429.
for attempt in {1..11}; do
  expected=422; [[ $attempt == 11 ]] && expected=429
  status "$expected" -H "X-Forwarded-For: 203.0.113.$attempt" -H "X-Real-IP: 192.0.2.$attempt" \
    'https://app.prosepect.test/api/v1/auth/google/start?terms_version=invalid&privacy_version=invalid&age_confirmed=true'
done
status 201 --config "$work/auth.curl" -F "file=@$work/payload.bin" https://app.prosepect.test/api/v1/files
file_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$work/body")
status 200 --config "$work/auth.curl" "https://app.prosepect.test/api/v1/files/$file_id/download"
cmp "$work/payload.bin" "$work/body"
status 401 "https://app.prosepect.test/api/v1/files/$file_id/download"
status 200 --config "$work/other.curl" https://app.prosepect.test/api/v1/session
status 404 --config "$work/other.curl" "https://app.prosepect.test/api/v1/files/$file_id/download"
printf "SELECT object_key FROM files WHERE id='%s';" "$file_id" | sql > "$work/object-key"
# No public static route may serve private volume bytes, even by known object key.
status 404 "https://app.prosepect.test/data/files/$(cat "$work/object-key")"
! cmp -s "$work/payload.bin" "$work/body"
# Delete the direct DB-only second tenant fixture; normal registration still has cap=1.
printf "DELETE FROM users WHERE id='00000000-0000-4000-8000-000000000003';" | sql
file_uid=$("${stack[@]}" exec -T api id -u)
file_gid=$("${stack[@]}" exec -T api id -g)
[[ "$file_uid" != 0 ]]
[[ $("${stack[@]}" exec -T api stat -c '%u:%g:%a' /data/files) == "$file_uid:$file_gid:700" ]]
# Recreate the container, not just the process: bytes must live in the volume.
"${stack[@]}" up -d --force-recreate --no-deps --wait --wait-timeout 180 api
status 200 --config "$work/auth.curl" "https://app.prosepect.test/api/v1/files/$file_id/download"
cmp "$work/payload.bin" "$work/body"
# Prove a long-lived production worker persists a deterministic LOCAL failure.
# With API stopped there is no dispatcher race; missing calendar fails before Google.
"${stack[@]}" stop api
printf "INSERT INTO sync_jobs (id,user_id,kind,idempotency_key) VALUES ('00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001','calendar_watch','synthetic-worker');" | sql
"${stack[@]}" up -d --no-deps worker
for attempt in {1..60}; do
  [[ $(printf "SELECT status FROM sync_jobs WHERE idempotency_key='synthetic-worker';" | sql) == failed ]] && break
  sleep 1
done
[[ $(printf "SELECT COUNT(*) FROM sync_jobs WHERE idempotency_key='synthetic-worker' AND status='failed' AND attempt_count>=1 AND last_error LIKE '%%requires a calendar%%';" | sql) == 1 ]]
printf "UPDATE sync_jobs SET available_at=NOW()+INTERVAL '1 day' WHERE idempotency_key='synthetic-worker';" | sql
"${stack[@]}" restart worker
[[ $(printf "SELECT status FROM sync_jobs WHERE idempotency_key='synthetic-worker';" | sql) == failed ]]
"${stack[@]}" start api
"${stack[@]}" up -d --wait --wait-timeout 180
# Explicitly prove production rejects insecure auth configuration at startup.
if "${stack[@]}" run --rm --no-deps -e ALLOW_INSECURE_DEV_AUTH=true api > "$work/rejected.log" 2>&1; then
  echo 'Production incorrectly accepted insecure authentication' >&2; exit 1
fi
grep -q 'ALLOW_INSECURE_DEV_AUTH cannot be enabled in production' "$work/rejected.log"
python3 scripts/personal/snapshot.py backup "$work/snapshot" --env "$work/installation.env" --project "$project"
python3 scripts/personal/snapshot.py restore "$work/snapshot" "$work/restored" --project "$recovery"
restored=(docker compose --env-file "$work/restored/installation.env" -p "$recovery" -f "$work/restored/compose.yaml")
[[ $(printf "SELECT display_name FROM users WHERE id='00000000-0000-4000-8000-000000000001';" | "${restored[@]}" exec -T postgres psql -X -Atq -U prosepect -d prosepect) == 'Synthetic recovery marker' ]]
[[ $(printf "SELECT status FROM sync_jobs WHERE idempotency_key='synthetic-worker';" | "${restored[@]}" exec -T postgres psql -X -Atq -U prosepect -d prosepect) == failed ]]
# Read restored bytes as the original API UID/GID, no network or production Compose.
[[ $(docker network inspect "${recovery}_default" --format '{{.Internal}}') == true ]]
restore_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["images"]["postgres"]["restore"])' "$work/snapshot/manifest.json")
docker volume inspect "${recovery}_files-data" >/dev/null
[[ $(docker run --rm --network none --user "$file_uid:$file_gid" --entrypoint stat \
  --mount "type=volume,src=${recovery}_files-data,dst=/files,readonly" \
  "$restore_image" -c '%u:%g:%a' /files) == "$file_uid:$file_gid:700" ]]
docker run --rm -i --network none --user "$file_uid:$file_gid" --entrypoint /bin/sh \
  --mount "type=volume,src=${recovery}_files-data,dst=/files,readonly" "$restore_image" -ec \
  'read -r key; case "$key" in *[!0-9a-f-]*|"") exit 1;; esac; cat "/files/$key"' \
  < "$work/object-key" > "$work/restored.bin"
cmp "$work/payload.bin" "$work/restored.bin"
[[ $(docker network inspect "${recovery}_default" --format '{{.Internal}}') == true ]]
# Existing resource/config reuse must fail closed, without modifying the restore.
if python3 scripts/personal/snapshot.py restore "$work/snapshot" "$work/second-restore" --project "$recovery" 2>/dev/null; then
  echo 'Restore wrongly reused an existing project' >&2; exit 1
fi
echo 'PASS: production HTTPS routes/auth/storage, spoof-resistant XFF, owner invite, worker persistence, matched DB+file restore.'
echo 'Google sign-in, public DNS/ACME and physical phone access remain MANUAL live gates.'
