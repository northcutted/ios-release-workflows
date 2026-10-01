"""Find platform resources in an immutable checkout or an installed wheel."""
from pathlib import Path


def root():
    source = Path(__file__).resolve().parents[2]
    return source if (source / "Gemfile.lock").is_file() else Path(__file__).parent / "assets"
