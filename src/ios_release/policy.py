"""Validate platform workflow trust boundaries and normalize syntax for actionlint."""
import json
from pathlib import Path
import re
import subprocess
import tempfile

import yaml
from .yamlio import workflows

SLSA = "slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v2.1.0"


def validate(items):
    errors = []
    def check(value, message):
        if not value:
            errors.append(message)
    for file, w in items.items():
        check(w.get("permissions") == {"contents": "read"}, file + ": default token must be read-only")
        events = w.get("on") or {}
        check("pull_request_target" not in events, file + ": pull_request_target is forbidden")
        contract = (events.get("workflow_call") or {}).get("secrets", {})
        for key in re.findall(r"secrets\.([A-Z_]+)", json.dumps(w.get("jobs"))):
            check(key in contract, file + ": environment secret requires explicit reusable contract: " + key)
        check(file != "ci.yml" or not contract, file + ": CI cannot declare release secrets")
        for name, j in w.get("jobs", {}).items():
            label, body = file + "/" + name, json.dumps(j)
            scripts = "\n".join(s.get("run", "") for s in j.get("steps", []))
            permissions = j.get("permissions") or {}
            if j.get("uses"):
                check(j["uses"] == SLSA or re.fullmatch(r"\$/\.github/workflows/\w[\w-]*\.yml", j["uses"]), label + ": reusable workflow must use pinned platform or generator exception")
                continue
            check(type(j.get("timeout-minutes")) is int and j["timeout-minutes"] <= 90, label + ": bounded timeout required")
            check("self-hosted" not in body, label + ": hosted runners required")
            check(not re.search(r"\$\{\{\s*(inputs\.|github\.event\.)", scripts), label + ": event/input values must use environment variables")
            for step in j.get("steps", []):
                uses = step.get("uses", "")
                if not uses:
                    continue
                check(re.search(r"@[a-f0-9]{40}$", uses) or re.fullmatch(r"\$/actions/[a-z-]+", uses), label + ": action must be pinned by SHA or self revision")
                if uses.startswith("actions/checkout@"):
                    check((step.get("with") or {}).get("persist-credentials") is False, label + ": checkout credentials must not persist")
                check(not uses.startswith("actions/cache@"), label + ": opaque executable caches forbidden")
            if file == "ci.yml":
                check("secrets." not in body and not j.get("environment") and not permissions.get("id-token"), label + ": PR path must not receive secrets or privilege")
            if re.search(r'fastlane\.rb" (build|test|analyze)|swiftlint lint|(?:bin/ios-release"|ios-release) qa (analyze|test)|ios-release (archive|test)|ios-release apple build', scripts):
                check(not permissions.get("id-token") and not permissions.get("attestations"), label + ": compilation must not sign provenance")
            if re.search(r'fastlane\.rb" build|ios-release apple build', scripts):
                check(j.get("environment") == "signing" and "APP_STORE_CONNECT_API_KEY" not in body, label + ": compilation signing boundary")
            if re.search(r'fastlane\.rb" submit|ios-release apple submit', scripts):
                check(j.get("environment") == "production" and ("fetch.py" in scripts or "ios-release fetch" in scripts) and (j.get("concurrency") or {}).get("cancel-in-progress") is False, label + ": review requires production approval and reauthentication")
            if j.get("environment") and j["environment"] not in {"signing", "app-store-observe"}:
                check(not re.search(r"bundle exec fastlane|scripts/ci/download_release", scripts), label + ": privileged job may not execute consumer Fastlane")
    check(not re.search(r"fastlane|actions/ruby|bundle exec|setup --apple|ios-release apple", json.dumps(items["ci.yml"])), "Native QA must not load Ruby or Fastlane")
    prepare, release = items["prepare.yml"], items["release.yml"]
    needs = prepare["jobs"]["build"].get("needs", [])
    check("qa" not in ([needs] if isinstance(needs, str) else needs), "Archive must run alongside QA")
    check("upload" not in prepare["jobs"] and "publish" not in prepare["jobs"], "Preparation must not distribute")
    check(items["promote.yml"]["jobs"]["upload"].get("needs") == "verify", "Upload must depend on verification")
    check(items["ci.yml"]["jobs"]["gate"].get("if") == "always()", "CI gate must always report")
    jobs = release["jobs"]
    check(jobs["promote"].get("needs") == "resolve" and jobs["deploy-existing"].get("needs") == "resolve", "Release mutations must depend on verified selection")
    resolve = jobs["resolve"]
    check(not resolve.get("environment") and "secrets." not in json.dumps(resolve) and not (resolve.get("permissions") or {}).get("id-token"), "Selection must remain read-only and secret-free")
    check(jobs["deploy-existing"].get("environment") == "release-publishing", "Operation tag creation requires the restricted publisher environment")
    opts = jobs["promote"]["with"]
    check(opts.get("artifact_id") == "${{ needs.resolve.outputs.artifact_id }}" and opts.get("sha256") == "${{ needs.resolve.outputs.sha256 }}", "Promotion must consume frozen verified artifact identity")
    check(opts.get("upload_adapter") == "${{ needs.resolve.outputs.upload_adapter }}", "Upload adapter must come from verified candidate configuration")
    check(all(job.get("secrets") != "inherit" for job in jobs.values()), "Release secrets must remain explicitly bound")
    return errors


def lint(root):
    root = Path(root).resolve()
    with tempfile.TemporaryDirectory(prefix="ios-actionlint-") as temp:
        paths = []
        for file, w in workflows(root / ".github/workflows").items():
            for job in w.get("jobs", {}).values():
                for step in [job, *job.get("steps", [])]:
                    uses = step.get("uses", "")
                    if not uses.startswith("$/"):
                        continue
                    rel = uses[2:]
                    target = root / rel if rel.endswith(".yml") else root / rel / "action.yml"
                    if ".." in rel or "@" in rel or not target.exists():
                        raise ValueError("Invalid self reference: " + uses)
                    step["uses"] = "./" + rel
            path = Path(temp) / file
            path.write_text(yaml.safe_dump(w, sort_keys=False))
            paths.append(str(path))
        subprocess.run(["actionlint", "-config-file", str(root / ".github/actionlint.yaml"), *paths], check=True, cwd=root)
