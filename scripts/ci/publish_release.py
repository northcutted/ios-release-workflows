#!/usr/bin/env python3
"""Publish a complete draft; reject conflicting retries instead of replacing assets."""
import json
import os
from pathlib import Path
import subprocess

from evidence import CONFIG, digest, require, validate_manifest

def gh(*args, data=None):
    return subprocess.check_output(["gh", *args], text=True, input=json.dumps(data) if data is not None else None)


def find_release(repository, tag):
    # The tag endpoint returns published releases only. Include authenticated
    # drafts, paginate, and reject ambiguous operations instead of guessing.
    pages = json.loads(gh('api', f'repos/{repository}/releases?per_page=100', '--paginate', '--slurp'))
    matches = [release for page in pages for release in page if release['tag_name'] == tag]
    require(len(matches) <= 1, 'Multiple releases claim this tag; reconcile the recorded operations')
    return matches[0] if matches else None


def ensure_tag(repository, tag, source):
    existing = subprocess.run(['gh', 'api', f'repos/{repository}/git/ref/tags/{tag}'], text=True, capture_output=True)
    if existing.returncode:
        require('404' in existing.stderr, 'Cannot inspect release tag')
        gh('api', f'repos/{repository}/git/refs', '--method', 'POST', '--input', '-',
           data={'ref': 'refs/tags/' + tag, 'sha': source})
    require(tag_commit(repository, tag) == source, 'Release tag target conflicts with candidate')


def tag_commit(repository, tag):
    # Peel annotated tags too; target_commitish is only a release creation hint.
    reference = json.loads(gh("api", f"repos/{repository}/git/ref/tags/{tag}"))["object"]
    seen = set()
    while reference["type"] == "tag":
        require(reference["sha"] not in seen, "Cyclic tag object")
        seen.add(reference["sha"])
        reference = json.loads(gh("api", f"repos/{repository}/git/tags/{reference['sha']}"))["object"]
    require(reference["type"] == "commit", "Tag does not identify a commit")
    return reference["sha"]


def publish(root):
    root = Path(root)
    manifest = validate_manifest(root, "release-manifest.json", final=True)
    repository, tag = CONFIG["repository"], manifest["tag"]
    settings = json.loads(gh("api", f"repos/{repository}/immutable-releases"))
    require(settings.get("enabled") is True, "Enable immutable releases before creating or publishing a draft")
    marker = '<!-- ios-release-handoff-sha256:' + digest(root / 'release-manifest.json') + ' -->'
    release = find_release(repository, tag)
    if release and release['draft']:
        require(marker in release.get('body', ''), 'Draft belongs to another signed handoff')
    # A new draft does not create its eventual tag. Create and peel it explicitly
    # before attaching evidence; never move a pre-existing conflicting ref.
    ensure_tag(repository, tag, manifest['source_sha'])
    if not release:
        release = json.loads(gh('api', f'repos/{repository}/releases', '--method', 'POST', '--input', '-',
            data={'tag_name': tag, 'draft': True, 'prerelease': False, 'name': tag,
                  'body': (root / 'release-notes.md').read_text() + '\n\n' + marker}))
    require(isinstance(release.get('id'), int) and release.get('tag_name') == tag, 'Invalid release operation identity')
    endpoint = f"repos/{repository}/releases/{release['id']}"
    print('Release operation ID: ' + str(release['id']))
    require(tag_commit(repository, tag) == manifest["source_sha"], "Release tag target conflicts with candidate")
    remote = {a["name"]: a for a in release["assets"]}
    require(len(remote) == len(release['assets']), 'Duplicate release assets')
    allowed = {a["name"] for a in manifest["artifacts"]} | {"release-manifest.json", "release-attestation.jsonl"}
    expected = {name: root / name for name in allowed}
    require(not set(remote) - set(expected), "Unexpected assets already attached to release")
    for name, record in remote.items():
        require(record.get("digest") == "sha256:" + digest(expected[name]), f"Conflicting existing release asset: {name}")
    for name, path in sorted(expected.items()):
        if name in remote:
            require(remote[name].get("digest") == "sha256:" + digest(path), f"Conflicting existing release asset: {name}")
        else:
            require(release["draft"], "Cannot add assets to a published release")
            gh("release", "upload", tag, str(path), "--repo", repository)
    readback = json.loads(gh("api", endpoint))
    require({a["name"]: a.get("digest") for a in readback["assets"]} == {name: "sha256:" + digest(path) for name, path in expected.items()}, "Release asset readback mismatch")
    if release["draft"]:
        require(tag_commit(repository, tag) == manifest['source_sha'], 'Release tag changed before publication')
        gh('api', endpoint, '--method', 'PATCH', '--input', '-', data={'draft': False})
    published = json.loads(gh("api", endpoint))
    require(published.get("immutable") is True and not published["draft"], "Repository immutable releases must be enabled before publishing")
    require({a['name']: a.get('digest') for a in published['assets']} ==
            {name: 'sha256:' + digest(path) for name, path in expected.items()}, 'Published release asset mismatch')
    require(tag_commit(repository, tag) == manifest['source_sha'], 'Published release tag changed')
    print(published["html_url"])


if __name__ == "__main__":
    publish("release-assets")
