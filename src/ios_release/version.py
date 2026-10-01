"""Read-only Conventional Commit version decisions and release notes."""
import argparse
from datetime import date
import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess

STABLE = re.compile(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$")
RANK = {False: 0, None: 0, "patch": 1, "minor": 2, "major": 3}
DEFAULTS = [{"breaking": True, "release": "major"}, {"revert": True, "release": "patch"},
            {"type": "feat", "release": "minor"}, {"type": "fix", "release": "patch"}, {"type": "perf", "release": "patch"}]


def policy(root):
    path = Path(root) / ".github/ios-version.json"
    if path.exists():
        data = json.loads(path.read_text())
        if data.get("schema_version") != 1 or set(data) != {"schema_version", "release_rules", "note_types"}:
            raise ValueError("Unsupported version policy")
    else:
        # Compatibility for consumers upgrading from the Node toolkit.
        old = json.loads((Path(root) / ".releaserc.json").read_text())
        plugins = dict(old["plugins"])
        if set(plugins) != {"@semantic-release/commit-analyzer", "@semantic-release/release-notes-generator"}:
            raise ValueError("Unsupported legacy release plugins; use .github/ios-version.json")
        analyzer, writer = plugins["@semantic-release/commit-analyzer"], plugins["@semantic-release/release-notes-generator"]
        if analyzer.get("preset") != "conventionalcommits" or writer.get("preset") != "conventionalcommits" or set(analyzer) - {"preset", "releaseRules"} or set(writer) - {"preset", "presetConfig"} or set(writer.get("presetConfig", {})) - {"types"}:
            raise ValueError("Unsupported legacy release options; use .github/ios-version.json")
        data = {"schema_version": 1, "release_rules": analyzer.get("releaseRules", []), "note_types": writer["presetConfig"]["types"]}
    for rule in data["release_rules"]:
        if set(rule) - {"type", "scope", "breaking", "revert", "release"} or rule.get("release") not in RANK:
            raise ValueError("Unsupported release rule")
    for entry in data["note_types"]:
        if set(entry) - {"type", "scope", "section", "hidden", "effect"} or "type" not in entry:
            raise ValueError("Unsupported release note type")
    return data


def parse_commit(commit):
    message = commit["message"]
    header, _, body = message.partition("\n")
    parsed = {**commit, "header": header, "body": body, "type": "", "scope": "", "subject": header, "notes": [], "revert": False}
    match = re.fullmatch(r"(\w*)(?:\((.*)\))?(!)?: (.*)", header)
    if match:
        parsed.update(type=match[1], scope=match[2] or "", subject=match[4])
    for note in re.finditer(r"^BREAKING[ -]CHANGE:\s*(.*(?:\n(?![A-Za-z-]+:|(?:Closes|Fixes|Resolves)\s+#).*)*)", body, flags=re.M):
        parsed["notes"].append(note[1].strip())
    if match and match[3] and not parsed["notes"]:
        parsed["notes"].append(parsed["subject"])
    parsed["breaking"] = bool(parsed["notes"])
    reverted = re.search(r'^(?:Revert|revert:)\s"?([\s\S]+?)"?\s*This reverts commit (\w*)\.', message, re.I)
    if reverted:
        parsed["revert"] = True
        parsed["reverted_hash"] = reverted[2]
        parsed["reverted_header"] = reverted[1].rstrip('"')
    return parsed


def filter_reverts(commits):
    removed = set()
    for i, commit in enumerate(commits):
        if i in removed or not commit["revert"]:
            continue
        for j, original in enumerate(commits):
            if j != i and j not in removed and original["hash"].startswith(commit["reverted_hash"]) and original["header"] == commit["reverted_header"]:
                removed.update([i, j])
                break
    return [commit for i, commit in enumerate(commits) if i not in removed]


def matches(rule, commit):
    for key, value in rule.items():
        if key == "release":
            continue
        if key in {"breaking", "revert"}:
            if bool(commit[key]) != value:
                return False
        elif not fnmatch.fnmatchcase(commit[key], value):
            return False
    return True


def release_type(commits, rules):
    release = None
    for commit in commits:
        selected = [rule["release"] for rule in rules if matches(rule, commit)]
        if not selected:
            selected = [rule["release"] for rule in DEFAULTS if matches(rule, commit)]
        if selected:
            current = max(selected, key=RANK.__getitem__)
            if RANK[current] > RANK[release]:
                release = current
    return release


def bump(version, release):
    major, minor, patch = map(int, version.split("."))
    if release == "major":
        return f"{major + 1}.0.0"
    if release == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def notes(commits, types, version, tag, source, previous, repo):
    base = "https://github.com/" + repo
    target = source if tag is None else tag
    heading = f"[{version}]({base}/compare/{previous}...{target})" if previous else version
    sections = [f"## {heading} ({date.today().isoformat()})"]
    breaking, groups = [], {}
    link_refs = lambda text: re.sub(r"(?<![\w/])#(\d+)", lambda m: f"[#{m[1]}]({base}/issues/{m[1]})", text)
    for commit in commits:
        entry = next((item for item in types if item["type"] == ("revert" if commit["revert"] else commit["type"]).lower() and (not item.get("scope") or item["scope"] == commit["scope"])), None)
        if not commit["notes"] and (entry is None or entry.get("effect", "bump") == "hidden"):
            continue
        scope = commit["scope"] if commit["scope"] != "*" else ""
        prefix = f"**{scope}:** " if scope else ""
        breaking.extend(prefix + link_refs(note) for note in commit["notes"])
        section = entry.get("section", "") if entry else commit["type"]
        line = f'* {prefix}{link_refs(commit["subject"])} ([{commit["hash"][:7]}]({base}/commit/{commit["hash"]}))'
        refs = re.findall(r"(?im)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)", commit["body"])
        refs = [issue for issue in dict.fromkeys(refs) if "#" + issue not in commit["subject"]]
        if refs:
            line += ", closes " + " ".join(f"[#{issue}]({base}/issues/{issue})" for issue in refs)
        groups.setdefault(section, []).append((scope, commit["subject"], line))
    if breaking:
        sections.append("### ⚠ BREAKING CHANGES\n\n" + "\n".join("* " + note.replace("\n", "\n  ") for note in sorted(breaking)))
    order = [item["section"] for item in types if item.get("section")]
    for section in sorted(groups, key=lambda value: order.index(value) if value in order else -1):
        lines = "\n".join(line for _, _, line in sorted(groups[section]))
        sections.append((f"### {section}\n\n" if section else "") + lines)
    return "\n\n".join(sections) + "\n"


def analyze(root, config_file=None):
    root = Path(root)
    def git(*args):
        return subprocess.check_output(["git", *args], cwd=root, text=True).strip()
    settings, source = policy(root), git("rev-parse", "HEAD")
    tags = [tag for tag in git("tag", "--merged", source).splitlines() if tag.startswith("v") and STABLE.fullmatch(tag[1:])]
    tag = max(tags, key=lambda value: tuple(map(int, value[1:].split(".")))) if tags else None
    hashes = git("rev-list", f"{tag}..{source}" if tag else source).splitlines()
    commits = filter_reverts([parse_commit({"hash": sha, "message": git("show", "-s", "--format=%B", sha)}) for sha in hashes])
    release = release_type(commits, settings["release_rules"])
    version = bump(tag[1:], release) if tag and release else tag[1:] if tag else "1.0.0"
    config_path = root / (config_file or os.environ.get("IOS_RELEASE_CONFIG", ".github/ios-release.json"))
    app = json.loads(config_path.read_text()) if config_path.exists() else {}
    replacement = app.get("replacement_release")
    if replacement and (not STABLE.fullmatch(replacement.get("version", "")) or replacement.get("source_tag") != "v" + replacement["version"] or tag != replacement["source_tag"]):
        raise ValueError("Replacement must target the highest reachable stable release")
    candidate = replacement["version"] if replacement else version
    candidate_tag = None if replacement else "v" + version
    rendered = notes(commits, settings["note_types"], candidate, candidate_tag, source, tag, os.environ.get("GITHUB_REPOSITORY", "example/fixture")) if release else "Verification build; no release changes."
    return {"will_release": bool(release) or bool(replacement), "version": candidate, "git_tag": candidate_tag,
            "replacement_for": replacement["source_tag"] if replacement else None, "source_sha": source, "notes": rendered}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", nargs="?", default="build/version/semantic-release.json")
    args = parser.parse_args(argv)
    result = analyze(os.environ["IOS_APP_ROOT"])
    path = Path(args.output)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    return 0
