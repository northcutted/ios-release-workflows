import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts/ci'))
os.environ.setdefault('IOS_RELEASE_CONFIG', str(ROOT / 'examples/minimal.json'))
from native_qa import junit, run, xcode_command


def results(states):
    counts = {name: states.count(state) for state, name in (
        ('Passed', 'passedTests'), ('Failed', 'failedTests'), ('Skipped', 'skippedTests'),
        ('Expected Failure', 'expectedFailures'))}
    summary = dict(counts, totalTestCount=len(states), result='Failed' if counts['failedTests'] else 'Passed')
    tree = {'testNodes': [{'nodeType': 'Test Suite', 'name': 'Suite', 'children': [
        {'nodeType': 'Test Case', 'name': f'test{i}', 'nodeIdentifier': f'Suite/test{i}', 'result': state}
        for i, state in enumerate(states)]}]}
    return summary, tree


class NativeQATest(unittest.TestCase):
    def test_report_preserves_pass_fail_skip_and_expected_failures(self):
        summary, tree = results(['Passed', 'Failed', 'Skipped', 'Expected Failure'])
        xml, passed = junit(summary, tree)
        cases = list(ET.fromstring(xml).iter('testcase'))
        self.assertFalse(passed)
        self.assertEqual(len(cases), 4)
        self.assertEqual(len([case for case in cases if case.find('failure') is not None]), 1)
        self.assertEqual(len([case for case in cases if case.find('skipped') is not None]), 2)

    def test_missing_unknown_and_inconsistent_results_fail_closed(self):
        for states in ([], ['unknown']):
            with self.assertRaises(ValueError):
                junit(*results(states))
        summary, tree = results(['Passed'])
        summary['passedTests'] = 2
        with self.assertRaises(ValueError):
            junit(summary, tree)
        summary, tree = results(['Skipped'])
        self.assertFalse(junit(summary, tree)[1])
        summary, tree = results(['Passed'])
        summary['result'] = 'Failed'
        self.assertFalse(junit(summary, tree)[1])

    def test_workspace_serial_defaults_and_explicit_targets(self):
        config = {'workspace': 'Other App.xcworkspace', 'scheme': 'Other App', 'test_targets': ['CoreTests', 'ExtensionTests']}
        with patch.dict(os.environ, {'TEST_WORKERS': '1'}):
            args = xcode_command(config, 'test', 'platform=iOS Simulator,id=exact', 'result.xcresult')
        self.assertEqual(args[1:3], ['-workspace', 'Other App.xcworkspace'])
        self.assertIn('-only-testing:ExtensionTests', args)
        self.assertEqual(args[args.index('-parallel-testing-enabled') + 1], 'NO')
        self.assertIn('CODE_SIGNING_ALLOWED=NO', args)
        self.assertEqual(args[args.index('-collect-test-diagnostics') + 1], 'on-failure')
        with patch.dict(os.environ, {'TEST_WORKERS': '3'}), self.assertRaises(ValueError):
            xcode_command(config, 'test', 'destination', 'bundle')

    def test_real_runner_keeps_exit_status_and_never_reuses_stale_junit(self):
        config = json.loads((ROOT / 'examples/minimal.json').read_text())
        resolved = {'TEST_DESTINATION': 'exact', 'SIMULATOR_UDIDS': json.dumps({config['test_device']: 'C71FB2C5-952C-4E6D-A2AD-1ADAE3B28FA1'})}
        summary, tree = results(['Passed'])
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as tmp:
            os.chdir(tmp)
            self.addCleanup(os.chdir, previous)
            output = Path('qa-results/test')
            output.mkdir(parents=True)
            (output / 'report.junit').write_text('<stale/>')
            env = {'SOURCE_SHA': 'a' * 40, 'GITHUB_RUN_ID': '123'}
            with patch.dict(os.environ, env), patch('toolchain.resolve', return_value=(resolved, {})), patch('native_qa.prepare'), \
                 patch('native_qa.stream', return_value=65), patch('native_qa.subprocess.check_output', side_effect=[json.dumps(summary), json.dumps(tree)]):
                self.assertEqual(run('test', config), 65)
            self.assertEqual(json.loads((output / 'result.json').read_text())['source_sha'], 'a' * 40)
            self.assertNotIn('stale', (output / 'report.junit').read_text())
            with patch('toolchain.resolve', return_value=(resolved, {})), patch('native_qa.prepare'), \
                 patch('native_qa.stream', return_value=0), patch('native_qa.subprocess.check_output', side_effect=FileNotFoundError('xcresult unavailable')):
                self.assertNotEqual(run('test', config), 0)
            self.assertFalse((output / 'report.junit').exists())
            self.assertNotEqual(json.loads((output / 'result.json').read_text())['status'], 0)
            os.chdir(previous)

    def test_public_interface_help_does_not_require_app_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = subprocess.check_output([sys.executable, str(ROOT / 'bin/ios-release'), '--commands-json'], cwd=tmp, text=True)
            contract = json.loads(output)
            self.assertEqual(contract['schema_version'], 1)
            self.assertIn('qa', contract['commands'])

    def test_readiness_timeout_prevents_xctest_and_records_failed_qa(self):
        config = json.loads((ROOT / 'examples/minimal.json').read_text())
        resolved = {'TEST_DESTINATION': 'exact', 'SIMULATOR_UDIDS': json.dumps({config['test_device']: 'C71FB2C5-952C-4E6D-A2AD-1ADAE3B28FA1'})}
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as tmp:
            os.chdir(tmp)
            try:
                with patch('toolchain.resolve', return_value=(resolved, {})), patch('native_qa.prepare', side_effect=subprocess.TimeoutExpired('bootstatus', 180)), patch('native_qa.stream') as execute:
                    self.assertNotEqual(run('test', config), 0)
                    execute.assert_not_called()
                self.assertNotEqual(json.loads(Path('qa-results/test/result.json').read_text())['status'], 0)
                self.assertFalse(Path('qa-results/test/report.junit').exists())
            finally:
                os.chdir(previous)


if __name__ == '__main__':
    unittest.main()
