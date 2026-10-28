//! Materialized, committed-path build inputs. Never execute hooks or copy signing transfers.
use crate::{
    config::App,
    fsutil,
    process::{Executor, Step, checked},
    source,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
};

pub const FILES: [&str; 2] = ["release-source.zip", "source-snapshot.json"];
const MAX_FILE: u64 = 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024;

fn name(value: &str) -> Result<()> {
    crate::config::relative_path(value)?;
    ensure!(
        !value.contains('\\')
            && value
                .split('/')
                .all(|s| !s.is_empty() && s != "." && !source::excluded(s)),
        "Noncanonical or excluded source path: {value}"
    );
    Ok(())
}

pub fn capture(app: &App, executor: &mut impl Executor) -> Result<Value> {
    let expected = std::env::var("SOURCE_SHA")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .unwrap_or_default();
    capture_for_source(app, executor, &expected)
}

pub fn capture_for_source(
    app: &App,
    executor: &mut impl Executor,
    expected: &str,
) -> Result<Value> {
    let target = app.root.join("build/source-snapshot");
    fsutil::confined(&app.root, &target)?;
    // A failed recapture must never leave a usable old snapshot behind.
    ensure!(!target.is_symlink(), "Source snapshot output is a symlink");
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    ensure!(
        expected.len() == 40 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
        "Source capture needs an explicit full source SHA"
    );
    let head = checked(
        executor,
        &Step::new("git", ["rev-parse", "--verify", "HEAD"], 30),
        &app.root,
    )?;
    ensure!(
        head.trim() == expected,
        "Source capture must use the selected checked-out commit"
    );
    let tree = checked(
        executor,
        &Step::new("git", ["ls-tree", "-r", "-z", "HEAD", "--", "."], 60),
        &app.root,
    )?;
    let mut tracked = BTreeMap::new();
    for entry in tree.split('\0').filter(|s| !s.is_empty()) {
        let (header, path) = entry.split_once('\t').context("Malformed Git tree entry")?;
        let parts: Vec<_> = header.split_whitespace().collect();
        ensure!(parts.len() == 3, "Malformed Git tree header");
        // Match the established QA/build fingerprint scope. Excluded caches and vaults are not source.
        if path.split('/').any(source::excluded) {
            continue;
        }
        name(path)?;
        ensure!(
            parts[1] == "blob" && matches!(parts[0], "100644" | "100755"),
            "Source symlinks and submodules require explicit materialized project inputs: {path}"
        );
        ensure!(
            tracked
                .insert(
                    path.to_owned(),
                    if parts[0] == "100755" {
                        0o755_u32
                    } else {
                        0o644
                    }
                )
                .is_none(),
            "Duplicate source path"
        );
    }
    ensure!(
        !tracked.is_empty() && tracked.len() < 200000,
        "Empty or oversized source tree"
    );
    let actual = source::paths(&app.root)?
        .iter()
        .map(|p| {
            Ok(p.strip_prefix(&app.root)?
                .to_str()
                .context("Non-UTF8 source path")?
                .to_owned())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(
        actual == tracked.keys().cloned().collect(),
        "Source capture requires every build input to have a committed path; remove untracked inputs or commit missing/deleted paths"
    );
    let vault = crate::signing::path(app)?;
    fs::create_dir_all(target.parent().unwrap())?;
    let temporary = tempfile::tempdir_in(target.parent().unwrap())?;
    let staged = temporary.path();
    let file = fs::File::create(staged.join(FILES[0]))?;
    let mut zip = zip::ZipWriter::new(file);
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    for (relative, mode) in tracked {
        let path = app.root.join(&relative);
        ensure!(
            path != vault,
            "Move the signing vault outside the source fingerprint scope before capture"
        );
        fsutil::confined(&app.root, &path)?;
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "Source input is not a regular file: {relative}"
        );
        let size = fs::metadata(&path)?.len();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                (fs::metadata(&path)?.permissions().mode() & 0o111 != 0) == (mode == 0o755),
                "Source executable mode differs from the selected commit: {relative}"
            );
        }
        total = total.checked_add(size).context("Source size overflow")?;
        ensure!(
            size <= MAX_FILE && total <= MAX_TOTAL,
            "Source archive exceeds size bounds"
        );
        let mut input = fs::File::open(&path)?;
        let mut prefix = [0_u8; 128];
        let count = input.read(&mut prefix)?;
        ensure!(
            !prefix[..count].starts_with(b"version https://git-lfs.github.com/spec/v1"),
            "Unresolved LFS pointer in source input: {relative}"
        );
        zip.start_file(
            &relative,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(mode),
        )?;
        let mut digest = Sha256::new();
        digest.update(&prefix[..count]);
        zip.write_all(&prefix[..count])?;
        let mut written = count as u64;
        let mut buffer = [0_u8; 65536];
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            ensure!(written <= size, "Source input changed during capture");
            digest.update(&buffer[..n]);
            zip.write_all(&buffer[..n])?;
        }
        ensure!(written == size, "Source input changed during capture");
        files.insert(
            relative,
            json!({"sha256":format!("{:x}", digest.finalize()),"size":size,"mode":mode}),
        );
    }
    zip.finish()?.sync_all()?;
    let inputs = source::fingerprint_entries(
        files
            .iter()
            .map(|(n, v)| (n.as_str(), v["sha256"].as_str().unwrap())),
    );
    ensure!(
        inputs == source::fingerprint(&app.root)?,
        "Source changed during capture"
    );
    let report = json!({"schema_version":1,"scope":"tracked-materialized-build-inputs-v1",
        "source_sha":expected,"run_id":std::env::var("GITHUB_RUN_ID").ok(),
        "run_attempt":std::env::var("GITHUB_RUN_ATTEMPT").ok(),"inputs_sha256":inputs,
        "archive_sha256":fsutil::sha256(&staged.join(FILES[0]))?,"files":files});
    fsutil::json(&staged.join(FILES[1]), &report)?;
    fs::rename(staged, &target)?;
    Ok(report)
}

