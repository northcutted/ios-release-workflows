//! Small OpenStep reader used to change only the chosen targets' signing settings.
use crate::{config::App, fsutil};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Node {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}
#[derive(Clone, Debug)]
pub enum Kind {
    Text(String),
    Map(BTreeMap<String, Node>),
    List(Vec<Node>),
}
impl Node {
    pub fn text(&self) -> Result<&str> {
        match &self.kind {
            Kind::Text(s) => Ok(s),
            _ => bail!("Expected an Xcode project value"),
        }
    }
    pub fn map(&self) -> Result<&BTreeMap<String, Node>> {
        match &self.kind {
            Kind::Map(s) => Ok(s),
            _ => bail!("Expected an Xcode project dictionary"),
        }
    }
    pub fn list(&self) -> Result<&Vec<Node>> {
        match &self.kind {
            Kind::List(s) => Ok(s),
            _ => bail!("Expected an Xcode project array"),
        }
    }
    pub fn get(&self, key: &str) -> Result<&Node> {
        self.map()?
            .get(key)
            .with_context(|| format!("Xcode project has no {key}"))
    }
}
struct Parser<'a> {
    s: &'a str,
    i: usize,
}
impl Parser<'_> {
    fn space(&mut self) -> Result<()> {
        loop {
            while self
                .s
                .as_bytes()
                .get(self.i)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.i += 1;
            }
            if self.s[self.i..].starts_with("//") {
                self.i += self.s[self.i..].find('\n').unwrap_or(self.s.len() - self.i);
            } else if self.s[self.i..].starts_with("/*") {
                self.i += self.s[self.i + 2..]
                    .find("*/")
                    .context("Unterminated project comment")?
                    + 4;
            } else {
                break;
            }
        }
        Ok(())
    }
    fn take(&mut self, c: u8) -> Result<()> {
        self.space()?;
        ensure!(
            self.s.as_bytes().get(self.i) == Some(&c),
            "Unexpected Xcode project syntax"
        );
        self.i += 1;
        Ok(())
    }
    fn text(&mut self) -> Result<String> {
        self.space()?;
        let start = self.i;
        if self.s.as_bytes().get(self.i) == Some(&b'"') {
            self.i += 1;
            let mut out = String::new();
            loop {
                let c = self.s[self.i..]
                    .chars()
                    .next()
                    .context("Unterminated quoted project value")?;
                self.i += c.len_utf8();
                if c == '"' {
                    break;
                }
                if c == '\\' {
                    let c = self.s[self.i..]
                        .chars()
                        .next()
                        .context("Invalid project escape")?;
                    self.i += c.len_utf8();
                    out.push(c);
                } else {
                    out.push(c);
                }
            }
            Ok(out)
        } else {
            while self
                .s
                .as_bytes()
                .get(self.i)
                .is_some_and(|c| !c.is_ascii_whitespace() && !b"{}()=;,".contains(c))
            {
                self.i += 1;
            }
            ensure!(self.i > start, "Missing Xcode project value");
            Ok(self.s[start..self.i].to_owned())
        }
    }
    fn node(&mut self, depth: usize) -> Result<Node> {
        ensure!(depth < 128, "Xcode project nesting is too deep");
        self.space()?;
        let start = self.i;
        let kind = match self.s.as_bytes().get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut map = BTreeMap::new();
                loop {
                    self.space()?;
                    if self.s.as_bytes().get(self.i) == Some(&b'}') {
                        self.i += 1;
                        break;
                    }
                    let key = self.text()?;
                    self.take(b'=')?;
                    let node = self.node(depth + 1)?;
                    self.take(b';')?;
                    ensure!(
                        map.insert(key, node).is_none(),
                        "Duplicate Xcode project field"
                    );
                }
                Kind::Map(map)
            }
            Some(b'(') => {
                self.i += 1;
                let mut list = Vec::new();
                loop {
                    self.space()?;
                    if self.s.as_bytes().get(self.i) == Some(&b')') {
                        self.i += 1;
                        break;
                    }
                    list.push(self.node(depth + 1)?);
                    self.space()?;
                    if self.s.as_bytes().get(self.i) == Some(&b',') {
                        self.i += 1;
                    } else {
                        ensure!(
                            self.s.as_bytes().get(self.i) == Some(&b')'),
                            "Expected a project array separator"
                        );
                    }
                }
                Kind::List(list)
            }
            _ => Kind::Text(self.text()?),
        };
        Ok(Node {
            start,
            end: self.i,
            kind,
        })
    }
}
pub fn parse(s: &str) -> Result<Node> {
    let mut p = Parser { s, i: 0 };
    let node = p.node(0)?;
    p.space()?;
    ensure!(p.i == s.len(), "Trailing Xcode project content");
    Ok(node)
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn signed_project(
    source: &str,
    targets: &[Value],
    configuration: &str,
    team: &str,
) -> Result<String> {
    let root = parse(source)?;
    let objects = root.get("objects")?.map()?;
    let mut edits = Vec::new();
    let mut touched = std::collections::HashSet::new();
    for object in objects.values() {
        if object.map()?.get("isa").and_then(|v| v.text().ok()) != Some("PBXNativeTarget") {
            continue;
        }
        let name = object.get("name")?.text()?;
        let Some(target) = targets.iter().find(|t| t["name"].as_str() == Some(name)) else {
            continue;
        };
        let list = objects
            .get(object.get("buildConfigurationList")?.text()?)
            .context("Missing target build configurations")?
            .get("buildConfigurations")?
            .list()?;
        let mut matched = 0;
        for config_id in list {
            let config = objects
                .get(config_id.text()?)
                .context("Missing build configuration")?;
            if config.get("name")?.text()? != configuration {
                continue;
            }
            let settings = config.get("buildSettings")?;
            let profile = target["profile"]
                .as_str()
                .context("Signing target has no profile")?;
            let desired = [
                ("CODE_SIGN_STYLE", "Manual"),
                ("DEVELOPMENT_TEAM", team),
                ("PROVISIONING_PROFILE_SPECIFIER", profile),
                ("CODE_SIGN_IDENTITY", "Apple Distribution"),
            ];
            let mut additions = String::new();
            for (key, value) in desired {
                for (conditional, existing) in settings.map()? {
                    if conditional.starts_with(&format!("{key}[sdk=iphoneos")) {
                        edits.push((existing.start, existing.end, quoted(value)));
                    }
                }
                if let Some(existing) = settings.map()?.get(key) {
                    edits.push((existing.start, existing.end, quoted(value)));
                } else {
                    additions.push_str(&format!("\n\t\t\t\t{key} = {};", quoted(value)));
                }
            }
            if !additions.is_empty() {
                additions.push('\n');
                edits.push((settings.end - 1, settings.end - 1, additions));
            }
            matched += 1;
        }
        ensure!(
            matched == 1,
            "Expected one {configuration} configuration for target {name}"
        );
        touched.insert(name.to_owned());
    }
    ensure!(
        targets
            .iter()
            .all(|t| t["name"].as_str().is_some_and(|n| touched.contains(n))),
        "A configured signing target was not found in its project"
    );
    edits.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut out = source.to_owned();
    for (start, end, replacement) in edits {
        out.replace_range(start..end, &replacement);
    }
    parse(&out)?;
    Ok(out)
}
pub fn configure(app: &App) -> Result<()> {
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for target in app.config["targets"]
        .as_array()
        .context("Missing signing targets")?
    {
        let project = target["project"]
            .as_str()
            .or_else(|| app.config["project"].as_str())
            .context(
                "Workspace targets need their underlying project path; rerun ios-release init",
            )?;
        groups
            .entry(project.to_owned())
            .or_default()
            .push(target.clone());
    }
    for (project, targets) in groups {
        let path = app
            .root
            .join(crate::config::relative_path(&project)?)
            .join("project.pbxproj");
        fsutil::confined(&app.root, &path)?;
        let source = std::fs::read_to_string(&path)?;
        let updated = signed_project(
            &source,
            &targets,
            app.configuration("archive"),
            app.text("team_id")?,
        )?;
        if updated != source {
            let backup = app
                .root
                .join(".ios-release/project-backups")
                .join(format!("{}.pbxproj", fsutil::sha256(&path)?));
            if !backup.exists() {
                fsutil::atomic(&backup, source.as_bytes())?;
            }
            fsutil::atomic(&path, updated.as_bytes())?;
        }
    }
    Ok(())
}
