"""Bounded simulator readiness and conservative XCTest bootstrap classification."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid


def device_id(value):
    if str(uuid.UUID(value)).lower() != value.lower():
        raise ValueError('An exact simulator UUID is required')
    return value


def command(args, timeout=30):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=True).stdout


def device(value):
    devices = json.loads(command(['xcrun', 'simctl', 'list', 'devices', 'available', '--json']))['devices']
    matches = [d for group in devices.values() for d in group if d['udid'].lower() == value.lower()]
    if len(matches) != 1 or not matches[0].get('isAvailable'):
        raise ValueError('The selected simulator is not uniquely available')
    return matches[0]


def prepare(value, output, timeout=180):
    value = device_id(value)
    path = Path(output)
    path.parent.mkdir(parents=True, exist_ok=True)
    result = {'udid': value, 'status': 'failed', 'timeout_seconds': timeout}
    started = time.monotonic()
    try:
        result['before'] = device(value)
        result['boot_log'] = command(['xcrun', 'simctl', 'bootstatus', value, '-b'], timeout=timeout)
        result['after'] = device(value)
        if result['after']['state'] != 'Booted':
            raise ValueError('Simulator boot did not reach Booted')
        result['status'] = 'ready'
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        result['error'] = str(error)
        if isinstance(error, subprocess.TimeoutExpired):
            result['boot_log'] = (error.stdout or b'').decode(errors='replace') if isinstance(error.stdout, bytes) else error.stdout
        raise
    finally:
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        path.write_text(json.dumps(result, indent=2) + '\n')
    return result


def reset(value):
    value = device_id(value)
    if os.getenv('GITHUB_ACTIONS') != 'true' or os.getenv('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise ValueError('Automatic simulator reset is limited to disposable GitHub-hosted jobs')
    if value.lower() not in [device_id(v).lower() for v in json.loads(os.environ['SIMULATOR_UDIDS']).values()]:
        raise ValueError('Simulator reset must use a configured, resolved device')
    selected = device(value)
    if selected['state'] == 'Booted':
        command(['xcrun', 'simctl', 'shutdown', value], timeout=60)
    elif selected['state'] != 'Shutdown':
        raise ValueError('Simulator is in an unexpected state; refusing automatic reset')
    command(['xcrun', 'simctl', 'erase', value], timeout=60)


def bootstrap_failure(summary, tests):
    """A runner's synthetic failure case is not an executed application test."""
    if not isinstance(summary, dict) or not isinstance(tests, dict):
        return False
    runner = re.compile(r'^\S+-Runner \(\d+\) encountered an error$')
    failures = summary.get('testFailures', [])
    cases = []

    def visit(nodes):
        if not isinstance(nodes, list):
            return False
        for node in nodes:
            if not isinstance(node, dict):
                return False
            if node.get('nodeType') == 'Test Case':
                cases.append(node)
            if not visit(node.get('children', [])):
                return False
        return True

    valid_tree = visit(tests.get('testNodes', []))
    counts = [summary.get(key) for key in ('passedTests', 'skippedTests', 'expectedFailures', 'failedTests', 'totalTestCount')]
    if (not valid_tree or any(type(value) is not int or value < 0 for value in counts)
            or not isinstance(failures, list) or any(not isinstance(f, dict) for f in failures)
            or summary.get('result') != 'Failed' or not failures or not cases
            or any(summary.get(key) != 0 for key in ('passedTests', 'skippedTests', 'expectedFailures'))
            or summary.get('failedTests') != len(cases) or summary.get('totalTestCount') != len(cases)
            or len(failures) != len(cases)):
        return False
    identifiers = [failure.get('testIdentifierString', '') for failure in failures]
    if any(not isinstance(identifier, str) or not runner.fullmatch(identifier) for identifier in identifiers):
        return False
    case_ids = [case.get('nodeIdentifier', '') for case in cases]
    if any(not isinstance(identifier, str) for identifier in case_ids) or sorted(identifiers) != sorted(case_ids):
        return False
    if any(case.get('result') != 'Failed' for case in cases):
        return False
    for failure in failures:
        text = failure.get('failureText', '')
        if not isinstance(text, str):
            return False
        stalled = ('Early unexpected exit, operation never finished bootstrapping' in text
                   and 'The test runner crashed while preparing to run tests:' in text
                   and '-[XCTWaiter(StallHandling) handleStalledWait:]' in text)
        timed_out = text == 'The test runner timed out while preparing to run tests.'
        if not (stalled or timed_out):
            return False
        case = next(case for case in cases if case['nodeIdentifier'] == failure['testIdentifierString'])
        messages = [node.get('name') for node in case.get('children', []) if node.get('nodeType') == 'Failure Message']
        if messages != [text] or case.get('name') != failure['testIdentifierString']:
            return False
    return True


def inspect_bundle(bundle, output):
    path = Path(output)
    path.parent.mkdir(parents=True, exist_ok=True)
    assessment = {'recoverable': False, 'reason': 'Missing or unrecognized XCTest evidence'}
    try:
        reports = {}
        for report in ('summary', 'tests'):
            raw = command(['xcrun', 'xcresulttool', 'get', 'test-results', report, '--path', str(bundle), '--compact'])
            (path.parent / (report + '.json')).write_text(raw)
            reports[report] = json.loads(raw)
        assessment['recoverable'] = bootstrap_failure(reports['summary'], reports['tests'])
        assessment['reason'] = 'Recognized bootstrap failure; no application tests executed' if assessment['recoverable'] else 'Application test results or unrecognized failure; recovery refused'
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        assessment['error'] = str(error)
    path.write_text(json.dumps(assessment, indent=2) + '\n')
    return assessment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='operation', required=True)
    ready = sub.add_parser('prepare')
    ready.add_argument('udid')
    ready.add_argument('--output', required=True)
    erase = sub.add_parser('reset')
    erase.add_argument('udid')
    inspect = sub.add_parser('inspect')
    inspect.add_argument('bundle')
    inspect.add_argument('--output', required=True)
    args = parser.parse_args()
    if args.operation == 'prepare':
        result = prepare(args.udid, args.output)
        print(json.dumps({key: result[key] for key in ('udid', 'status', 'elapsed_seconds')}))
    elif args.operation == 'reset':
        reset(args.udid)
    else:
        print(json.dumps(inspect_bundle(args.bundle, args.output)))


if __name__ == '__main__':
    main()