/// Verify without extracting or executing archived source. Outer package signatures remain mandatory.
pub fn verify(directory: &Path, archive: &Value, attempt: &Value) -> Result<Value> {
    for file in FILES {
        ensure!(
            directory.join(file).is_file() && !directory.join(file).is_symlink(),
            "Missing regular source snapshot member: {file}"
        );
    }
    ensure!(
        fs::metadata(directory.join(FILES[1]))?.len() <= 64 * 1024 * 1024,
        "Source manifest exceeds 64 MiB"
    );
    let report: Value = serde_json::from_slice(&fs::read(directory.join(FILES[1]))?)?;
    ensure!(
        report["schema_version"] == 1
            && report["scope"] == "tracked-materialized-build-inputs-v1"
            && report["source_sha"]
                .as_str()
                .is_some_and(|s| s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            && report["source_sha"] == archive["source_sha"]
            && report["run_id"] == archive["run_id"]
            && report["run_attempt"] == *attempt
            && report["inputs_sha256"] == archive["inputs_sha256"],
        "Source snapshot belongs to different source/build inputs/run/attempt"
    );
    ensure!(
        report["archive_sha256"].as_str() == Some(&fsutil::sha256(&directory.join(FILES[0]))?),
        "Source archive checksum changed"
    );
    let files = report["files"]
        .as_object()
        .context("Source file inventory missing")?;
    ensure!(
        !files.is_empty() && files.len() < 200000,
        "Empty or oversized source inventory"
    );
    let mut zip = zip::ZipArchive::new(fs::File::open(directory.join(FILES[0]))?)?;
    ensure!(
        zip.len() == files.len(),
        "Source archive has missing or extra files"
    );
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        name(entry.name())?;
        ensure!(
            seen.insert(entry.name().to_owned()) && !entry.is_symlink() && !entry.is_dir(),
            "Duplicate or nonregular source entry"
        );
        let record = files.get(entry.name()).context("Unexpected source entry")?;
        let mode = record["mode"].as_u64().context("Source mode missing")?;
        ensure!(
            matches!(mode, 0o644 | 0o755)
                && entry.unix_mode() == Some(0o100000 | mode as u32)
                && record["size"].as_u64() == Some(entry.size())
                && entry.size() <= MAX_FILE,
            "Source mode/size differs from inventory"
        );
        total = total
            .checked_add(entry.size())
            .context("Source size overflow")?;
        ensure!(total <= MAX_TOTAL, "Source archive exceeds size bounds");
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65536];
        let mut read = 0_u64;
        loop {
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            read += n as u64;
            ensure!(read <= entry.size(), "Source entry exceeds declared size");
            digest.update(&buffer[..n]);
        }
        ensure!(
            read == entry.size() && record["sha256"] == format!("{:x}", digest.finalize()),
            "Source content changed"
        );
    }
    let fingerprint = source::fingerprint_entries(
        files
            .iter()
            .map(|(n, v)| (n.as_str(), v["sha256"].as_str().unwrap())),
    );
    ensure!(
        report["inputs_sha256"] == fingerprint,
        "Source inventory differs from verified build inputs"
    );
    Ok(report)
}

pub fn check_build(app: &App, archive: &Value, required: bool) -> Result<()> {
    let folder = app.root.join("build/source-snapshot");
    if required || folder.exists() {
        let report = verify(
            &folder,
            archive,
            &json!(std::env::var("GITHUB_RUN_ATTEMPT").ok()),
        )?;
        for (name, record) in report["files"].as_object().unwrap() {
            let path = app.root.join(name);
            fsutil::confined(&app.root, &path)?;
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file(),
                "Source changed to a nonregular input after capture"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    (metadata.permissions().mode() & 0o111 != 0) == (record["mode"] == 0o755),
                    "Source executable mode changed after capture"
                );
            }
        }
        ensure!(
            archive["inputs_sha256"] == source::fingerprint(&app.root)?,
            "Source snapshot differs from current build inputs"
        );
    }
    Ok(())
}
