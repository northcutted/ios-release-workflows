#!/usr/bin/env python3
"""Compare the compiled CLI against the deployed Python contracts, without Xcode."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(ROOT / 'scripts/ci'))
from native_qa import junit
from simulator import bootstrap_failure


def normalized(xml):
    root = ET.fromstring(xml)
    for node in root.iter('failure'):
        try:
            node.text = json.dumps(json.loads(node.text or ''), sort_keys=True)
        except ValueError:
            pass
    return [(node.tag, {key: float(value) if key == 'time' else value
                        for key, value in sorted(node.attrib.items())}, node.text or '')
            for node in root.iter()]


def pair(states):
    summary = {key: states.count(state) for state, key in (
        ('Passed', 'passedTests'), ('Failed', 'failedTests'),
        ('Skipped', 'skippedTests'), ('Expected Failure', 'expectedFailures'))}
    summary.update(totalTestCount=len(states), result='Failed' if 'Failed' in states else 'Passed')
    return summary, {'testNodes': [{'nodeType': 'Test Suite', 'name': 'Suite', 'children': [
        {'nodeType': 'Test Case', 'name': f'test{i}', 'nodeIdentifier': f'Suite/test{i}', 'result': state}
        for i, state in enumerate(states)]}]}


with tempfile.TemporaryDirectory() as directory:
    temporary = Path(directory)
    def compare(summary, tests):
        (temporary / 'summary.json').write_text(json.dumps(summary))
        (temporary / 'tests.json').write_text(json.dumps(tests))
        command = subprocess.run([str(BINARY), 'xcresult-report', '--summary', str(temporary / 'summary.json'),
                                  '--tests', str(temporary / 'tests.json')], text=True, capture_output=True)
        try:
            xml, passed = junit(summary, tests)
        except (ValueError, KeyError, TypeError):
            assert command.returncode != 0, command.stdout
            return
        assert command.returncode == 0, command.stderr
        report = json.loads(command.stdout)
        assert report['passed'] == passed
        assert report['bootstrap_recoverable'] == bootstrap_failure(summary, tests)
        assert normalized(report['junit']) == normalized(xml)

    for states in [[], ['Passed'], ['Failed'], ['Skipped'], ['Expected Failure'],
                   ['Passed', 'Failed', 'Skipped', 'Expected Failure'], ['unknown']]:
        compare(*pair(states))
    summary, tests = pair(['Passed'])
    for key in ['totalTestCount', 'passedTests', 'failedTests']:
        changed = copy.deepcopy(summary); changed[key] = 2
        compare(changed, tests)
    fixtures = ROOT / 'scripts/ci/tests/fixtures'
    summary = json.loads((fixtures / 'simulator-bootstrap-summary.json').read_text())
    tests = json.loads((fixtures / 'simulator-bootstrap-tests.json').read_text())
    compare(summary, tests)
    for text in ['XCTAssertTrue failed', 'Failed to launch application via Xcode',
                 'The test runner crashed while running tests', 'Unknown startup failure',
                 'The test runner timed out while preparing to run tests.']:
        changed = copy.deepcopy(summary); changed['testFailures'][0]['failureText'] = text
        tree = copy.deepcopy(tests)
        tree['testNodes'][0]['children'][0]['children'][0]['children'][0]['children'][0]['name'] = text
        compare(changed, tree)

    config = json.loads((ROOT / 'examples/minimal.json').read_text())
    config.update(localization_catalogs=['Localizable.xcstrings'], localization_locales=['de', 'ar'])
    if os.environ.get('GITHUB_REPOSITORY'):
        config['repository'] = os.environ['GITHUB_REPOSITORY']
    (temporary / 'app.json').write_text(json.dumps(config))
    os.environ['IOS_RELEASE_CONFIG'] = str(temporary / 'app.json')
    import localization
    translated = lambda value: {'stringUnit': {'state': 'translated', 'value': value}}
    def catalog(english, german, arabic):
        return {'sourceLanguage': 'en', 'strings': {english: {'localizations': {
            'de': translated(german), 'ar': translated(arabic)}}}}
    samples = [catalog('Hello', 'Hallo', 'Hello'),
               catalog('${name} **hello** %d', '${name} **hallo** %d', '${name} **hi** %d'),
               catalog('${name}', '${other}', '${name}'),
               catalog('%@ %d', '%2$d %1$@', '%1$@ %d'),
               catalog('Hello', '', 'Hello'),
               catalog('Hello', '^[Hallo](inflect: true)', 'Hello'),
               {'strings': {'ignored': {'shouldTranslate': False}}},
               {'strings': {'Missing': {}}}]
    for sample in samples:
        path = temporary / 'Localizable.xcstrings'; path.write_text(json.dumps(sample))
        expected = localization.audit_catalog(path)
        result = subprocess.run([str(BINARY), '--app-root', str(temporary), '--config', 'app.json',
                                 'qa', 'localization'], capture_output=True, text=True)
        assert (result.returncode == 0) == (not expected), (sample, expected, result.stderr)
print('Rust/Python parity passed: 16 XCTest scenarios and 8 localization scenarios.')
