use crate::{
    config::{App, relative_path},
    fsutil,
    process::{Executor, Step, checked},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Options {
    pub project: Option<String>,
    pub workspace: Option<String>,
    pub scheme: Option<String>,
    pub team: Option<String>,
    pub repository: Option<String>,
    pub xcode: Option<String>,
    pub no_workflows: bool,
    pub platform_revision: String,
    pub tracking: Option<bool>,
    pub non_exempt_encryption: Option<bool>,
}
fn choose(label: &str, values: &[String]) -> Result<String> {
    ensure!(
        !values.is_empty(),
        "No {label} found; create a shared Xcode scheme and rerun init"
    );
    if values.len() == 1 {
        return Ok(values[0].clone());
    }
    ensure!(
        std::io::IsTerminal::is_terminal(&io::stdin()),
        "Multiple {label} found: {}. Choose it explicitly with --{label}",
        values.join(", ")
    );
    println!("Choose {label}:");
    for (i, value) in values.iter().enumerate() {
        println!("  {}. {value}", i + 1);
    }
    print!("Selection: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let index: usize = input.trim().parse().context("Enter a selection number")?;
    values
        .get(index.checked_sub(1).context("Invalid selection")?)
        .cloned()
        .context("Invalid selection")
}
pub fn discover_xcode(
    root: &Path,
    developer: Option<&str>,
    executor: &mut impl Executor,
) -> Result<(Value, Value)> {
    let developer = if let Some(path) = developer {
        path.to_owned()
    } else {
        checked(executor, &Step::new("xcode-select", ["-p"], 30), root)?
    };
    ensure!(
        Path::new(&developer).is_absolute(),
        "Xcode developer path must be absolute"
    );
    let version = checked(
        executor,
        &Step::new("xcodebuild", ["-version"], 30).developer(&developer),
        root,
    )?;
    let lines = version.lines().collect::<Vec<_>>();
    ensure!(lines.len() == 2, "Xcode did not report its version");
    let version = lines[0]
        .strip_prefix("Xcode ")
        .context("Unexpected Xcode version")?;
    let build = lines[1]
        .strip_prefix("Build version ")
        .context("Unexpected Xcode build")?;
    let sdk = checked(
        executor,
        &Step::new("xcrun", ["--sdk", "iphoneos", "--show-sdk-version"], 30).developer(&developer),
        root,
    )?;
    let runtimes: Value = serde_json::from_str(&checked(
        executor,
        &Step::new("xcrun", ["simctl", "list", "runtimes", "--json"], 30).developer(&developer),
        root,
    )?)?;
    let runtime = runtimes["runtimes"]
        .as_array()
        .context("Simulator runtimes unavailable")?
        .iter()
        .filter(|r| {
            r["isAvailable"] == true
                && r["identifier"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("com.apple.CoreSimulator.SimRuntime.iOS-"))
        })
        .max_by_key(|r| {
            r["version"]
                .as_str()
                .unwrap_or("")
                .split('.')
                .map(|n| n.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .context("Install an iOS simulator runtime in Xcode Settings > Components")?;
    let devices: Value = serde_json::from_str(&checked(
        executor,
        &Step::new(
            "xcrun",
            ["simctl", "list", "devices", "available", "--json"],
            30,
        )
        .developer(&developer),
        root,
    )?)?;
    Ok((
        json!({"path":developer,"version":version,"build":build,"sdk":sdk,"runtime":runtime["version"]}),
        devices,
    ))
}
#[allow(clippy::too_many_arguments)] // Discovery inputs stay explicit and independently testable.
pub fn config_from_settings(
    root: &Path,
    kind: &str,
    project: &str,
    scheme: &str,
    xcode: Value,
    devices: &Value,
    settings: &Value,
    options: &Options,
) -> Result<Value> {
    let entries = settings
        .as_array()
        .context("Xcode build settings must be a list")?;
    let mut app = Vec::new();
    let mut extensions = Vec::new();
    let mut tests = Vec::new();
    let mut catalogs = Vec::new();
    for entry in entries {
        let s = &entry["buildSettings"];
        let product = s["PRODUCT_TYPE"].as_str().unwrap_or("");
        let name = entry["target"]
            .as_str()
            .context("Build target has no name")?;
        if product.contains("unit-test") {
            tests.push(name.to_owned());
        }
        if product == "com.apple.product-type.application"
            || product == "com.apple.product-type.app-extension"
        {
            let bundle = s["PRODUCT_BUNDLE_IDENTIFIER"]
                .as_str()
                .context("App target has no bundle identifier")?;
            let project_path = Path::new(s["PROJECT_FILE_PATH"].as_str().unwrap_or(project));
            let project_path = if project_path.is_absolute() {
                project_path
                    .strip_prefix(root)
                    .context("Signing project lies outside this checkout")?
            } else {
                project_path
            };
            let entitlements = if let Some(path) = s["CODE_SIGN_ENTITLEMENTS"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                let path = root.join(relative_path(path)?);
                let value =
                    plist::Value::from_file(&path).context("Cannot read target entitlements")?;
                serde_json::to_value(value)?
            } else {
                json!({})
            };
            let target = json!({"name":name,"project":project_path,"bundle_id":bundle,"profile":"","entitlements":entitlements,"tracking":options.tracking,"non_exempt_encryption":options.non_exempt_encryption,"capabilities":capabilities(&entitlements)});
            if product == "com.apple.product-type.application" {
                app.push(target);
            } else {
                extensions.push(target);
            }
        }
    }
    ensure!(
        app.len() == 1,
        "The chosen scheme must contain exactly one main iOS app"
    );
    let team = options
        .team
        .clone()
        .or_else(|| {
            entries.iter().find_map(|e| {
                e["buildSettings"]["DEVELOPMENT_TEAM"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
        })
        .unwrap_or_default();
    let runtime = format!(
        "com.apple.CoreSimulator.SimRuntime.iOS-{}",
        xcode["runtime"].as_str().unwrap().replace('.', "-")
    );
    let available = devices["devices"][&runtime]
        .as_array()
        .context("No available devices for the selected iOS runtime")?;
    let device = available
        .iter()
        .find(|d| {
            d["isAvailable"] == true && d["name"].as_str().is_some_and(|s| s.starts_with("iPhone"))
        })
        .context("Create an iPhone simulator in Xcode")?["name"]
        .as_str()
        .unwrap();
    for entry in walk(root)? {
        if entry.extension().is_some_and(|e| e == "xcstrings") {
            catalogs.push(entry.strip_prefix(root)?.to_string_lossy().into_owned());
        }
    }
    let bundle = app[0]["bundle_id"].clone();
    app.extend(extensions);
    for target in &app {
        let path = root
            .join(
                target["project"]
                    .as_str()
                    .context("Target project missing")?,
            )
            .join("project.pbxproj");
        if path.exists() {
            let text = fs::read_to_string(&path)?;
            let parsed = crate::project::parse(&text)?;
            for node in parsed.get("objects")?.map()?.values() {
                let object = node.map()?;
                if object.get("isa").and_then(|n| n.text().ok()) == Some("PBXNativeTarget")
                    && object.get("productType").and_then(|n| n.text().ok())
                        == Some("com.apple.product-type.bundle.unit-test")
                {
                    let name = node.get("name")?.text()?.to_owned();
                    if !tests.contains(&name) {
                        tests.push(name);
                    }
                }
            }
        }
    }
    let mut configurations = json!({"test":"Debug","archive":"Release"});
    let scheme_file = root
        .join(project)
        .join("xcshareddata/xcschemes")
        .join(format!("{scheme}.xcscheme"));
    if scheme_file.exists() {
        fsutil::confined(root, &scheme_file)?;
        let xml = fs::read_to_string(scheme_file)?;
        for (action, key) in [("ArchiveAction", "archive"), ("TestAction", "test")] {
            let pattern = regex::Regex::new(&format!(
                r#"(?s)<{action}\b[^>]*\bbuildConfiguration\s*=\s*"([^"]+)""#
            ))?;
            if let Some(captures) = pattern.captures(&xml) {
                configurations[key] = json!(&captures[1]);
            }
        }
    }
    let mut config = json!({"schema_version":2,"scheme":scheme,"team_id":team,"test_targets":tests,"test_device":device,"xcode":xcode,"targets":app,"locales":["en-US"],"localization_catalogs":catalogs,"localization_locales":["en"],"screenshot_devices":[device],"screens":[],"signing":{"vault":".ios-release/signing.vault"},"configurations":{"test":"Debug","archive":"Release"},"app_store":{"bundle_id":bundle,"release_type":"MANUAL","phased_release":false,"testflight_groups":[]},"metadata_path":"store/metadata","screenshots_path":"store/screenshots"});
    config[kind] = json!(project);
    config["configurations"] = configurations;
    if let Some(repo) = &options.repository {
        config["repository"] = json!(repo);
    } else if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        config["repository"] = json!(repo);
    }
    Ok(config)
}
pub fn capabilities(entitlements: &Value) -> Vec<&'static str> {
    [
        ("aps-environment", "PUSH_NOTIFICATIONS"),
        (
            "com.apple.developer.associated-domains",
            "ASSOCIATED_DOMAINS",
        ),
        ("com.apple.security.application-groups", "APP_GROUPS"),
        ("com.apple.developer.icloud-container-identifiers", "ICLOUD"),
        ("com.apple.developer.applesignin", "APPLE_ID_AUTH"),
        ("com.apple.developer.in-app-payments", "APPLE_PAY"),
    ]
    .into_iter()
    .filter_map(|(key, value)| entitlements.get(key).map(|_| value))
    .collect()
}
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if ![
                    ".git",
                    ".ios-release",
                    "build",
                    "target",
                    "Pods",
                    ".build",
                    "node_modules",
                ]
                .contains(&entry.file_name().to_string_lossy().as_ref())
                    && !path
                        .extension()
                        .is_some_and(|e| e == "xcodeproj" || e == "xcworkspace")
                {
                    pending.push(path);
                }
            } else if kind.is_file() {
                paths.push(path);
            }
        }
        ensure!(paths.len() < 100000, "App source inventory is too large");
    }
    Ok(paths)
}
pub fn init(
    root: &Path,
    options: &Options,
    executor: &mut impl Executor,
    plan: bool,
) -> Result<Value> {
    let root = root.canonicalize()?;
    let config_path = root.join(".ios-release.json");
    if config_path.exists() {
        let mut app = App::load(&root, Path::new(".ios-release.json"))?;
        ensure!(
            app.config["schema_version"] == 2,
            "Use the existing v1 platform entrypoint for legacy configuration"
        );
        if let Some(team) = &options.team {
            app.config["team_id"] = json!(team);
        }
        for target in app.config["targets"].as_array_mut().unwrap() {
            if let Some(value) = options.tracking {
                target["tracking"] = json!(value);
            }
            if let Some(value) = options.non_exempt_encryption {
                target["non_exempt_encryption"] = json!(value);
            }
        }
        if !plan {
            app.save()?;
        }
        return Ok(app.config);
    }
    if !plan && !options.no_workflows {
        ensure!(
            options.platform_revision.len() == 40
                && options
                    .platform_revision
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit()),
            "Pass a reviewed --platform-revision when using an unpackaged development binary"
        );
        ensure!(
            !root.join(".github/workflows/ios-native.yml").exists(),
            "Existing native workflow will not be overwritten"
        );
    }
    let (kind, project) = match (&options.project, &options.workspace) {
        (Some(p), None) => ("project", p.clone()),
        (None, Some(p)) => ("workspace", p.clone()),
        (None, None) => {
            let values = fs::read_dir(&root)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e == "xcodeproj" || e == "xcworkspace")
                })
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let p = choose("project", &values)?;
            (
                if p.ends_with(".xcworkspace") {
                    "workspace"
                } else {
                    "project"
                },
                p,
            )
        }
        _ => bail!("Choose a project or workspace, not both"),
    };
    relative_path(&project)?;
    if plan {
        return Ok(
            json!({"operation":"init","project":project,"configuration":".ios-release.json","discover":"shared schemes, build settings, Xcode, simulators and signing targets","writes":[".ios-release.json","store/metadata/en-US",".github/workflows/ios-native.yml"],"apple_mutations":false}),
        );
    }
    let (xcode, devices) = discover_xcode(&root, options.xcode.as_deref(), executor)?;
    let developer = xcode["path"].as_str().unwrap();
    let schemes: Value = serde_json::from_str(&checked(
        executor,
        &Step::new(
            "xcodebuild",
            [
                format!("-{kind}"),
                project.clone(),
                "-list".into(),
                "-json".into(),
            ],
            60,
        )
        .developer(developer),
        &root,
    )?)?;
    let scheme = if let Some(s) = &options.scheme {
        s.clone()
    } else {
        choose(
            "scheme",
            &schemes[kind]["schemes"]
                .as_array()
                .context("No shared schemes")?
                .iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect::<Vec<_>>(),
        )?
    };
    let settings: Value = serde_json::from_str(&checked(
        executor,
        &Step::new(
            "xcodebuild",
            [
                format!("-{kind}"),
                project.clone(),
                "-scheme".into(),
                scheme.clone(),
                "-showBuildSettings".into(),
                "-json".into(),
            ],
            120,
        )
        .developer(developer),
        &root,
    )?)?;
    let mut config = config_from_settings(
        &root, kind, &project, &scheme, xcode, &devices, &settings, options,
    )?;
    if config["repository"].is_null()
        && let Ok(remote) = checked(
            executor,
            &Step::new("git", ["remote", "get-url", "origin"], 30),
            &root,
        )
    {
        let pattern = regex::Regex::new(
            r"^(?:https://github\.com/|git@github\.com:)([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?$",
        )?;
        if let Some(captures) = pattern.captures(remote.trim()) {
            config["repository"] = json!(&captures[1]);
        }
    }
    fsutil::json(&config_path, &config)?;
    let metadata = root.join("store/metadata/en-US");
    fs::create_dir_all(&metadata)?;
    for (name, value) in [
        ("name.txt", format!("{scheme}\n")),
        ("description.txt", "Describe what your app does.\n".into()),
        ("keywords.txt", "".into()),
        ("support_url.txt", "".into()),
        ("privacy_url.txt", "".into()),
        ("release_notes.txt", "Initial release.\n".into()),
    ] {
        let path = metadata.join(name);
        if !path.exists() {
            fsutil::atomic(&path, value.as_bytes())?;
        }
    }
    let ignore = root.join(".gitignore");
    let mut text = fs::read_to_string(&ignore).unwrap_or_default();
    for pattern in [
        "build/",
        ".ios-release/project-backups/",
        "*.p8",
        "*.p12",
        "*.mobileprovision",
    ] {
        if !text.lines().any(|l| l == pattern) {
            text.push_str(&format!("\n{pattern}\n"));
        }
    }
    fsutil::atomic(&ignore, text.as_bytes())?;
    if !options.no_workflows {
        write_workflow(&root, &options.platform_revision)?;
    }
    println!(
        "Initialized {scheme}. Next: ios-release auth login, ios-release signing sync, then ios-release release prepare. Review encryption/tracking declarations and fill store/metadata before submission."
    );
    Ok(config)
}
pub fn write_workflow(root: &Path, revision: &str) -> Result<()> {
    ensure!(
        revision.len() == 40 && revision.bytes().all(|c| c.is_ascii_hexdigit()),
        "GitHub Actions must pin a full reviewed platform commit; pass --platform-revision"
    );
    let path = root.join(".github/workflows/ios-native.yml");
    ensure!(
        !path.exists(),
        "Existing native workflow will not be overwritten"
    );
    let text = include_str!("../assets/native-call.yml").replace("__PLATFORM_REVISION__", revision);
    fsutil::atomic(&path, text.as_bytes())
}
pub fn auth_home() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("Missing home")?)
            .join(".config/ios-release"),
    )
}
pub fn login(key_id: &str, issuer: &str, key_file: &Path) -> Result<()> {
    let pem = fs::read_to_string(key_file)
        .context("Cannot read the .p8 file downloaded from App Store Connect")?;
    crate::api::Credentials::from_pem(key_id.into(), issuer.into(), &pem)?;
    let home = auth_home()?;
    fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    }
    let target = home.join(format!("AuthKey_{key_id}.p8"));
    fsutil::atomic(&target, pem.as_bytes())?;
    fsutil::json(
        &home.join("auth.json"),
        &json!({"key_id":key_id,"issuer_id":issuer,"key_file":target}),
    )?;
    println!(
        "Apple API credentials saved privately for this user. They will not be committed or printed."
    );
    Ok(())
}
pub fn local_credentials() -> Result<Value> {
    serde_json::from_slice(
        &fs::read(auth_home()?.join("auth.json"))
            .context("Run ios-release auth login with your App Store Connect team API key")?,
    )
    .context("Invalid local credential profile")
}
pub fn load_app(root: &Path) -> Result<App> {
    App::load(root, Path::new(".github/ios-release.json"))
}
