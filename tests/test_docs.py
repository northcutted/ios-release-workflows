import json
from pathlib import Path
import shutil
import tempfile
import unittest

from ios_release.docs import collect, generated_files, run, check_links

ROOT = Path(__file__).resolve().parents[1]


class DocsTests(unittest.TestCase):
    def fixture(self, name="minimal"):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / ".github/workflows").mkdir(parents=True)
        (root / "guide").mkdir()
        shutil.copy(ROOT / f"examples/{name}.json", root / ".github/ios-release.json")
        (root / ".github/ios-release-platform.json").write_text(json.dumps({"repository": "northcutted/ios-release-workflows", "revision": "a" * 40}))
        (root / ".github/ios-release-docs.json").write_text(json.dumps({"schema_version": 1, "mode": "consumer", "output_dir": "guide", "pages": ["guide"], "navigation": [{"label": "Guide", "path": "guide/index.md"}]}))
        (root / "guide/index.md").write_text("# Guide\n\n## Start\n")
        (root / ".github/workflows/check.yml").write_text("name: Check\non: push\njobs:\n  test:\n    runs-on: macos-26\n")
        return root

    def test_unrelated_consumers_need_no_npm_classifier_or_ruby(self):
        for name in ["minimal", "extensions"]:
            root = self.fixture(name)
            self.assertEqual(run(root), [])
            before = generated_files(root)
            self.assertEqual(generated_files(root), before)
            self.assertEqual(run(root, True), [])
            self.assertNotIn("PicStrip", before["guide/reference.json"])
            with (root / ".github/workflows/check.yml").open("a") as file:
                file.write("    timeout-minutes: 15\n")
            self.assertEqual(len([error for error in run(root, True) if "is stale" in error]), 2)
            for file, text in before.items():
                self.assertEqual((root / file).read_text(), text)
            self.assertEqual(run(root), [])

    def test_contracts_keep_false_defaults_and_omit_step_code(self):
        root = self.fixture()
        (root / ".github/workflows/example.yaml").write_text("""name: Example
on:
  workflow_call:
    inputs:
      submit:
        description: Stage | submit
        type: boolean
        default: false
    secrets:
      APPLE_KEY:
        required: true
    outputs:
      receipt:
        value: ${{ jobs.first.outputs.receipt }}
jobs:
  first:
    runs-on: ubuntu-24.04
    steps:
      - run: echo do-not-copy-step-code
  second:
    needs: first
    runs-on: ubuntu-24.04
    if: inputs.submit
""")
        workflow = next(w for w in collect(root)["workflows"] if w["name"] == "Example")
        self.assertIs(workflow["inputs"]["submit"]["default"], False)
        self.assertEqual(workflow["secret_names"], ["APPLE_KEY"])
        self.assertEqual(workflow["jobs"][1]["needs"], ["first"])
        files = generated_files(root)
        self.assertNotIn("do-not-copy-step-code", files["guide/reference.json"])
        self.assertIn("Stage &#124; submit", files["guide/reference.md"])

    def test_workspace_inventory_and_duplicate_yaml(self):
        root = self.fixture()
        path = root / ".github/ios-release.json"
        config = json.loads(path.read_text())
        config["workspace"] = "Different.xcworkspace"
        config.pop("project")
        path.write_text(json.dumps(config))
        self.assertIn("Different.xcworkspace", generated_files(root)["guide/reference.md"])
        (root / ".github/workflows/broken.yml").write_text("name: First\nname: Second\non: push\njobs: {}\n")
        with self.assertRaisesRegex(ValueError, "unique"):
            collect(root)

    def test_links_ignore_fences(self):
        root = self.fixture()
        (root / "guide/nested").mkdir()
        (root / "guide/nested/page.md").write_text("# Page\n\n[Missing](missing.md)\n[Heading](../index.md#missing)\n[Good](../index.md#start)\n[External](https://example.invalid/)\n\n```md\n[Example](missing-example.md)\n```\n")
        self.assertEqual(len(check_links(root)), 2)

    def test_platform_interfaces(self):
        data = collect(ROOT)
        self.assertIsNone(data["configuration"])
        self.assertIn("qa", data["commands"]["commands"])
        self.assertIn("source", next(w for w in data["workflows"] if w["file"].endswith("/ci.yml"))["inputs"])
