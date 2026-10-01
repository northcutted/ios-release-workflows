import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from simulator import bootstrap_failure, inspect_bundle, prepare, reset

ID = 'C71FB2C5-952C-4E6D-A2AD-1ADAE3B28FA1'
FIXTURES = Path(__file__).parent / 'fixtures'


class SimulatorTests(unittest.TestCase):
    def setUp(self):
        self.summary = json.loads((FIXTURES / 'simulator-bootstrap-summary.json').read_text())
        self.tests = json.loads((FIXTURES / 'simulator-bootstrap-tests.json').read_text())

    def test_captured_accessibility_bootstrap_crash_is_recoverable(self):
        self.assertTrue(bootstrap_failure(self.summary, self.tests))
        self.summary['testFailures'][0]['failureText'] = 'The test runner timed out while preparing to run tests.'
        self.tests['testNodes'][0]['children'][0]['children'][0]['children'][0]['children'][0]['name'] = self.summary['testFailures'][0]['failureText']
        self.assertTrue(bootstrap_failure(self.summary, self.tests))

    def test_assertions_launch_errors_and_executed_tests_never_recover(self):
        for text in ['XCTAssertTrue failed', 'Failed to launch application via Xcode',
                     'The test runner crashed while running tests', 'Unknown startup failure']:
            changed = copy.deepcopy(self.summary)
            changed['testFailures'][0]['failureText'] = text
            self.assertFalse(bootstrap_failure(changed, self.tests))
        for key in ['passedTests', 'skippedTests', 'expectedFailures']:
            changed = dict(self.summary, **{key: 1})
            self.assertFalse(bootstrap_failure(changed, self.tests))
        cases = copy.deepcopy(self.tests)
        runner = cases['testNodes'][0]['children'][0]['children'][0]['children'][0]
        runner['nodeIdentifier'] = 'PicStripUITests/testAllScreenshots()'
        self.assertFalse(bootstrap_failure(self.summary, cases))

    def test_missing_or_inconsistent_evidence_never_recovers(self):
        for changed in [{}, dict(self.summary, totalTestCount=2), dict(self.summary, failedTests=2), dict(self.summary, passedTests=False),
                        dict(self.summary, testFailures=[]), dict(self.summary, result='Passed')]:
            self.assertFalse(bootstrap_failure(changed, self.tests))
        self.assertFalse(bootstrap_failure(self.summary, {'testNodes': []}))
        self.assertFalse(bootstrap_failure(self.summary, {'testNodes': [None]}))
        inconsistent = copy.deepcopy(self.tests)
        inconsistent['testNodes'][0]['children'][0]['children'][0]['children'][0]['children'][0]['name'] = 'XCTAssertTrue failed'
        self.assertFalse(bootstrap_failure(self.summary, inconsistent))
        with tempfile.TemporaryDirectory() as tmp, patch('simulator.command', side_effect=FileNotFoundError('missing bundle')):
            result = inspect_bundle('missing.xcresult', Path(tmp) / 'assessment.json')
            self.assertFalse(result['recoverable'])
            self.assertTrue((Path(tmp) / 'assessment.json').exists())

    def test_bounded_readiness_records_success_and_timeout(self):
        selected = {'udid': ID, 'state': 'Shutdown', 'isAvailable': True}
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'readiness.json'
            with patch('simulator.device', side_effect=[selected, dict(selected, state='Booted')]), patch('simulator.command', return_value='booted') as run:
                self.assertEqual(prepare(ID, output)['status'], 'ready')
                run.assert_called_once_with(['xcrun', 'simctl', 'bootstatus', ID, '-b'], timeout=180)
            with patch('simulator.device', return_value=selected), patch('simulator.command', side_effect=subprocess.TimeoutExpired('bootstatus', 180)):
                with self.assertRaises(subprocess.TimeoutExpired):
                    prepare(ID, output)
            self.assertEqual(json.loads(output.read_text())['status'], 'failed')

    def test_reset_only_erases_the_resolved_device_on_a_disposable_host(self):
        env = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted', 'SIMULATOR_UDIDS': json.dumps({'device': ID})}
        with patch.dict(os.environ, env), patch('simulator.device', return_value={'state': 'Booted'}), patch('simulator.command') as run:
            reset(ID)
        self.assertEqual([call.args[0] for call in run.call_args_list], [
            ['xcrun', 'simctl', 'shutdown', ID], ['xcrun', 'simctl', 'erase', ID]])
        for changed in [{'GITHUB_ACTIONS': 'false'}, {'RUNNER_ENVIRONMENT': 'self-hosted'}, {'SIMULATOR_UDIDS': '{}'}]:
            with patch.dict(os.environ, dict(env, **changed)), patch('simulator.command') as run:
                with self.assertRaises(ValueError):
                    reset(ID)
                run.assert_not_called()

    def test_uuid_and_boot_state_are_validated_before_mutation(self):
        with patch('simulator.command') as run, self.assertRaises(ValueError):
            prepare('booted', '/tmp/unused.json')
        run.assert_not_called()
        env = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted', 'SIMULATOR_UDIDS': json.dumps({'device': ID})}
        with patch.dict(os.environ, env), patch('simulator.device', return_value={'state': 'Creating'}), patch('simulator.command') as run:
            with self.assertRaises(ValueError):
                reset(ID)
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
