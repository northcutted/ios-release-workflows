use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct App {
    pub root: PathBuf,
    pub config: Value,
    pub config_path: PathBuf,
}

pub fn relative_path(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && !value.contains(['\n', '\r', '\0']),
        "Unsafe app path"
    );
    let path = PathBuf::from(value);
    ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "App paths must remain relative to the app root"
    );
    Ok(path)
}

pub fn filename(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value != "."
            && value != ".."
            && !value.contains(['/', '\\', '\n', '\r', '\0']),
        "Unsafe filename component"
    );
    Ok(())
}

pub fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .context("Expected a list")?
        .iter()
        .map(|v| {
            let text = v.as_str().context("Expected a string")?;
            ensure!(
                !text.is_empty() && !text.contains(['\n', '\r', '\0']),
                "Empty or invalid configured name"
            );
            Ok(text.to_owned())
        })
        .collect()
}

impl App {
    pub fn load(root: &Path, config: &Path) -> Result<Self> {
        let root = root.canonicalize().context("App root does not exist")?;
        let path = if config == Path::new(".github/ios-release.json")
            && !root.join(config).exists()
            && root.join(".ios-release.json").exists()
        {
            root.join(".ios-release.json")
        } else if config.is_absolute() {
            config.to_owned()
        } else {
            root.join(config)
        };
        let mut config: Value =
            serde_json::from_slice(&fs::read(&path).context("Cannot read app configuration")?)?;
        if std::env::var("IOS_RELEASE_NATIVE_WORKFLOW").as_deref() == Ok("1") {
            let workspace = PathBuf::from(
                std::env::var_os("GITHUB_WORKSPACE").context("Hosted workspace missing")?,
            )
            .canonicalize()?;
            ensure!(
                root.starts_with(&workspace),
                "Native workflow app root escapes its source checkout"
            );
            crate::fsutil::confined(&root, &path)?;
        }
        if config["schema_version"] == 2 {
            let values = config
                .as_object_mut()
                .context("Configuration must be an object")?;
            for (key, default) in [
                ("localization_catalogs", serde_json::json!([])),
                ("localization_locales", serde_json::json!([])),
                ("locales", serde_json::json!(["en-US"])),
                ("screens", serde_json::json!([])),
                ("screenshot_devices", serde_json::json!([])),
                ("metadata_path", serde_json::json!("store/metadata")),
                ("screenshots_path", serde_json::json!("store/screenshots")),
                ("team_id", serde_json::json!("")),
                ("targets", serde_json::json!([])),
                ("app_store", serde_json::json!({})),
            ] {
                values.entry(key).or_insert(default);
            }
            if !values.contains_key("compatibility") {
                values.insert(
                    "compatibility".into(),
                    values.get("xcode").cloned().unwrap_or(Value::Null),
                );
            }
        }
        let app = Self {
            root,
            config,
            config_path: path,
        };
        app.validate()?;
        Ok(app)
    }

    pub fn text(&self, key: &str) -> Result<&str> {
        self.config[key]
            .as_str()
            .with_context(|| format!("Missing string configuration: {key}"))
    }

    pub fn names(&self, key: &str) -> Result<Vec<String>> {
        strings(&self.config[key])
    }

    pub fn path(&self, key: &str) -> Result<PathBuf> {
        let path = self.root.join(relative_path(self.text(key)?)?);
        crate::fsutil::confined(&self.root, &path)?;
        Ok(path)
    }

    pub fn project(&self) -> Result<(&str, &str)> {
        match (
            self.config["project"].as_str(),
            self.config["workspace"].as_str(),
        ) {
            (Some(project), None) => Ok(("project", project)),
            (None, Some(workspace)) => Ok(("workspace", workspace)),
            _ => bail!("Choose exactly one Xcode project or workspace"),
        }
    }

