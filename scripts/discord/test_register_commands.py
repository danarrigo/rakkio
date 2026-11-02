"""Offline registration safety tests. No Discord requests or real credentials."""
import io
import json
import unittest
from unittest.mock import Mock, patch

import register_commands as registration


class Response(io.BytesIO):
    status = 200


class RegistrationTests(unittest.TestCase):
    def test_only_personal_commands_are_registered_with_narrow_client_credentials(self):
        opener = Mock()
        opener.open.side_effect = [
            Response(json.dumps({"access_token": "synthetic-access", "token_type": "Bearer",
                                 "scope": "applications.commands.update"}).encode()),
            Response(b"[]"),
        ]
        registration.register("123456789012345678", "synthetic-secret", opener)
        token_request = opener.open.call_args_list[0].args[0]
        self.assertEqual(token_request.full_url, "https://discord.com/api/v10/oauth2/token")
        self.assertEqual(token_request.get_header("User-agent"), registration.USER_AGENT)
        self.assertIn(b"scope=applications.commands.update", token_request.data)
        command_request = opener.open.call_args_list[1].args[0]
        self.assertEqual(command_request.method, "PUT")
        self.assertEqual(command_request.get_header("User-agent"), registration.USER_AGENT)
        self.assertEqual(command_request.full_url, "https://discord.com/api/v10/applications/123456789012345678/commands")
        commands = json.loads(command_request.data)
        self.assertEqual([command["name"] for command in commands], ["add", "today", "done"])
        self.assertTrue(all(command["integration_types"] == [1] for command in commands))
        self.assertNotIn("bot", str(commands))

    def test_rejects_injected_application_ids_before_any_request(self):
        opener = Mock()
        for app in ["../other", "https://other.test", "1?token=secret", "-1", ""]:
            with self.subTest(app=app), self.assertRaises(ValueError):
                registration.register(app, "synthetic-secret", opener)
        opener.open.assert_not_called()

    def test_never_follows_redirects_with_credentials(self):
        self.assertIsNone(registration.NoRedirect().redirect_request(None, None, 302, "redirect", {}, "https://other.test"))

    def test_cancelled_setup_sends_no_requests(self):
        with patch("sys.argv", ["register_commands.py", "--application-id", "123"]), \
             patch("sys.stdin.isatty", return_value=True), patch("builtins.input", return_value="cancel"), \
             patch.object(registration.getpass, "getpass") as secret, patch.object(registration, "register") as register, \
             patch("sys.stdout", new_callable=io.StringIO):
            registration.main()
        secret.assert_not_called()
        register.assert_not_called()

    def test_bad_or_oversized_token_response_never_registers_commands(self):
        for response in [b"{}", b"[]", b" " * 65537]:
            opener = Mock()
            opener.open.return_value = Response(response)
            with self.subTest(response_length=len(response)), self.assertRaises(ValueError):
                registration.register("123", "synthetic-secret", opener)
            self.assertEqual(opener.open.call_count, 1)


if __name__ == "__main__":
    unittest.main()
