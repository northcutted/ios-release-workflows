use crate::fsutil;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn excluded(name: &str) -> bool {
    [
        ".git",
        "build",
        "qa-results",
        "target",
        ".build",
        ".ios-release",
        "store",
        "node_modules",
        "Pods",
        "DerivedData",
        "xcuserdata",
        ".venv",
        "__pycache__",
    ]
    .contains(&name)
}

/// Bind local QA to the inputs present when it ran, including uncommitted source.
pub fn fingerprint(root: &Path) -> Result<String> {
    let files = paths(root)?;
    let mut entries = vec![];
    for path in files {
        entries.push((
            path.strip_prefix(root)?.to_string_lossy().into_owned(),
            fsutil::sha256(&path)?,
        ));
    }
    Ok(fingerprint_entries(
        entries
            .iter()
            .map(|(name, hash)| (name.as_str(), hash.as_str())),
    ))
}

pub(crate) fn fingerprint_entries<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    // Preserve the existing Path ordering: a directory's files precede a sibling
    // such as App.xcodeproj. JSON/BTreeMap string ordering differs at '/'.
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|a, b| Path::new(a.0).cmp(Path::new(b.0)));
    let mut hash = Sha256::new();
    for (name, digest) in entries {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(digest.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(crate) fn paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_owned()];
    let mut files = vec![];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if excluded(&name) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() || kind.is_symlink() {
                fsutil::confined(root, &path)?;
                ensure!(
                    path.is_file(),
                    "Source directory symlinks must be replaced with explicit project references"
                );
                files.push(path);
            }
        }
        ensure!(
            files.len() + pending.len() < 200000,
            "Source inventory exceeded its bound"
        );
    }
    files.sort();
    Ok(files)
}
