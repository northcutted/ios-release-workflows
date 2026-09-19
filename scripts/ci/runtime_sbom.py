#!/usr/bin/env python3
"""Add first-party bundle identities and embedded binary inventory to Syft's SPDX."""
import argparse
import datetime
import json
from pathlib import Path

from evidence import read, write, CONFIG

parser = argparse.ArgumentParser()
parser.add_argument("inventory")
parser.add_argument("syft")
parser.add_argument("output")
args = parser.parse_args()
inventory, document = read(args.inventory), read(args.syft)
document["name"] = "application.ipa"
document["documentNamespace"] = "https://github.com/" + CONFIG["repository"] + "/sbom/" + inventory["ipa_sha256"]
packages = document.setdefault("packages", [])
relationships = document.setdefault("relationships", [])
for index, app in enumerate(inventory["applications"]):
    spdx_id = f"SPDXRef-ApplicationBundle-{index}"
    packages.append({"SPDXID": spdx_id, "name": app["bundle_id"], "versionInfo": f"{app['version']}+{app['build_number']}",
                     "downloadLocation": "NOASSERTION", "filesAnalyzed": False,
                     "primaryPackagePurpose": "APPLICATION", "licenseConcluded": "NOASSERTION",
                     "copyrightText": "NOASSERTION", "checksums": [{"algorithm": "SHA256", "checksumValue": app["executable_sha256"]}]})
    relationships.append({"spdxElementId": "SPDXRef-DOCUMENT", "relatedSpdxElement": spdx_id, "relationshipType": "DESCRIBES"})
for index, component in enumerate(inventory["embedded_components"]):
    spdx_id = f"SPDXRef-Embedded-{index}"
    packages.append({"SPDXID": spdx_id, "name": component["path"], "downloadLocation": "NOASSERTION", "filesAnalyzed": False,
                     "primaryPackagePurpose": "LIBRARY", "licenseConcluded": "NOASSERTION", "copyrightText": "NOASSERTION",
                     "checksums": [{"algorithm": "SHA256", "checksumValue": component["sha256"]}]})
    relationships.append({"spdxElementId": "SPDXRef-ApplicationBundle-0", "relatedSpdxElement": spdx_id, "relationshipType": "CONTAINS"})
document["comment"] = inventory["runtime_dependency_policy"] + (" No embedded runtime libraries were found." if not inventory["embedded_components"] else " Embedded components are inventoried without inferring their licenses or provenance.")
for index, dependency in enumerate(inventory["declared_runtime_dependencies"]):
    spdx_id = f"SPDXRef-DeclaredDependency-{index}"
    packages.append({"SPDXID": spdx_id, "name": dependency["name"], "versionInfo": dependency["version"], "downloadLocation": dependency["source"], "filesAnalyzed": False, "licenseConcluded": "NOASSERTION", "copyrightText": "NOASSERTION", "comment": "Consumer-declared runtime dependency; may be statically linked."})
    relationships.append({"spdxElementId": "SPDXRef-ApplicationBundle-0", "relatedSpdxElement": spdx_id, "relationshipType": "DEPENDS_ON"})
write(args.output, document)
