import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from ios_release.cli import apple_environment, ruby_command, check


class SetupTests(unittest.TestCase):
    def test_pinned_ruby_wins_over_shell_path_without_global_activation(self):
        with tempfile.TemporaryDirectory() as directory:
            platform = Path(directory)
            (platform / ".ruby-version").write_text("3.4.10\n")
            with patch("ios_release.cli.shutil.which", return_value="/tool"), patch("ios_release.cli.subprocess.check_output", side_effect=["4.0.6", "/managed/ruby/3.4.10"]):
                self.assertEqual(ruby_command(platform), ["/managed/ruby/3.4.10/bin/ruby", "-S"])
            with patch("ios_release.cli.ruby_command", return_value=["/managed/ruby/3.4.10/bin/ruby", "-S"]), patch.dict(os.environ, {"PATH": "/other/ruby/bin"}):
                env = apple_environment(platform, platform)
                self.assertEqual(env["PATH"], "/managed/ruby/3.4.10/bin:/other/ruby/bin")
                self.assertEqual(os.environ["PATH"], "/other/ruby/bin")
                self.assertEqual(env["BUNDLE_FROZEN"], "true")

    def test_platform_fixture_environment_survives_a_separate_action_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            app, tools = Path(directory) / "workspace", Path(directory) / "action"
            (app / "tests").mkdir(parents=True)
            with patch("ios_release.docs.profile", return_value={"mode": "platform"}), patch("ios_release.yamlio.workflows", return_value={}), patch("ios_release.policy.validate", return_value=[]), patch("ios_release.docs.run", return_value=[]), patch("ios_release.cli.subprocess.run") as run:
                check([], tools, app)
            environment = run.call_args.kwargs["env"]
            self.assertEqual(environment["IOS_RELEASE_CONFIG"], str(app / "examples/picstrip.json"))
            self.assertEqual(environment["IOS_RELEASE_REVISION"], "f" * 40)
            self.assertNotIn("GITHUB_REPOSITORY", environment)
