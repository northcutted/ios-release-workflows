"""Data-only consumer configuration; never import code from an application checkout."""
import json
import os
from pathlib import Path
import re

def require(value, message):
    if not value:
        raise ValueError(message)

def load(path=None):
    path = Path(path or os.environ.get("IOS_RELEASE_CONFIG", ".github/ios-release.json"))
    data = json.loads(path.read_text())
    require(data.get("schema_version") == 1, "Unsupported app configuration schema")
    require(re.fullmatch(r"[\w.-]+/[\w.-]+", data.get("repository", "")), "Invalid consumer repository")
    if os.getenv("GITHUB_REPOSITORY"):
        require(data["repository"].lower() == os.environ["GITHUB_REPOSITORY"].lower(), "Configuration belongs to another repository")
    require(data.get("default_branch") == "main", "v1 requires protected main")
    require(bool(data.get("project")) != bool(data.get("workspace")), "Choose exactly one Xcode project or workspace")
    require(data.get("scheme") and data.get("test_targets"), "Scheme and test targets are required")
    require(re.fullmatch(r"git@github\.com:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\.git", data.get("signing_repository", "")), "v1 requires a GitHub SSH signing repository")
    for key in ("project", "workspace", "metadata_path", "screenshots_path", "accessibility_path", "localization_catalog"):
        if data.get(key):
            value = Path(data[key])
            require(not value.is_absolute() and ".." not in value.parts and "\n" not in str(value), f"Unsafe {key}")
    require(re.fullmatch(r"[A-Z0-9]{10}", data.get("team_id", "")), "Invalid Apple team")
    targets = data.get("targets", [])
    require(targets and len({t["bundle_id"] for t in targets}) == len(targets), "Expected unique application targets")
    for target in targets:
        require(re.fullmatch(r"[A-Za-z0-9.-]+", target["bundle_id"]), "Invalid bundle identifier")
        require(isinstance(target.get("entitlements"), dict), "Declare expected entitlements for every target")
        require(isinstance(target.get("non_exempt_encryption"), bool), "Declare encryption policy for every target")
        require(isinstance(target.get("tracking"), bool), "Declare privacy tracking policy for every target")
        require(target.get("profile"), "Declare a provisioning profile for every target")
    data["bundle_ids"] = [t["bundle_id"] for t in targets]
    require(data["app_store"]["bundle_id"] == data["bundle_ids"][0], "Main bundle must be first")
    require(data["app_store"]["release_type"] in ("MANUAL", "AFTER_APPROVAL"), "Unsupported release policy")
    require(isinstance(data["app_store"]["phased_release"], bool), "Declare phased release policy")
    require(data["upload_adapter"] in ("transporter", "build-uploads"), "Unknown upload adapter")
    require(data.get("qa_checks") == ["lint", "localization", "analyze", "test", "test-compatibility"], "Incomplete QA contract")
    require(data.get("runtime_dependencies") is not None, "Declare runtime dependencies, including an explicit empty list")
    require(data["source_workflow"] == ".github/workflows/main.yml", "v1 prepare entrypoint must be main.yml")
    return data

CONFIG = load()
