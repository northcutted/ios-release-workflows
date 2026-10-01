"""Local build/screenshot adapters using only platform-owned Ruby dependencies."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('gems-install')
    archive = sub.add_parser('archive')
    archive.add_argument('--version', required=True)
    archive.add_argument('--build-number', required=True)
    screenshots = sub.add_parser('screenshots-capture')
    screenshots.add_argument('--devices')
    screenshots.add_argument('--languages')
    args = parser.parse_args()
    app = Path(os.environ['IOS_APP_ROOT'])
    env = {**os.environ, 'BUNDLE_GEMFILE': str(ROOT / 'Gemfile'), 'BUNDLE_FROZEN': 'true',
           'BUNDLE_PATH': os.environ.get('BUNDLE_PATH', str(app / 'build/platform-gems')),
           'BUNDLE_APP_CONFIG': os.environ.get('BUNDLE_APP_CONFIG', str(app / 'build/platform-bundle')),
           'FASTLANE_SKIP_DOCS': 'true', 'FASTLANE_SKIP_UPDATE_CHECK': 'true', 'FASTLANE_OPT_OUT_USAGE': 'true'}
    if args.command == 'gems-install':
        command = ['bundle', 'install', '--jobs', '4', '--retry', '3']
    elif args.command == 'archive':
        env.update(VERSION=args.version, BUILD_NUMBER=args.build_number)
        command = ['bundle', 'exec', 'ruby', str(ROOT / 'scripts/fastlane.rb'), 'build']
    else:
        # Screenshot scenarios are intentionally app-owned and run without release secrets.
        env.update(IOS_RELEASE_SIMULATOR_HELPER=str(ROOT / 'fastlane/lib/simulator_recovery.rb'),
                   IOS_RELEASE_SIMULATOR_TOOL=str(ROOT / 'scripts/ci/simulator.py'),
                   IOS_RELEASE_PYTHON=sys.executable)
        command = ['bundle', 'exec', 'fastlane', 'screenshots']
        if args.devices:
            command.append('devices:' + args.devices)
        if args.languages:
            command.append('languages:' + args.languages)
    return subprocess.call([*json.loads(env.get('IOS_RELEASE_RUBY_PREFIX', '[]')), *command], cwd=app, env=env)


if __name__ == '__main__':
    raise SystemExit(main())