    pub fn xcode(&self, compatibility: bool) -> Result<&Value> {
        self.config[if compatibility {
            "compatibility"
        } else {
            "xcode"
        }]
        .as_object()
        .context("Missing configured Xcode")?;
        Ok(&self.config[if compatibility {
            "compatibility"
        } else {
            "xcode"
        }])
    }

    fn validate(&self) -> Result<()> {
        if self.config["schema_version"] == 2 {
            return self.validate_v2();
        }
        ensure!(
            self.config["schema_version"] == 1,
            "Unsupported app configuration schema"
        );
        ensure!(
            self.text("default_branch")? == "main",
            "v1 requires protected main"
        );
        let repo = self.text("repository")?;
        ensure!(
            regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")?.is_match(repo),
            "Invalid repository"
        );
        if let Ok(expected) = std::env::var("GITHUB_REPOSITORY") {
            ensure!(
                expected.eq_ignore_ascii_case(repo),
                "Configuration belongs to another repository"
            );
        }
        let (_, project) = self.project()?;
        relative_path(project)?;
        ensure!(!self.text("scheme")?.is_empty(), "Scheme is required");
        ensure!(
            !self.names("test_targets")?.is_empty(),
            "Explicit test targets are required"
        );
        for target in self.names("test_targets")? {
            filename(&target)?;
        }
        for key in ["xcode", "compatibility"] {
            let expected = self.xcode(key == "compatibility")?;
            for field in ["path", "version", "build", "sdk", "runtime"] {
                let value = expected[field]
                    .as_str()
                    .context("Incomplete Xcode configuration")?;
                ensure!(
                    !value.is_empty() && !value.contains(['\n', '\r', '\0']),
                    "Invalid Xcode configuration"
                );
            }
            ensure!(
                Path::new(expected["path"].as_str().unwrap()).is_absolute(),
                "Xcode path must be absolute"
            );
        }
        ensure!(
            regex::Regex::new(r"^[A-Z0-9]{10}$")?.is_match(self.text("team_id")?),
            "Invalid Apple team"
        );
        for key in [
            "metadata_path",
            "screenshots_path",
            "accessibility_path",
            "localization_catalog",
        ] {
            if let Some(value) = self.config[key].as_str()
                && !value.is_empty()
            {
                relative_path(value)?;
            }
        }
        for key in ["screenshot_devices", "locales", "screens"] {
            let names = self.names(key)?;
            ensure!(
                !names.is_empty() && names.iter().collect::<HashSet<_>>().len() == names.len(),
                "{key} must be unique and nonempty"
            );
            for name in names {
                filename(&name)?;
            }
        }
        for path in self.names("localization_catalogs")? {
            relative_path(&path)?;
        }
        let catalogs = self.names("localization_catalogs")?;
        ensure!(
            catalogs.is_empty() || !self.names("localization_locales")?.is_empty(),
            "Localization locales must be explicit for configured catalogs"
        );
        let targets = self.config["targets"]
            .as_array()
            .context("Missing application targets")?;
        ensure!(!targets.is_empty(), "Application targets are required");
        let mut ids = HashSet::new();
        let bundle_id = regex::Regex::new(r"^[A-Za-z0-9.-]+$")?;
        for target in targets {
            let id = target["bundle_id"]
                .as_str()
                .context("Target bundle ID is required")?;
            ensure!(
                bundle_id.is_match(id) && ids.insert(id),
                "Invalid or duplicate target bundle ID"
            );
            ensure!(
                target["profile"].as_str().is_some_and(|v| !v.is_empty()),
                "Each target needs a provisioning profile"
            );
            ensure!(
                target["entitlements"].is_object()
                    && target["tracking"].is_boolean()
                    && target["non_exempt_encryption"].is_boolean(),
                "Each target must declare entitlements and privacy policy"
            );
        }
        ensure!(
            self.config["app_store"]["bundle_id"] == targets[0]["bundle_id"],
            "Main application target must be first"
        );
        Ok(())
    }
    fn validate_v2(&self) -> Result<()> {
        let (_, project) = self.project()?;
        relative_path(project)?;
        ensure!(
            !self.text("scheme")?.is_empty(),
            "App scheme is required; run ios-release init"
        );
        for key in ["metadata_path", "screenshots_path"] {
            relative_path(self.text(key)?)?;
        }
        for key in [
            "test_targets",
            "localization_catalogs",
            "localization_locales",
            "locales",
            "screens",
            "screenshot_devices",
        ] {
            let values = if self.config[key].is_null() {
                Vec::new()
            } else {
                self.names(key)?
            };
            ensure!(
                values.iter().collect::<HashSet<_>>().len() == values.len(),
                "Duplicate configured {key}"
            );
            for value in values {
                if key == "localization_catalogs" {
                    relative_path(&value)?;
                } else {
                    filename(&value)?;
                }
            }
        }
        if let Some(repo) = self.config["repository"].as_str() {
            ensure!(
                regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")?.is_match(repo),
                "Invalid repository"
            );
            if let Ok(expected) = std::env::var("GITHUB_REPOSITORY") {
                ensure!(
                    repo.eq_ignore_ascii_case(&expected),
                    "Configuration belongs to another repository"
                );
            }
        }
        let mut ids = HashSet::new();
        let bundle_pattern = regex::Regex::new(r"^[A-Za-z0-9.-]+$")?;
        for target in self.config["targets"]
            .as_array()
            .context("Targets must be a list")?
        {
            let id = target["bundle_id"]
                .as_str()
                .context("Target needs a bundle ID")?;
            ensure!(
                bundle_pattern.is_match(id) && ids.insert(id),
                "Invalid or duplicate bundle ID"
            );
        }
        Ok(())
    }
    pub fn require_signing(&self) -> Result<()> {
        ensure!(
            self.config["app_store"]["bundle_id"] == self.config["targets"][0]["bundle_id"],
            "Main app target must be first and match App Store bundle ID"
        );
        ensure!(
            regex::Regex::new(r"^[A-Z0-9]{10}$")?.is_match(self.text("team_id")?),
            "Set your Apple team ID with ios-release init --team-id"
        );
        ensure!(
            self.config["targets"]
                .as_array()
                .is_some_and(|v| !v.is_empty()),
            "Application targets must be configured"
        );
        Ok(())
    }
    pub fn require_archive(&self) -> Result<()> {
        self.require_signing()?;
        for target in self.config["targets"].as_array().unwrap() {
            ensure!(
                target["profile"].as_str().is_some_and(|s| !s.is_empty()),
                "Missing provisioning profile; run ios-release signing sync"
            );
            ensure!(
                target["entitlements"].is_object()
                    && target["tracking"].is_boolean()
                    && target["non_exempt_encryption"].is_boolean(),
                "Declare each target's entitlements, tracking and encryption before archiving"
            );
        }
        Ok(())
    }
    pub fn save(&self) -> Result<()> {
        crate::fsutil::json(&self.config_path, &self.config)
    }
    pub fn configuration(&self, operation: &str) -> &str {
        self.config["configurations"][operation]
            .as_str()
            .unwrap_or(if operation == "archive" {
                "Release"
            } else {
                "Debug"
            })
    }
    pub fn qa_checks(&self) -> Result<Vec<String>> {
        let checks = if self.config["qa_checks"].is_null() {
            vec!["analyze".into(), "localization".into(), "test".into()]
        } else {
            strings(&self.config["qa_checks"])?
        };
        ensure!(
            checks.iter().any(|v| v == "test")
                && checks.iter().all(|v| [
                    "analyze",
                    "localization",
                    "test",
                    "test-compatibility",
                    "lint"
                ]
                .contains(&v.as_str()))
                && checks.iter().collect::<HashSet<_>>().len() == checks.len(),
            "QA checks must be unique supported checks and include app tests"
        );
        Ok(checks)
    }
}
