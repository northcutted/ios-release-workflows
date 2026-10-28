from pathlib import Path
import json
import os
import subprocess
import tempfile
import unittest

from ios_release.policy import validate
from ios_release.yamlio import workflows

ROOT = Path(__file__).resolve().parents[1]


class PolicyTests(unittest.TestCase):
    def test_picstrip_source_capture_sets_the_consumer_identity_in_the_child_process(self):
        job = workflows(ROOT / ".github/workflows")["rust-checks.yml"]["jobs"]["store-inputs"]
        checkout = next(s["with"] for s in job["steps"] if s.get("with", {}).get("path") == "consumer")
        step = next(s for s in job["steps"] if "release source" in s.get("run", ""))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "platform/rust/target/release/ios-release"
            binary.parent.mkdir(parents=True)
            binary.write_text("#!/usr/bin/env python3\nimport json, os, sys\nprint(json.dumps({'repository': os.environ['GITHUB_REPOSITORY'], 'source': os.environ['SOURCE_SHA'], 'args': sys.argv[1:]}))\n")
            binary.chmod(0o755)
            output = root / "consumer/build/native-readiness/source-snapshot.json"
            output.parent.mkdir(parents=True)
            # Actions supplies reserved defaults after YAML env. Exercise the actual
            # shell command, not a Python subprocess environment that masks this rule.
            env = {**os.environ, **step.get("env", {}), "GITHUB_REPOSITORY": "northcutted/ios-release-workflows"}
            subprocess.run(["bash", "-e", "-c", step["run"]], cwd=root, env=env, check=True, capture_output=True)
            actual = json.loads(output.read_text())
            self.assertEqual(actual["repository"], checkout["repository"])
            self.assertEqual(actual["source"], checkout["ref"])
            self.assertEqual(actual["args"], ["--app-root", "consumer", "release", "source"])

    def test_durable_records_preserve_publisher_boundary_and_interrupted_receipts(self):
        for change in ["environment", "token", "authority", "artifact", "apple", "interrupted", "bundle", "verifier"]:
            w = workflows(ROOT / ".github/workflows")
            jobs = w["native-app.yml"]["jobs"]
            job = jobs["record_receipt"]
            if change == "environment": job["environment"] = "native-app-store"
            elif change == "token":
                next(s for s in job["steps"] if s.get("id") == "publisher")["with"]["repositories"] = ""
            elif change == "authority": job["permissions"]["contents"] = "write"
            elif change == "artifact":
                next(s for s in job["steps"] if "artifact-ids" in s.get("with", {}))["with"] = {"name":"latest"}
            elif change == "apple": job["env"]["IOS_RELEASE_API_KEY_ID"] = "${{ secrets.IOS_RELEASE_API_KEY_ID }}"
            elif change == "interrupted": job["if"] = "inputs.durable_records"
            elif change == "bundle":
                next(s for s in jobs["receipt"]["steps"] if s.get("id") == "receipt_artifact")["with"]["path"] = "receipt/operation.json"
            else: job["steps"] = [s for s in job["steps"] if s.get("uses") != "$/actions/slsa-verifier"]
            self.assertTrue(validate(w), change)

    def test_native_provenance_cannot_skip_verification_publish_or_lose_proofs(self):
        for change in ["generator", "publish", "continue", "dependency", "verify", "artifact", "bundle", "installer"]:
            w = workflows(ROOT / ".github/workflows")
            jobs = w["native-app.yml"]["jobs"]
            if change == "generator": jobs["provenance"]["uses"] = "$/ .github/workflows/prepare.yml"
            elif change == "publish": jobs["provenance"]["with"]["upload-assets"] = True
            elif change == "continue": jobs["provenance"]["with"]["continue-on-error"] = True
            elif change == "dependency": jobs["preparation"]["needs"] = ["seal"]
            elif change == "verify":
                for step in jobs["preparation"]["steps"]:
                    if step.get("run") == "ios-release github verify-preparation": step["run"] = "true"
            elif change == "artifact":
                step = next(s for s in jobs["preparation"]["steps"] if "artifact-ids" in s.get("with", {}))
                step["with"] = {"name":"latest"}
            elif change == "bundle":
                step = next(s for s in jobs["preparation"]["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
                step["with"]["path"] = "build/native-preparation/native-release.zip"
            else:
                jobs["select"]["steps"] = [s for s in jobs["select"]["steps"] if s.get("uses") != "$/actions/slsa-verifier"]
            self.assertTrue(validate(w), change)

    def test_native_preparation_cannot_omit_bound_sboms(self):
        for omitted in ["install_syft.sh", "ios-release release sbom", "--require-sbom"]:
            w = workflows(ROOT / ".github/workflows")
            for step in w["native-app.yml"]["jobs"]["seal"]["steps"]:
                if "run" in step:
                    step["run"] = step["run"].replace(omitted, "omitted")
            self.assertIn("require source/runtime SBOMs", "\n".join(validate(w)))

    def test_native_source_snapshot_cannot_be_omitted_recreated_after_build_or_unchecked(self):
        for change in ["omit", "late", "credentials", "optional", "verify", "seal", "build-upload", "verify-upload"]:
            w = workflows(ROOT / ".github/workflows")
            jobs = w["native-app.yml"]["jobs"]
            steps = jobs["build"]["steps"]
            capture = next(s for s in steps if s.get("run") == "ios-release release source")
            if change == "omit": steps.remove(capture)
            elif change == "late": steps.remove(capture); steps.append(capture)
            elif change == "credentials": capture["env"] = {"IOS_RELEASE_SIGNING_PASSWORD": "secret"}
            elif change == "optional": capture["continue-on-error"] = True
            elif change in ["verify", "seal"]:
                for step in jobs[change]["steps"]:
                    if "run" in step: step["run"] = step["run"].replace("--require-source", "")
            else:
                upload = next(s["with"] for s in jobs[change.split("-")[0]]["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
                upload["path"] = upload["path"].replace("${{ inputs.app_root }}/build/source-snapshot/", "")
            self.assertIn("source snapshot", "\n".join(validate(w)), change)

    def test_native_rehearsal_cannot_receive_secrets_or_sign_provenance(self):
        for field, value in [("environment", "native-signing"), ("permissions", {"id-token": "write"})]:
            w = workflows(ROOT / ".github/workflows")
            w["native-readiness.yml"]["jobs"]["qa"][field] = value
            self.assertIn("PR path must not receive", "\n".join(validate(w)))

    def test_native_preparation_requires_real_store_assets_and_symbols(self):
        w = workflows(ROOT / ".github/workflows")
        jobs = w["native-app.yml"]["jobs"]
        jobs["seal"]["steps"][0]["with"].pop("lfs")
        upload = next(s for s in jobs["verify"]["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
        upload["with"]["path"] = "build/application.ipa"
        errors = "\n".join(validate(w))
        self.assertIn("requires LFS", errors)
        self.assertIn("retain verified symbols", errors)

    def test_native_retries_cannot_replace_exact_upstream_artifacts_with_names(self):
        w = workflows(ROOT / ".github/workflows")
        jobs = w["native-app.yml"]["jobs"]
        jobs["signing"]["outputs"]["artifact"] = "latest"
        download = next(s for s in jobs["qa"]["steps"] if s.get("uses", "").startswith("actions/download-artifact@"))
        download["with"] = {"name": "native-signing"}
        errors = "\n".join(validate(w))
        self.assertIn("preserve attempts", errors)
        self.assertIn("exact upstream artifact IDs", errors)
    def test_native_release_preserves_credential_and_approval_boundaries(self):
        w = workflows(ROOT / ".github/workflows")
        jobs = w["native-app.yml"]["jobs"]
        jobs["qa"]["environment"] = "native-signing-admin"
        jobs["build"]["env"]["IOS_RELEASE_API_KEY_ID"] = "${{ secrets.IOS_RELEASE_API_KEY_ID }}"
        jobs["seal"]["steps"].append({"run": "ios-release release build"})
        jobs["production"]["environment"] = "native-app-store"
        jobs["deliver"]["if"] = "always()"
        errors = "\n".join(validate(w))
        for pattern in ["secret-free", "Native build", "provenance must not execute", "owner approval", "may not perform production"]:
            self.assertIn(pattern, errors)
    def test_declared_boundaries(self):
        self.assertEqual(validate(workflows(ROOT / ".github/workflows")), [])

    def test_reject_approval_bypass_secrets_mutable_actions_and_oidc(self):
        w = workflows(ROOT / ".github/workflows")
        w["deploy.yml"]["jobs"]["review"]["environment"] = "app-store-staging"
        w["ci.yml"]["jobs"]["lint"]["steps"].append({"run": "echo bad", "env": {"KEY": "${{ secrets.KEY }}"}})
        w["prepare.yml"]["jobs"]["build"]["steps"][0]["uses"] = "actions/checkout@main"
        w["prepare.yml"]["jobs"]["build"]["permissions"] = {"id-token": "write"}
        errors = "\n".join(validate(w))
        for pattern in ["production approval", "PR path", "action must be pinned", "compilation must not sign"]:
            self.assertIn(pattern, errors)

    def test_missing_secret_contract_and_unverified_selection(self):
        w = workflows(ROOT / ".github/workflows")
        del w["prepare.yml"]["on"]["workflow_call"]["secrets"]["MATCH_SSH_PRIVATE_KEY"]
        jobs = w["release.yml"]["jobs"]
        jobs["promote"]["needs"] = []
        jobs["promote"]["with"].update(artifact_id="latest", upload_adapter="transporter")
        jobs["resolve"]["environment"] = "testflight"
        errors = "\n".join(validate(w))
        for pattern in ["explicit reusable contract", "verified selection", "frozen verified artifact", "read-only and secret-free", "verified candidate configuration"]:
            self.assertIn(pattern, errors)

    def test_native_qa_cannot_load_ruby_or_sign_provenance(self):
        w = workflows(ROOT / ".github/workflows")
        w["ci.yml"]["jobs"]["test"]["permissions"] = {"id-token": "write"}
        w["ci.yml"]["jobs"]["analyze"]["steps"].append({"uses": "$/actions/ruby"})
        errors = "\n".join(validate(w))
        self.assertIn("compilation must not sign", errors)
        self.assertIn("Native QA must not load", errors)
