"""Deterministic, offline documentation from checked-in contracts."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote

from . import yamlio

PROFILE = ".github/ios-release-docs.json"


def read(root, file):
    return json.loads((Path(root) / file).read_text())


def profile(root):
    data = read(root, PROFILE)
    if data["schema_version"] != 1:
        raise ValueError("Unsupported documentation profile")
    for value in [data["output_dir"], *data["pages"], *(link["path"] for link in data["navigation"]),
                  *([data["classifier"]] if data.get("classifier") else [])]:
        if not value or Path(value).is_absolute() or ".." in Path(value).parts:
            raise ValueError("Documentation paths must stay inside the repository")
    return data


def array(value):
    return [] if value is None else value if isinstance(value, list) else [value]


def pick(value, keys):
    return {key: value[key] for key in keys if key in value}


def collect(root):
    root = Path(root)
    settings = profile(root)
    config = None if settings["mode"] == "platform" else read(root, ".github/ios-release.json")
    platform = read(root, ".github/ios-release-platform.json") if config else {"repository": settings["repository"]}
    items = []
    for file, workflow in yamlio.workflows(root / ".github/workflows").items():
        if not workflow.get("name") or not workflow.get("on") or not workflow.get("jobs"):
            raise ValueError(f"Incomplete workflow: {file}")
        events = workflow["on"]
        if isinstance(events, (str, list)):
            events = {event: None for event in array(events)}
        call = events.get("workflow_call") or {}
        dispatch = events.get("workflow_dispatch") or {}
        items.append({
            "file": f".github/workflows/{file}", "name": workflow["name"],
            "triggers": {event: {} if event in {"workflow_dispatch", "workflow_call"} else value for event, value in events.items()},
            "inputs": call.get("inputs", dispatch.get("inputs", {})),
            "secret_names": sorted(call.get("secrets", {})), "outputs": call.get("outputs", {}),
            "permissions": workflow.get("permissions"), "concurrency": workflow.get("concurrency"),
            "jobs": [{"id": name, "name": job.get("name", name), "needs": array(job.get("needs")),
                      "condition": job.get("if"), "runner": job.get("runs-on"), "uses": job.get("uses"),
                      "environment": job.get("environment"), "timeout_minutes": job.get("timeout-minutes"),
                      "permissions": job.get("permissions"),
                      "secret_names": sorted(job["secrets"]) if isinstance(job.get("secrets"), dict) else []}
                     for name, job in workflow["jobs"].items()],
        })
    examples = []
    if settings.get("classifier"):
        classifier = root / settings["classifier"]
        if classifier.suffix != ".py":
            raise ValueError("Documentation classifier must use Python")
        examples = json.loads(subprocess.check_output([sys.executable, str(classifier), "examples",
                              json.dumps(settings["change_examples"])], cwd=root, text=True))
    from .cli import command_contract
    ruby = root / ".ruby-version"
    return {
        "schema_version": 2, "scope": "Checked-in workflow contracts; excludes live service state and step scripts.",
        "sources": [PROFILE, *([".github/ios-release.json", ".github/ios-release-platform.json"] if config else []),
                    *([".ruby-version"] if ruby.exists() else []),
                    *([settings["classifier"]] if settings.get("classifier") else []),
                    *([] if config else ["src/ios_release/cli.py"]), *(item["file"] for item in items)],
        "platform": platform, "settings": settings,
        "configuration": {
            **pick(config, ["repository", "default_branch", "project", "workspace", "scheme", "xcode", "compatibility",
                            "test_device", "test_targets", "qa_checks", "upload_adapter", "metadata_path", "screenshots_path",
                            "locales", "localization_locales", "screenshot_devices", "screens", "replacement_release"]),
            "app_store": pick(config["app_store"], ["release_type", "phased_release", "testflight_groups"]),
            "developer_ruby": ruby.read_text().strip() if ruby.exists() else None,
            "expected_screenshots": len(config["locales"]) * len(config["screenshot_classes"]) * len(config["screens"]),
        } if config else None,
        "change_examples": examples, "commands": None if config else command_contract(), "workflows": items,
    }


def escape(value):
    if not isinstance(value, str):
        value = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    for before, after in [("&", "&amp;"), ("<", "&lt;"), (">", "&gt;"), ("|", "&#124;"), ("`", "&#96;"), ("\n", " ")]:
        value = value.replace(before, after)
    return value


def code(value):
    return f"<code>{escape(value)}</code>"


def table(headings, rows):
    return "\n".join("| " + " | ".join(row) + " |" for row in [headings, ["---"] * len(headings), *rows])


def anchor(value):
    return re.sub(r"\s", "-", re.sub(r"[^\w\-\s]", "", value.lower()))


def render_markdown(data):
    config, settings = data["configuration"], data["settings"]
    local = lambda file: os.path.relpath(file, settings["output_dir"])
    nav = " · ".join(f'[{link["label"]}]({local(link["path"])})' for link in settings["navigation"])
    lines = ["# CI/CD reference", "", "<!-- Generated by the pinned ios-release platform. Edit sources, then run the documented docs command. -->", "", nav, "",
             "Generated from checked-in workflow interfaces and configuration. This reference records declared contracts; inspect live services for current state. Step scripts and secret values are omitted.", "",
             f'[Machine-readable index](reference.json) (schema {data["schema_version"]}). Regenerate with `make docs`; verify with `make check-docs`.', ""]
    platform = data["platform"]
    if platform.get("revision"):
        lines += ["## Platform guides", ""] + [f'[{name.title()}](https://github.com/{platform["repository"]}/blob/{platform["revision"]}/docs/{name}.md)' for name in ["setup", "operations", "architecture", "maintenance"]] + [""]
    lines += ["## Platform and configuration", "", f'Platform: [{platform["repository"]} at {platform["revision"][:12]}](https://github.com/{platform["repository"]}/tree/{platform["revision"]}).' if platform.get("revision") else f'Platform: {platform["repository"]}.', ""]
    if config:
        toolchain = lambda name: " / ".join(code(config[name][key]) for key in ["version", "build", "runtime", "sdk"])
        store, replacement = config["app_store"], config.get("replacement_release")
        lines += [f'Source: [app configuration]({local(".github/ios-release.json")}), [platform pin]({local(".github/ios-release-platform.json")}).', "",
                  table(["Setting", "Checked-in value"], [
                      ["Project / scheme", f'{code(config.get("workspace") or config["project"])} / {code(config["scheme"])}'],
                      ["Primary Xcode / build / simulator / SDK", toolchain("xcode")], ["Compatibility Xcode / build / simulator / SDK", toolchain("compatibility")],
                      ["Unit test device", escape(config["test_device"])], ["Developer Ruby", code(config["developer_ruby"])],
                      ["Release QA checks", ", ".join(map(code, config["qa_checks"]))], ["Upload adapter", code(config["upload_adapter"])],
                      ["Store release policy", f'{code(store["release_type"])}; phased release: {code(store["phased_release"])}'],
                      ["Automatic TestFlight groups", ", ".join(map(escape, store["testflight_groups"])) or "None configured"],
                      ["Replacement override", f'{code(replacement["version"])} replaces recorded build {code(replacement["build_number"])}' if replacement else "None"],
                      ["Store locales", f'{len(config["locales"])}: ' + ", ".join(map(code, config["locales"]))],
                      ["App translations", f'{len(config["localization_locales"])} in addition to the source language'],
                      ["Screenshot devices", ", ".join(map(escape, config["screenshot_devices"]))],
                      ["Screenshot inventory", f'{config["expected_screenshots"]} images; {len(config["screens"])} scenes per device class and store locale'],
                      ["Screenshot scenes", ", ".join(map(code, config["screens"]))],
                  ]), ""]
        if data["change_examples"]:
            lines += ["## Which checks run?", "", "Single-path examples evaluated by the consumer classifier. Mixed changes and fallback behavior follow that source.", "",
                      table(["Changed path", *settings["example_flags"].values()], [[code(item["path"]), *("Yes" if item[key] else "—" for key in settings["example_flags"])] for item in data["change_examples"]]), ""]
    if data["commands"]:
        lines += ["## Commands", "", table(["Command", "Purpose"], [[code(name), escape(command["description"])] for name, command in data["commands"]["commands"].items()]), ""]
    lines += ["## Workflows", "", table(["Workflow", "File", "Events"], [[f'[{escape(w["name"])}](#{anchor(w["name"])})', f'[{Path(w["file"]).name}]({local(w["file"])})', ", ".join(map(code, w["triggers"]))] for w in data["workflows"]]), "",
              "Jobs below belong to the checked-in workflows. A linked reusable workflow expands into its own jobs. A dash means no explicit override; GitHub dependency/default behavior still applies. Conditions are shown verbatim, not evaluated here.", ""]
    for w in data["workflows"]:
        repo = config["repository"] if config else platform["repository"]
        lines += [f'### {w["name"]}', "", f'[Source]({local(w["file"])}) · [Actions](https://github.com/{repo}/actions/workflows/{Path(w["file"]).name})', "",
                  "Triggers (cron expressions use UTC):", "", "```json", json.dumps(w["triggers"], indent=2, ensure_ascii=False), "```", "",
                  f'Concurrency: {code(w["concurrency"])}. Default token permissions: {code(w["permissions"])}.', ""]
        if w["inputs"]:
            lines += [table(["Input", "Type", "Required", "Default", "Description / choices"], [
                [code(name), code(item.get("type", "string")), "Yes" if item.get("required") else "No",
                 code('"" (blank)' if item["default"] == "" else item["default"]) if "default" in item else "—",
                 "<br>".join(filter(None, [escape(item.get("description", "")), ", ".join(map(code, item.get("options", [])))]))]
                for name, item in w["inputs"].items()]), ""]
        if w["secret_names"]:
            lines += ["Named secrets: " + ", ".join(map(code, w["secret_names"])) + ".", ""]
        if w["outputs"]:
            lines += ["Outputs:", "", "```json", json.dumps(w["outputs"], indent=2, ensure_ascii=False), "```", ""]
        rows = []
        for job in w["jobs"]:
            uses = job["uses"]
            if uses and uses.startswith("$/"):
                execution = f'[{uses[2:]}]({local(uses[2:])})'
            elif uses:
                target, revision = uses.split("@")
                owner, name, *file = target.split("/")
                execution = f'[{file[-1]}](https://github.com/{owner}/{name}/blob/{revision}/{"/".join(file)})'
            else:
                execution = f'{code(job["runner"])}; {job["timeout_minutes"] if job["timeout_minutes"] is not None else "default"} min'
                if job["environment"]:
                    execution += f'<br>Environment: {code(job["environment"])}'
            rows.append([code(job["id"]) + (f'<br>{escape(job["name"])}' if job["name"] != job["id"] else ""),
                         ", ".join(map(code, job["needs"])) or "—", execution, "—" if job["condition"] is None else code(job["condition"])])
        lines += [table(["Job", "Needs", "Execution", "Condition"], rows), ""]
    return "\n".join(lines)


def generated_files(root):
    data = collect(root)
    output = data["settings"]["output_dir"]
    return {f"{output}/reference.json": json.dumps(data, indent=2, ensure_ascii=False) + "\n",
            f"{output}/reference.md": render_markdown(data)}


def without_fences(text):
    return re.sub(r"^(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\1\s*$", "", text, flags=re.M)


def check_links(root):
    root, errors = Path(root), []
    pages = []
    for page in profile(root)["pages"]:
        path = root / page
        pages.extend(sorted(path.rglob("*.md")) if path.is_dir() else [path])
    for file in pages:
        for href in re.findall(r"\[[^\]\n]*\]\(([^\s)]+)\)", without_fences(file.read_text())):
            if re.match(r"^[a-z][a-z0-9+.-]*:", href, re.I) or href.startswith("//"):
                continue
            relative, _, fragment = href.partition("#")
            target = (file.parent / unquote(relative)).resolve() if relative else file
            if not target.exists():
                errors.append(f"{file.relative_to(root)}: missing link {href}")
            elif fragment and target.suffix == ".md":
                headings = [anchor(value) for value in re.findall(r"^#{1,6}\s+(.+)$", without_fences(target.read_text()), flags=re.M)]
                if unquote(fragment) not in headings:
                    errors.append(f"{file.relative_to(root)}: missing heading {href}")
    return errors


def run(root, check=False):
    errors = []
    for file, contents in generated_files(root).items():
        path = Path(root) / file
        if check:
            if not path.exists() or path.read_text() != contents:
                errors.append(f"{file} is stale; regenerate with the documented docs command")
        elif not path.exists() or path.read_text() != contents:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents)
    return errors + check_links(root)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    errors = run(os.environ.get("IOS_APP_ROOT", os.getcwd()), args.check)
    if errors:
        raise ValueError("\n".join(errors))
    print("CI/CD reference is current; local documentation links passed." if args.check else "CI/CD reference generated; local documentation links passed.")
    return 0
