#!/usr/bin/env python3
"""Extract request contracts from Apple's downloaded OpenAPI JSON (development only)."""
import hashlib
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
source = pathlib.Path(sys.argv[1])
spec = json.loads(source.read_bytes())


def dereference(value):
    if isinstance(value, list):
        return [dereference(v) for v in value]
    if not isinstance(value, dict):
        return value
    if "$ref" in value:
        resolved = spec
        for key in value["$ref"].removeprefix("#/").split("/"):
            resolved = resolved[key]
        return dereference(resolved)
    return {key: ({name: dereference(schema) for name, schema in v.items()} if key == "properties" else dereference(v)) for key, v in value.items()
            if key not in {"description", "title", "format", "deprecated", "readOnly", "writeOnly", "example"}}


normalize = lambda path: re.sub(r"\{[^}]*\}", "{}", path)
used = set()
for module in ["api", "signing", "store", "metadata"]:
    text = (root / f"rust/src/{module}.rs").read_text()
    used.update(normalize(p) for p in re.findall(r'"(/v1/[^"?]+)', text))

contracts = {}
for path, operations in spec["paths"].items():
    pattern = normalize(path)
    if pattern not in used:
        continue
    contract = {}
    for method, operation in operations.items():
        if method == "parameters":
            continue
        parameters = operation.get("parameters", []) + operations.get("parameters", [])
        item = {"query": [p["name"] for p in parameters if p.get("in") == "query"],
                "required_query": [p["name"] for p in parameters if p.get("in") == "query" and p.get("required")]}
        body = operation.get("requestBody", {})
        if body:
            item["body"] = dereference(body["content"]["application/json"]["schema"])
        contract[method.upper()] = item
    contracts[pattern] = contract

missing = used - contracts.keys()
if missing:
    raise SystemExit(f"Native endpoints missing from Apple OpenAPI: {sorted(missing)}")
output = {"source": "https://developer.apple.com/sample-code/app-store-connect/app-store-connect-openapi-specification.zip",
          "spec_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
          "info": spec["info"], "paths": contracts}
(root / "rust/tests/fixtures/apple-contracts.json").write_text(json.dumps(output, indent=2, sort_keys=True) + "\n")
print(f"Extracted {len(contracts)} endpoint contracts from Apple OpenAPI.")
