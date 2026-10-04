from pathlib import Path
import unittest

from ios_release.policy import validate
from ios_release.yamlio import workflows

ROOT = Path(__file__).resolve().parents[1]


class PolicyTests(unittest.TestCase):
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
