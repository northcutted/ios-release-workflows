#!/usr/bin/env python3
"""Run native QA and preserve source-bound evidence without Ruby or Fastlane."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid
import xml.etree.ElementTree as ET
from simulator import prepare


def junit(summary, tests):
    """Cross-check individual results against Apple's summary before emitting JUnit."""
    cases = []

    def visit(nodes, parents=()):
        for node in nodes:
            if node['nodeType'] == 'Test Case':
                cases.append((node, parents))
            else:
                visit(node.get('children', []), (*parents, node['name']))

    visit(tests['testNodes'])
    counts = Counter(node.get('result') for node, _ in cases)
    states = {'Passed': 'passedTests', 'Failed': 'failedTests', 'Skipped': 'skippedTests',
              'Expected Failure': 'expectedFailures'}
    if not cases or set(counts) - states.keys():
        raise ValueError('Missing test cases or unknown XCTest result')
    if summary['totalTestCount'] != len(cases) or any(
            not isinstance(summary[key], int) or summary[key] != counts[state]
            for state, key in states.items()):
        raise ValueError('XCTest summary and individual results disagree')
    if summary['result'] not in states:
        raise ValueError('Unknown XCTest summary result')
    root = ET.Element('testsuites')
    suite = ET.SubElement(root, 'testsuite', name=summary.get('title', 'XCTest'), tests=str(len(cases)),
                          failures=str(counts['Failed']), errors='0',
                          skipped=str(counts['Skipped'] + counts['Expected Failure']))
    for node, parents in cases:
        identifier = node.get('nodeIdentifier', node['name'])
        case = ET.SubElement(suite, 'testcase', name=identifier,
                             classname='/'.join(parents), time=str(node.get('durationInSeconds', 0)))
        if node['result'] == 'Failed':
            messages = [failure.get('failureText', '') for failure in summary.get('testFailures', [])
                        if failure.get('testIdentifierString') == identifier]
            failure = ET.SubElement(case, 'failure', message='XCTest failure')
            failure.text = '\n'.join(messages) or json.dumps(node.get('children', []))
        elif node['result'] in ('Skipped', 'Expected Failure'):
            ET.SubElement(case, 'skipped', message=node['result'])
    valid = summary['result'] == 'Passed' and counts['Failed'] == 0 and counts['Passed'] > 0
    return ET.tostring(root, encoding='unicode') + '\n', valid


def xcode_command(config, name, destination, result_bundle=None):
    workers = os.environ.get('TEST_WORKERS', '1')
    if workers not in ('1', '2'):
        raise ValueError('TEST_WORKERS must be 1 or 2')
    kind = 'workspace' if config.get('workspace') else 'project'
    args = ['xcodebuild', '-' + kind, config[kind], '-scheme', config['scheme'],
            '-configuration', 'Debug', '-destination', destination, 'CODE_SIGNING_ALLOWED=NO']
    if name == 'analyze':
        return args + ['-sdk', 'iphonesimulator', 'analyze']
    if not config['test_targets']:
        raise ValueError('Explicit test targets are required')
    return args + ['-parallel-testing-enabled', 'YES' if workers == '2' else 'NO',
                   '-parallel-testing-worker-count', workers, '-maximum-concurrent-test-simulator-destinations', workers,
                   '-collect-test-diagnostics', 'on-failure',
                   '-resultBundlePath', str(result_bundle),
                   *['-only-testing:' + target for target in config['test_targets']], 'clean', 'test']


def stream(command, log):
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    for line in process.stdout:
        print(line, end='', flush=True)
        log.write(line)
    return process.wait()


def run(name, config):
    root = Path('qa-results') / name
    root.mkdir(parents=True, exist_ok=True)
    # A failed invocation must never inherit a previous green report.
    (root / 'report.junit').unlink(missing_ok=True)
    output = Path('build/test_output')
    if name.startswith('test'):
        output.mkdir(parents=True, exist_ok=True)
        (output / 'report.junit').unlink(missing_ok=True)
    status = 1
    try:
        with (root / 'output.log').open('w') as log:
            if name == 'lint':
                command = ['swiftlint', 'lint', '--strict', '--no-cache', '--config', '.swiftlint.yml']
            elif name == 'localization':
                command = [sys.executable, str(Path(__file__).with_name('localization.py'))]
            else:
                from toolchain import resolve
                values, environment = resolve(config, compatibility=name == 'test-compatibility')
                os.environ.update(values)
                Path('build').mkdir(exist_ok=True)
                Path('build/build-env.json').write_text(json.dumps(environment, indent=2) + '\n')
                if name.startswith('test'):
                    selected = json.loads(values['SIMULATOR_UDIDS'])[config['test_device']]
                    prepare(selected, root / 'simulator-readiness.json')
                bundle = output / (name + '-' + uuid.uuid4().hex + '.xcresult')
                command = xcode_command(config, name, values['TEST_DESTINATION'], bundle)
            status = stream(command, log)
        if name.startswith('test'):
            reports = {}
            for report in ('summary', 'tests'):
                raw = subprocess.check_output(['xcrun', 'xcresulttool', 'get', 'test-results', report,
                                               '--path', str(bundle), '--compact'], text=True)
                (root / (report + '.json')).write_text(raw)
                reports[report] = json.loads(raw)
            xml, passed = junit(reports['summary'], reports['tests'])
            (root / 'report.junit').write_text(xml)
            (output / 'report.junit').write_text(xml)
            status = status or (0 if passed else 1)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        status = status or 1
        print(f'QA evidence failed: {error}', file=sys.stderr)
    finally:
        result = {'check': name, 'status': status,
                  'source_sha': os.environ.get('SOURCE_SHA', os.environ.get('GITHUB_SHA')),
                  'run_id': os.environ.get('GITHUB_RUN_ID')}
        (root / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    return status


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('check', choices=['lint', 'localization', 'analyze', 'test', 'test-compatibility'])
    args = parser.parse_args()
    from configuration import CONFIG
    sys.exit(run(args.check, CONFIG))
