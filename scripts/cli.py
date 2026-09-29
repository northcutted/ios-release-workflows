"""A versioned command surface; internal script paths may change behind it."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
COMMANDS = {
    'gems-install': ('developer.py', 'Install the locked platform Ruby dependencies for local Apple tools'),
    'archive': ('developer.py', 'Archive and export locally using installed signing profiles'),
    'screenshots-capture': ('developer.py', 'Run app-owned screenshot scenarios with platform dependencies'),
    'qa': ('ci/native_qa.py', 'Run lint, localization, analysis or tests; preserve QA evidence'),
    'toolchain': ('ci/toolchain.py', 'Validate Xcode and resolve exact simulators'),
    'screenshots-verify': ('ci/evidence.py', 'Validate configured screenshot coverage and dimensions'),
    'screenshots-publish': ('ci/screenshot_pr.sh', 'Create the screenshot update PR in its protected workflow'),
    'deployment-ref': ('ci/deployment_ref.py', 'Verify a protected deployment or metadata operation tag'),
    'release-verify': ('ci/verify_release.py', 'Authenticate downloaded release evidence'),
    'controls-configure': ('ci/configure_repository.py', 'Preview repository controls; --apply explicitly changes them'),
    'controls-capture': ('ci/capture_controls.py', 'Capture an owner-verified repository control baseline'),
    'benchmark': ('ci/benchmark.py', 'Compare workflow job timings'),
    'install-actionlint': ('ci/install_actionlint.sh', 'Install the pinned workflow validator'),
    'docs': ('docs.mjs', 'Generate or check deterministic consumer documentation'),
}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-root', default=os.environ.get('IOS_APP_ROOT', os.getcwd()))
    parser.add_argument('--config', help='App-relative or absolute configuration path')
    parser.add_argument('--commands-json', action='store_true', help='Print the public command contract without executing it')
    parser.add_argument('command', nargs='?', choices=COMMANDS)
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.commands_json:
        print(json.dumps({'schema_version': 1, 'commands': {name: {'description': description}
              for name, (_, description) in COMMANDS.items()}}, indent=2))
        return 0
    if not args.command:
        parser.print_help()
        return 0
    app = Path(args.app_root).resolve()
    config = Path(args.config or os.environ.get('IOS_RELEASE_CONFIG', '.github/ios-release.json'))
    if not config.is_absolute():
        config = app / config
    env = {**os.environ, 'IOS_APP_ROOT': str(app), 'IOS_RELEASE_ROOT': str(ROOT),
           'IOS_RELEASE_CONFIG': str(config.resolve())}
    script = ROOT / 'scripts' / COMMANDS[args.command][0]
    runtime = {'.py': sys.executable, '.sh': 'bash', '.mjs': 'node'}[script.suffix]
    extra = ['screenshots'] if args.command == 'screenshots-verify' else [args.command] if script.name == 'developer.py' else []
    return subprocess.call([runtime, str(script), *extra, *args.arguments], cwd=app, env=env)
