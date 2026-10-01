"""Pseudo-localize string catalogs for layout checks; preserve existing translations."""
import argparse
import json
import os
from pathlib import Path
import re


def work(catalog, languages, force=False):
    source_language = catalog.get("sourceLanguage", "en")
    items = []
    for key, entry in catalog.get("strings", {}).items():
        localizations = entry.get("localizations", {})
        source = localizations.get(source_language, {}).get("stringUnit", {}).get("value")
        if not isinstance(source, str) or not source:
            continue
        for language in languages:
            target = localizations.get(language, {}).get("stringUnit", {}).get("value")
            if language != source_language and (force or not isinstance(target, str)):
                items.append((key, language, source))
    return items


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--files", nargs="+")
    parser.add_argument("--languages", nargs="+")
    parser.add_argument("--state", default=os.environ.get("LOCALIZATION_STRING_STATE", "translated"))
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    languages = args.languages or list(filter(None, re.split(r"[,\s]+", os.environ.get("LOCALIZATION_LANGUAGES") or os.environ.get("LANGUAGES", ""))))
    if not languages:
        parser.error("Pass --languages, for example: --languages es fr de ja")
    config = json.loads(Path(os.environ["IOS_RELEASE_CONFIG"]).read_text())
    paths = args.files or config["localization_catalogs"]
    catalogs = [(Path(path), json.loads(Path(path).read_text())) for path in paths]
    changes = [(path, catalog, work(catalog, languages, args.force)) for path, catalog in catalogs]
    count = sum(len(items) for _, _, items in changes)
    print(f"Preparing {count} pseudo-translations across {len(paths)} catalogs.")
    for path, catalog, items in changes:
        if args.dry_run:
            for key, language, _ in items[:20]:
                print(f"[dry-run] {path} {language}: {key}")
            continue
        if not items:
            continue
        for key, language, source in items:
            catalog["strings"][key].setdefault("localizations", {})[language] = {"stringUnit": {"state": args.state, "value": f"[{language}] {source}"}}
        path.write_text(json.dumps(catalog, indent=2, ensure_ascii=False) + "\n")
        print(f"Updated {path}")
    return 0
