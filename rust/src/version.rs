//! Read-only Conventional Commit version selection, compatible with the deployed policy.
use crate::{
    config::App,
    fsutil,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Clone, Debug)]
struct Commit {
    hash: String,
    header: String,
    body: String,
    kind: String,
    scope: String,
    subject: String,
    notes: Vec<String>,
    reverted: Option<(String, String)>,
}
fn stable(value: &str) -> Option<[u64; 3]> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let mut result = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty()
            || !part.bytes().all(|v| v.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        result[index] = part.parse().ok()?;
    }
    Some(result)
}
fn parse(hash: &str, message: &str) -> Commit {
    let (header, body) = message.split_once('\n').unwrap_or((message, ""));
    let re = Regex::new(r"^(\w*)(?:\((.*)\))?(!)?: (.*)$").unwrap();
    let caps = re.captures(header);
    let subject = caps
        .as_ref()
        .map_or(header, |c| c.get(4).unwrap().as_str())
        .to_owned();
    let mut notes = vec![];
    let mut note: Option<String> = None;
    let footer = Regex::new(r"^(?:[A-Za-z-]+:|(?:Closes|Fixes|Resolves)\s+#)").unwrap();
    for line in body.lines() {
        if let Some(text) = line
            .strip_prefix("BREAKING CHANGE:")
            .or_else(|| line.strip_prefix("BREAKING-CHANGE:"))
        {
            if let Some(text) = note.take() {
                notes.push(text.trim().to_owned());
            }
            note = Some(text.trim_start().to_owned());
        } else if footer.is_match(line) {
            if let Some(text) = note.take() {
                notes.push(text.trim().to_owned());
            }
        } else if let Some(text) = note.as_mut() {
            text.push('\n');
            text.push_str(line);
        }
    }
    if let Some(text) = note {
        notes.push(text.trim().to_owned());
    }
    if notes.is_empty() && caps.as_ref().is_some_and(|c| c.get(3).is_some()) {
        notes.push(subject.clone());
    }
    let revert =
        Regex::new(r#"(?is)^(?:Revert|revert:)\s"?(.+?)"?\s*This reverts commit (\w*)\."#).unwrap();
    let reverted = revert
        .captures(message)
        .map(|c| (c[2].to_owned(), c[1].trim_end_matches('"').to_owned()));
    Commit {
        hash: hash.into(),
        header: header.into(),
        body: body.into(),
        kind: caps
            .as_ref()
            .map_or("", |c| c.get(1).unwrap().as_str())
            .into(),
        scope: caps
            .as_ref()
            .and_then(|c| c.get(2))
            .map_or("", |c| c.as_str())
            .into(),
        subject,
        notes,
        reverted,
    }
}
fn glob(pattern: &str, value: &str) -> Result<bool> {
    let mut expression = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => expression.push_str(".*"),
            '?' => expression.push('.'),
            '[' => {
                expression.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    expression.push('^');
                }
                let mut closed = false;
                for c in chars.by_ref() {
                    expression.push(c);
                    if c == ']' {
                        closed = true;
                        break;
                    }
                }
                ensure!(closed, "Invalid version policy glob");
            }
            _ => expression.push_str(&regex::escape(&c.to_string())),
        }
    }
    expression.push('$');
    Ok(Regex::new(&expression)?.is_match(value))
}
fn rank(value: &Value) -> Result<u8> {
    match value.as_str() {
        Some("major") => Ok(3),
        Some("minor") => Ok(2),
        Some("patch") => Ok(1),
        _ if value.is_null() || value == false => Ok(0),
        _ => anyhow::bail!("Unsupported release rule"),
    }
}
fn matches(rule: &Value, commit: &Commit) -> Result<bool> {
    for (key, value) in rule.as_object().context("Release rule must be an object")? {
        let valid = match key.as_str() {
            "release" => {
                rank(value)?;
                true
            }
            "breaking" => {
                value.as_bool().context("breaking must be boolean")? == !commit.notes.is_empty()
            }
            "revert" => {
                value.as_bool().context("revert must be boolean")? == commit.reverted.is_some()
            }
            "type" => glob(
                value.as_str().context("type must be a pattern")?,
                &commit.kind,
            )?,
            "scope" => glob(
                value.as_str().context("scope must be a pattern")?,
                &commit.scope,
            )?,
            _ => anyhow::bail!("Unsupported release rule field {key}"),
        };
        if !valid {
            return Ok(false);
        }
    }
    Ok(true)
}
fn refs(text: &str, base: &str) -> String {
    Regex::new(r"(^|[^\w/])#(\d+)")
        .unwrap()
        .replace_all(text, |c: &regex::Captures| {
            format!("{}[#{}]({base}/issues/{})", &c[1], &c[2], &c[2])
        })
        .into_owned()
}

