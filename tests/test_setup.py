import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from ios_release.cli import apple_environment, ruby_command


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
