import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import developer


class DeveloperTests(unittest.TestCase):
    def test_screenshot_adapter_passes_its_own_python_and_packaged_helper(self):
        with tempfile.TemporaryDirectory() as app, patch.dict(os.environ, {'IOS_APP_ROOT': app}), patch.object(sys, 'argv', ['developer.py', 'screenshots-capture', '--devices', 'selected']), patch('developer.subprocess.call', return_value=0) as run:
            self.assertEqual(developer.main(), 0)
        environment = run.call_args.kwargs['env']
        self.assertEqual(environment['IOS_RELEASE_PYTHON'], sys.executable)
        self.assertEqual(environment['IOS_RELEASE_SIMULATOR_TOOL'], str(developer.ROOT / 'scripts/ci/simulator.py'))
        self.assertEqual(environment['IOS_RELEASE_SIMULATOR_HELPER'], str(developer.ROOT / 'fastlane/lib/simulator_recovery.rb'))
        self.assertTrue(Path(environment['IOS_RELEASE_SIMULATOR_HELPER']).is_file())
        self.assertIn('devices:selected', run.call_args.args[0])


if __name__ == '__main__':
    unittest.main()
