"""One command surface for local builds and hosted release operations."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from .toolkit import root as toolkit_root

COMMANDS = {
    "setup": (None, "Prepare the task's locked tools; --apple adds Ruby, --images adds app image dependencies"),
    "doctor": (None, "Check Python, optional Apple tools and configured Xcode without changing them"),
    "check": (None, "Check workflow policy, documentation and regression tests; --syntax adds actionlint"),
    "docs": (None, "Generate or check deterministic consumer documentation"),
    "version": (None, "Calculate a read-only candidate version and release notes"),
    "localization-pseudo": (None, "Pseudo-localize catalogs for layout smoke checks"),
    "screenshots-compose": (None, "Run app-owned screenshot composition with its locked image tools"),
    "test": ("ci/native_qa.py", "Run the configured simulator tests and preserve QA evidence"),
    "qa": ("ci/native_qa.py", "Run lint, localization, analysis or tests; preserve QA evidence"),
    "archive": ("developer.py", "Archive and export locally using installed signing profiles"),
    "screenshots": ("developer.py", "Run app-owned screenshot scenarios with locked platform dependencies"),
    "screenshots-capture": ("developer.py", "Compatibility alias for screenshots"),
    "gems-install": ("developer.py", "Compatibility alias for setup --apple"),
    "toolchain": ("ci/toolchain.py", "Validate Xcode and resolve exact simulators"),
    "screenshots-verify": ("ci/evidence.py", "Validate configured screenshot coverage and dimensions"),
    "screenshots-publish": ("ci/screenshot_pr.sh", "Create the screenshot update PR in its protected workflow"),
    "deployment-ref": ("ci/deployment_ref.py", "Verify a protected deployment or metadata operation tag"),
    "release-verify": ("ci/verify_release.py", "Authenticate downloaded release evidence"),
    "controls-configure": ("ci/configure_repository.py", "Preview repository controls; --apply explicitly changes them"),
    "controls-capture": ("ci/capture_controls.py", "Capture an owner-verified repository control baseline"),
    "benchmark": ("ci/benchmark.py", "Compare workflow job timings"),
    "install-actionlint": ("ci/install_actionlint.sh", "Install the pinned workflow validator"),
}


def command_contract():
    return {"schema_version": 1, "commands": {name: {"description": description} for name, (_, description) in COMMANDS.items()}}


def apple_environment(platform, app):
    prefix = ruby_command(platform)
    path = str(Path(prefix[0]).parent) + os.pathsep + os.environ.get("PATH", "") if prefix else os.environ.get("PATH", "")
    return {**os.environ, "PATH": path, "BUNDLE_GEMFILE": str(platform / "Gemfile"), "BUNDLE_FROZEN": "true",
            "BUNDLE_PATH": os.environ.get("BUNDLE_PATH", str(app / "build/platform-gems")),
            "BUNDLE_APP_CONFIG": os.environ.get("BUNDLE_APP_CONFIG", str(app / "build/platform-bundle")),
            "FASTLANE_SKIP_DOCS": "true", "FASTLANE_SKIP_UPDATE_CHECK": "true", "FASTLANE_OPT_OUT_USAGE": "true"}


def ruby_command(platform, install=False):
    expected = (platform / ".ruby-version").read_text().strip()
    actual = subprocess.check_output(["ruby", "-e", "print RUBY_VERSION"], text=True) if shutil.which("ruby") else None
    if actual == expected:
        return []
    if shutil.which("mise"):
        if install:
            subprocess.run(["mise", "install", "ruby@" + expected], check=True)
        installed = subprocess.check_output(["mise", "where", "ruby@" + expected], text=True).strip()
        # Resolve the executable explicitly: a user's shell configuration can
        # prepend another Ruby to PATH even inside `mise exec`.
        return [str(Path(installed) / "bin/ruby"), "-S"]
    raise ValueError(f"Ruby {expected} is required for Apple operations. Install it or install mise and run setup --apple.")


def setup(argv, platform, app):
    parser = argparse.ArgumentParser(description=COMMANDS["setup"][1])
    parser.add_argument("--apple", action="store_true")
    parser.add_argument("--images", action="store_true")
    parser.add_argument("--checks", action="store_true", help="Install the pinned workflow syntax checker")
    args = parser.parse_args(argv)
    # The checkout launcher has already synchronized Python from uv.lock.
    if args.checks or not (args.apple or args.images):
        from .tools import actionlint
        actionlint(app)
    if args.apple:
        prefix = ruby_command(platform, install=True)
        subprocess.run([*prefix, "bundle", "install", "--jobs", "4", "--retry", "3"], env=apple_environment(platform, app), check=True, cwd=app)
    if args.images:
        python = app / "build/ios-release-images/bin/python"
        subprocess.run(["uv", "venv", "--python", sys.executable, str(python.parent.parent)], check=True)
        subprocess.run(["uv", "pip", "install", "--python", str(python), "--require-hashes", "-r", str(app / "scripts/requirements.txt")], check=True)
    print("Locked tools are ready.")
    return 0


def doctor(argv, platform, app):
    parser = argparse.ArgumentParser(description=COMMANDS["doctor"][1])
    parser.add_argument("--apple", action="store_true")
    parser.add_argument("--xcode", action="store_true")
    args = parser.parse_args(argv)
    results = {"python": {"status": "ok", "version": sys.version.split()[0]}, "platform": {"status": "ok", "root": str(platform)}}
    if args.apple:
        try:
            prefix = ruby_command(platform)
            subprocess.run([*prefix, "bundle", "check"], env=apple_environment(platform, app), check=True, stdout=subprocess.DEVNULL)
            results["ruby"] = {"status": "ok", "version": (platform / ".ruby-version").read_text().strip()}
        except (ValueError, OSError, subprocess.SubprocessError):
            results["ruby"] = {"status": "missing", "remedy": "ios-release setup --apple"}
    if args.xcode:
        try:
            result = subprocess.run([sys.executable, str(platform / "scripts/ci/toolchain.py")], capture_output=True, text=True)
            results["xcode"] = {"status": "ok" if result.returncode == 0 else "mismatch", "detail": result.stdout.strip() or result.stderr.strip()}
        except OSError as error:
            results["xcode"] = {"status": "missing", "detail": str(error)}
    print(json.dumps(results, indent=2))
    return int(any(value["status"] != "ok" for value in results.values()))


def check(argv, platform, app):
    parser = argparse.ArgumentParser(description=COMMANDS["check"][1])
    parser.add_argument("--syntax", action="store_true")
    parser.add_argument("--apple", action="store_true", help="Run platform Ruby contract tests as well")
    args = parser.parse_args(argv)
    from . import docs, policy
    from .yamlio import workflows
    platform_mode = docs.profile(app)["mode"] == "platform"
    consumer = app / "scripts/ci/workflow_policy.py"
    if (app / ".github/ios-release-platform.json").exists():
        subprocess.run([sys.executable, str(consumer)], check=True, cwd=app)
        metadata = app / "scripts/validate_store_metadata.py"
        if metadata.exists():
            subprocess.run([sys.executable, str(metadata)], check=True, cwd=app)
    else:
        errors = policy.validate(workflows(app / ".github/workflows"))
        if errors:
            raise ValueError("\n".join(errors))
    errors = docs.run(app, True)
    if errors:
        raise ValueError("\n".join(errors))
    if args.syntax:
        from .tools import actionlint
        policy.lint(app, str(actionlint(app)))
    # Keep each suite in a separate process: release tests import configuration at startup.
    test_env = {**os.environ}
    if platform_mode:
        # Actions installs tools from its immutable action checkout, which is
        # a different directory from the platform workspace under test.
        test_env.update(IOS_RELEASE_CONFIG=str(app / "examples/picstrip.json"), IOS_RELEASE_REVISION="f" * 40,
                        IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS=json.dumps(["f" * 40]))
        test_env.pop("GITHUB_REPOSITORY", None)
    for directory in [app / "tests", app / "scripts/ci/tests"]:
        # macOS can resolve `tests` to an unrelated Xcode `Tests` fixture folder.
        if directory.is_dir() and directory.name in os.listdir(directory.parent):
            subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", str(directory), "-p", "test_*.py"], cwd=app, env=test_env, check=True)
    if args.apple:
        if not platform_mode:
            raise ValueError("--apple contract tests belong to the platform checkout")
        prefix = ruby_command(platform)
        ruby_env = apple_environment(platform, app)
        ruby_env.update({key: value for key, value in test_env.items() if key.startswith("IOS_RELEASE_")})
        ruby_env.pop("GITHUB_REPOSITORY", None)
        for file in sorted((app / "scripts/ci/tests").glob("*_test.rb")):
            subprocess.run([*prefix, "bundle", "exec", "ruby", str(file)], env=ruby_env, cwd=platform, check=True)
    print("Workflow policy, documentation and regression checks passed.")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-root", default=os.environ.get("IOS_APP_ROOT", os.getcwd()))
    parser.add_argument("--config")
    parser.add_argument("--commands-json", action="store_true")
    parser.add_argument("command", nargs="?", choices=COMMANDS)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.commands_json:
        print(json.dumps(command_contract(), indent=2))
        return 0
    if not args.command:
        parser.print_help()
        return 0
    platform, app = toolkit_root(), Path(args.app_root).resolve()
    config = Path(args.config or os.environ.get("IOS_RELEASE_CONFIG", ".github/ios-release.json"))
    if not config.is_absolute():
        config = app / config
    os.environ.update(IOS_APP_ROOT=str(app), IOS_RELEASE_ROOT=str(platform), IOS_RELEASE_CONFIG=str(config.resolve()))
    try:
        if args.command == "docs":
            from .docs import main as run
            return run(args.arguments)
        if args.command == "version":
            from .version import main as run
            return run(args.arguments)
        if args.command == "localization-pseudo":
            from .pseudo import main as run
            return run(args.arguments)
        if args.command == "install-actionlint":
            from .tools import actionlint
            executable = actionlint(app)
            if os.environ.get("GITHUB_PATH"):
                with open(os.environ["GITHUB_PATH"], "a") as out:
                    out.write(str(executable.parent) + "\n")
            print(executable)
            return 0
        if args.command == "screenshots-compose":
            python = app / "build/ios-release-images/bin/python"
            if not python.exists():
                raise ValueError("Run ios-release setup --images before composing screenshots")
            return subprocess.call([str(python), str(app / "scripts/compose_screenshots.py"), *args.arguments], cwd=app)
        if args.command in {"setup", "doctor", "check"}:
            return {"setup": setup, "doctor": doctor, "check": check}[args.command](args.arguments, platform, app)
        script = platform / "scripts" / COMMANDS[args.command][0]
        extra = ["screenshots"] if args.command == "screenshots-verify" else ["test"] if args.command == "test" else []
        if script.name == "developer.py":
            if args.command == "gems-install":
                return setup(["--apple", *args.arguments], platform, app)
            extra = ["screenshots-capture" if args.command == "screenshots" else args.command]
            prefix = ruby_command(platform)
            environment = apple_environment(platform, app)
            environment["IOS_RELEASE_RUBY_PREFIX"] = json.dumps(prefix)
        else:
            environment = os.environ.copy()
        runtime = sys.executable if script.suffix == ".py" else "bash"
        return subprocess.call([runtime, str(script), *extra, *args.arguments], cwd=app, env=environment)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1
