"""Exercise real HTTP/curl boundaries without production or provider requests."""
import contextlib
import http.server
import io
from pathlib import Path
import re
import subprocess
import threading
import time
import unittest
from unittest.mock import patch

import worker_request


class WorkerRequestTests(unittest.TestCase):
    def test_hosted_recovery_is_hourly_and_still_supports_manual_dispatch(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/worker.yml").read_text()
        self.assertEqual(re.findall(r'cron:\s*"([^"]+)"', workflow), ["7 * * * *"])
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("worker_request.py ready", workflow)

    def test_render_uses_database_free_liveness_without_changing_the_service(self):
        blueprint = (Path(__file__).resolve().parents[1] / "render.yaml").read_text()
        self.assertRegex(blueprint, r"(?m)^\s+healthCheckPath: /health$")
        self.assertRegex(blueprint, r"(?m)^\s+name: rakkio-api$")
        self.assertRegex(blueprint, r"(?m)^\s+plan: free$")

    @contextlib.contextmanager
    def endpoint(self, responses):
        calls = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.handle_request()

            def do_POST(self):
                self.handle_request()

            def handle_request(self):
                calls.append((self.command, self.headers.get("Authorization")))
                status, headers, body = responses[min(len(calls) - 1, len(responses) - 1)]
                self.send_response(status)
                for key, value in headers.items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(body.encode())

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/", calls
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def invoke(self, url, **kwargs):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            worker_request.request(url, delays=(0, 0, 0), budget=5, timeout=2, **kwargs)
        return output.getvalue()

    def test_worker_recovers_after_three_throttled_attempts(self):
        responses = [(429, {"X-Request-ID": "throttle-id"}, "Too Many Requests")] * 3
        responses.append((200, {}, '{"enqueued":0,"processed":false}'))
        with self.endpoint(responses) as (url, calls):
            output = self.invoke(url, token="test-only-token")
        self.assertEqual(len(calls), 4)
        self.assertTrue(all(call == ("POST", "Bearer test-only-token") for call in calls))
        self.assertIn('"x-request-id": "throttle-id"', output)
        self.assertNotIn("test-only-token", output)

    def test_readiness_recovers_from_503(self):
        with self.endpoint([(503, {}, "unavailable"), (200, {}, '{"status":"ready"}')]) as (url, calls):
            self.invoke(url)
        self.assertEqual([call[0] for call in calls], ["GET", "GET"])

    def test_persistent_throttling_fails_with_bounded_attempts(self):
        with self.endpoint([(429, {}, "Too Many Requests")]) as (url, calls):
            with self.assertRaises(RuntimeError):
                self.invoke(url, token="test-only")
        self.assertEqual(len(calls), 4)

    def test_does_not_retry_worker_5xx_or_auth_errors_or_follow_redirects(self):
        for status in [401, 403, 500, 502, 503, 504, 307]:
            with self.subTest(status=status):
                with self.endpoint([(status, {"Location": "/redirect"}, "")]) as (url, calls):
                    with self.assertRaises(RuntimeError):
                        self.invoke(url, token="test-only")
                self.assertEqual(len(calls), 1)

    def test_retry_after_is_honored_without_exceeding_budget(self):
        original_run = subprocess.run

        def run_with_library_wait(*args, **kwargs):
            # subprocess can sleep internally while waiting for a child. A worker
            # clock mock must not intercept this shared standard-library clock.
            time.sleep(0.001)
            return original_run(*args, **kwargs)

        with self.endpoint([(429, {"Retry-After": "2"}, ""), (200, {}, '{"status":"ready"}')]) as (url, calls):
            with patch.object(worker_request, "time") as clock, patch.object(subprocess, "run", side_effect=run_with_library_wait):
                clock.monotonic.side_effect = time.monotonic
                self.invoke(url)
                clock.sleep.assert_called_once_with(2)
        self.assertEqual(len(calls), 2)
        with self.endpoint([(429, {"Retry-After": "999"}, "")]) as (url, calls):
            with self.assertRaisesRegex(RuntimeError, "budget exhausted"):
                self.invoke(url)
        self.assertEqual(len(calls), 1)

    def test_diagnostics_omit_sensitive_headers_and_response_bodies(self):
        headers = {"Set-Cookie": "secret-cookie", "Authorization": "secret-auth", "Retry-After": "0"}
        with self.endpoint([(429, headers, "secret-body"), (200, {}, '{"status":"ready"}')]) as (url, _):
            output = self.invoke(url)
        self.assertNotIn("secret", output)
        self.assertIn('"retry-after": "0"', output)

    def test_invalid_success_body_fails_without_retry(self):
        for body in ['{"status":"not-ready"}', "not-json", '[]']:
            with self.endpoint([(200, {}, body)]) as (url, calls):
                with self.assertRaises((RuntimeError, ValueError)):
                    self.invoke(url)
            self.assertEqual(len(calls), 1)

    def test_invalid_token_never_sends_a_request(self):
        for token in ["", "bad\nheader"]:
            with self.assertRaises(RuntimeError):
                worker_request.request("http://127.0.0.1:1", token=token)

    def test_worker_transport_timeout_is_not_replayed(self):
        result = subprocess.CompletedProcess([], 28, stdout="000", stderr="timeout")
        with patch.object(worker_request.subprocess, "run", return_value=result) as run:
            with self.assertRaises(RuntimeError):
                self.invoke("http://127.0.0.1:1", token="test-only")
            self.assertEqual(run.call_count, 1)

    def test_retry_after_date_and_invalid_values(self):
        self.assertEqual(worker_request.retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), 0)
        self.assertEqual(worker_request.retry_after("invalid"), 0)
        self.assertEqual(worker_request.retry_after("-1"), 0)


if __name__ == "__main__":
    unittest.main()
