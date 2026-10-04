use crate::fsutil;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Bind local QA to the inputs present when it ran, including uncommitted source.
pub fn fingerprint(root: &Path) -> Result<String> {
    let mut pending = vec![root.to_owned()];
    let mut files = vec![];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if [
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
            ]
            .contains(&name.as_ref())
            {
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
    let mut hash = Sha256::new();
    for path in files {
        let name = path.strip_prefix(root)?.to_string_lossy();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(fsutil::sha256(&path)?.as_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}
