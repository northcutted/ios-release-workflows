"""Safe workflow YAML with YAML 1.2 booleans and duplicate-key rejection."""
import re
from pathlib import Path

import yaml


class WorkflowLoader(yaml.SafeLoader):
    pass


# YAML 1.1 considers GitHub's `on` key a boolean. Never mutate SafeLoader.
WorkflowLoader.yaml_implicit_resolvers = {
    key: [(tag, pattern) for tag, pattern in values if tag != "tag:yaml.org,2002:bool"]
    for key, values in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
WorkflowLoader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|false)$", re.I), list("tTfF"))


def mapping(loader, node):
    explicit = [loader.construct_object(key) for key, _ in node.value
                if key.tag != "tag:yaml.org,2002:merge"]
    if len(explicit) != len(set(explicit)):
        raise ValueError("Workflow keys must be unique")
    loader.flatten_mapping(node)
    return dict(loader.construct_pairs(node))


WorkflowLoader.add_constructor("tag:yaml.org,2002:map", mapping)


def load(path):
    value = yaml.load(Path(path).read_text(), Loader=WorkflowLoader)
    if not isinstance(value, dict):
        raise ValueError(f"Expected a workflow mapping: {path}")
    return value


def workflows(directory):
    return {p.name: load(p) for p in sorted(Path(directory).iterdir()) if p.suffix in {".yml", ".yaml"}}