pub fn decide(
    settings: &Value,
    history: &Value,
    previous: Option<&str>,
    source: &str,
    repository: &str,
    date: &str,
    replacement: Option<&Value>,
) -> Result<Value> {
    ensure!(
        settings["schema_version"] == 1 && settings.as_object().is_some_and(|v| v.len() == 3),
        "Unsupported version policy"
    );
    let rules = settings["release_rules"]
        .as_array()
        .context("Version release rules missing")?;
    let types = settings["note_types"]
        .as_array()
        .context("Release note types missing")?;
    for entry in types {
        ensure!(
            entry["type"].is_string()
                && entry.as_object().is_some_and(|v| v
                    .keys()
                    .all(
                        |k| ["type", "scope", "section", "hidden", "effect"].contains(&k.as_str())
                    )),
            "Unsupported release note type"
        );
    }
    let commits = history
        .as_array()
        .context("Commit history must be an array")?
        .iter()
        .map(|c| {
            Ok(parse(
                c["hash"].as_str().context("Commit hash missing")?,
                c["message"].as_str().context("Commit message missing")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut removed = BTreeSet::new();
    for (i, c) in commits.iter().enumerate() {
        if removed.contains(&i) {
            continue;
        }
        if let Some((hash, header)) = &c.reverted
            && let Some((j, _)) = commits.iter().enumerate().find(|(j, c)| {
                *j != i && !removed.contains(j) && c.hash.starts_with(hash) && c.header == *header
            })
        {
            removed.insert(i);
            removed.insert(j);
        }
    }
    let commits = commits
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, c)| c)
        .collect::<Vec<_>>();
    let defaults = json!([{"breaking":true,"release":"major"},{"revert":true,"release":"patch"},{"type":"feat","release":"minor"},{"type":"fix","release":"patch"},{"type":"perf","release":"patch"}]);
    // Validate every rule, including fields hidden behind an early nonmatch.
    for rule in rules {
        ensure!(
            rule.as_object().is_some_and(|v| v
                .keys()
                .all(|k| ["type", "scope", "breaking", "revert", "release"].contains(&k.as_str()))),
            "Unsupported release rule"
        );
        rank(&rule["release"])?;
    }
    let mut bump = 0;
    for commit in &commits {
        let mut matched = false;
        for rule in rules {
            if matches(rule, commit)? {
                matched = true;
                bump = bump.max(rank(&rule["release"])?);
            }
        }
        if !matched {
            for rule in defaults.as_array().unwrap() {
                if matches(rule, commit)? {
                    bump = bump.max(rank(&rule["release"])?);
                }
            }
        }
    }
    let mut number = previous
        .map(|v| stable(v.strip_prefix('v').unwrap_or(v)).context("Invalid prior stable tag"))
        .transpose()?
        .unwrap_or([1, 0, 0]);
    if previous.is_some() && bump > 0 {
        let index = 3 - bump as usize;
        number[index] = number[index].checked_add(1).context("Version overflow")?;
        for n in number.iter_mut().skip(index + 1) {
            *n = 0;
        }
    }
    let mut version = format!("{}.{}.{}", number[0], number[1], number[2]);
    let mut tag = Some(format!("v{version}"));
    if let Some(r) = replacement {
        let v = r["version"]
            .as_str()
            .context("Replacement version missing")?;
        ensure!(
            stable(v).is_some()
                && r["source_tag"] == format!("v{v}")
                && r["source_tag"].as_str() == previous,
            "Replacement must target the highest reachable stable release"
        );
        version = v.into();
        tag = None;
    }
    let notes = if bump == 0 {
        "Verification build; no release changes.".into()
    } else {
        let base = format!("https://github.com/{repository}");
        let heading = if let Some(previous) = previous {
            format!(
                "[{version}]({base}/compare/{previous}...{})",
                tag.as_deref().unwrap_or(source)
            )
        } else {
            version.clone()
        };
        let mut sections = vec![format!("## {heading} ({date})")];
        let mut breaking = vec![];
        let mut groups = BTreeMap::<String, Vec<(String, String, String)>>::new();
        let close_refs = Regex::new(r"(?im)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)")?;
        for c in &commits {
            let kind = if c.reverted.is_some() {
                "revert"
            } else {
                &c.kind
            }
            .to_lowercase();
            let entry = types.iter().find(|t| {
                t["type"] == kind
                    && t["scope"]
                        .as_str()
                        .is_none_or(|s| s.is_empty() || s == c.scope)
            });
            if c.notes.is_empty() && entry.is_none_or(|e| e["effect"] == "hidden") {
                continue;
            }
            let scope = if c.scope == "*" { "" } else { &c.scope };
            let prefix = if scope.is_empty() {
                String::new()
            } else {
                format!("**{scope}:** ")
            };
            breaking.extend(
                c.notes
                    .iter()
                    .map(|n| format!("{prefix}{}", refs(n, &base))),
            );
            let section = entry.map_or(&*c.kind, |e| e["section"].as_str().unwrap_or(""));
            let mut line = format!(
                "* {prefix}{} ([{}]({base}/commit/{}))",
                refs(&c.subject, &base),
                c.hash.chars().take(7).collect::<String>(),
                c.hash
            );
            let mut issues = vec![];
            for cap in close_refs.captures_iter(&c.body) {
                if !issues.contains(&cap[1].to_owned())
                    && !c.subject.contains(&format!("#{}", &cap[1]))
                {
                    issues.push(cap[1].to_owned());
                }
            }
            if !issues.is_empty() {
                line += &format!(
                    ", closes {}",
                    issues
                        .iter()
                        .map(|i| format!("[#{i}]({base}/issues/{i})"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            groups
                .entry(section.into())
                .or_default()
                .push((scope.into(), c.subject.clone(), line));
        }
        if !breaking.is_empty() {
            breaking.sort();
            sections.push(format!(
                "### ⚠ BREAKING CHANGES\n\n{}",
                breaking
                    .iter()
                    .map(|n| format!("* {}", n.replace('\n', "\n  ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        let order = types
            .iter()
            .filter_map(|v| v["section"].as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let mut groups = groups.into_iter().collect::<Vec<_>>();
        groups.sort_by_key(|(section, _)| {
            order
                .iter()
                .position(|s| *s == section)
                .map_or(0, |n| n + 1)
        });
        for (section, mut lines) in groups {
            lines.sort();
            sections.push(format!(
                "{}{}",
                if section.is_empty() {
                    String::new()
                } else {
                    format!("### {section}\n\n")
                },
                lines
                    .into_iter()
                    .map(|v| v.2)
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        sections.join("\n\n") + "\n"
    };
    Ok(
        json!({"will_release":bump>0||replacement.is_some(),"version":version,"git_tag":tag,"replacement_for":replacement.map(|r|r["source_tag"].clone()),"source_sha":source,"notes":notes}),
    )
}

pub fn analyze(app: &App, requested: Option<&str>, executor: &mut impl Executor) -> Result<Value> {
    let git = |executor: &mut _, args: &[&str]| {
        checked(
            executor,
            &Step::new("git", args.iter().copied(), 60),
            &app.root,
        )
    };
    let source = git(executor, &["rev-parse", "HEAD"])?;
    let policy = app.root.join(".github/ios-version.json");
    let mut decision = if policy.exists() {
        ensure!(
            git(executor, &["rev-parse", "--is-shallow-repository"])? == "false",
            "Version selection requires complete history; checkout with fetch-depth: 0"
        );
        let tags = git(executor, &["tag", "--merged", &source])?;
        let previous = tags
            .lines()
            .filter_map(|tag| stable(tag.strip_prefix('v')?).map(|n| (n, tag)))
            .max_by_key(|(n, _)| *n)
            .map(|(_, s)| s);
        let range = previous.map_or_else(|| source.clone(), |t| format!("{t}..{source}"));
        let hashes = git(executor, &["rev-list", &range])?;
        let mut history = vec![];
        for hash in hashes.lines() {
            history.push(
                json!({"hash":hash,"message":git(executor,&["show","-s","--format=%B",hash])?}),
            );
        }
        let date = time::OffsetDateTime::now_utc().date().to_string();
        decide(
            &serde_json::from_slice(&fs::read(policy)?)?,
            &json!(history),
            previous,
            &source,
            app.config["repository"]
                .as_str()
                .context("Repository missing")?,
            &date,
            app.config.get("replacement_release"),
        )?
    } else {
        json!({"source_sha":source,"notes":"Explicit native release preparation.","will_release":true})
    };
    if let Some(value) = requested.filter(|v| !v.is_empty()) {
        ensure!(
            stable(value).is_some(),
            "Release version must be MAJOR.MINOR.PATCH"
        );
        if decision["version"] != value {
            decision["notes"] = json!("Explicit native release preparation.");
        }
        decision["version"] = json!(value);
        decision["git_tag"] = json!(format!("v{value}"));
    }
    ensure!(
        decision["version"].is_string(),
        "Pass --requested or configure .github/ios-version.json"
    );
    Ok(decision)
}
pub fn write_output(value: &Value, output: &Path, github_output: Option<&Path>) -> Result<()> {
    fsutil::json(output, value)?;
    if let Some(path) = github_output {
        use std::io::Write;
        let version = value["version"].as_str().context("Version missing")?;
        ensure!(stable(version).is_some(), "Invalid version output");
        writeln!(
            fs::OpenOptions::new().append(true).open(path)?,
            "version={version}"
        )?;
    }
    Ok(())
}

/// A repository workflow counter reserves distinct numbers even before an IPA reaches Apple.
/// Never fall back to Apple's maximum alone: multiple unuploaded preparations would collide.
pub fn github_number(config: &Value, minimum: &str, run: &str, attempt: &str) -> Result<String> {
    let run: u64 = run.parse()?;
    let attempt: u64 = attempt.parse()?;
    let minimum: u64 = minimum.parse()?;
    let offset = config["build_number_offset"].as_u64().unwrap_or(0);
    let number = run.checked_add(offset).context("Build number overflow")?;
    ensure!(
        run > 0 && (1..=99).contains(&attempt) && (1..=9999).contains(&number),
        "Build number must fit Apple's format (1..9999, attempt 1..99)"
    );
    ensure!(
        number >= minimum,
        "Actions build number {number} is behind Apple's next number {minimum}; increase build_number_offset before preparing this workflow's first release"
    );
    Ok(format!("{number}.{attempt}"))
}
