use crate::{
    api::{self, Api},
    config::App,
    fsutil,
    process::{Arg, Executor, Step, checked},
    vault,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn current(value: &str, days: i64) -> bool {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .is_ok_and(|date| date > time::OffsetDateTime::now_utc() + time::Duration::days(days))
}
pub fn password() -> Result<String> {
    if let Ok(password) = std::env::var("IOS_RELEASE_SIGNING_PASSWORD") {
        return Ok(password);
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return rpassword::prompt_password(
            "Signing vault password (keep this in your password manager): ",
        )
        .context("Cannot read signing password");
    }
    anyhow::bail!(
        "Set IOS_RELEASE_SIGNING_PASSWORD to a unique password of at least 16 characters; keep it in your password manager and signing environment"
    )
}
pub fn path(app: &App) -> Result<PathBuf> {
    let relative = app.config["signing"]["vault"]
        .as_str()
        .unwrap_or(".ios-release/signing.vault");
    let path = app.root.join(crate::config::relative_path(relative)?);
    fsutil::confined(&app.root, &path)?;
    Ok(path)
}
pub fn validate_vault(app: &App, value: &Value) -> Result<()> {
    ensure!(
        value["schema_version"] == 1 && value["team_id"] == app.config["team_id"],
        "Signing vault belongs to another Apple team"
    );
    let expected = app.config["targets"]
        .as_array()
        .context("Application targets are required")?
        .iter()
        .map(|t| t["bundle_id"].clone())
        .collect::<Vec<_>>();
    ensure!(
        value["bundle_ids"] == json!(expected),
        "Signing vault belongs to another app or target set"
    );
    ensure!(
        value["private_key_pem"]
            .as_str()
            .is_some_and(|s| s.contains("PRIVATE KEY")),
        "Signing vault has no private signing key"
    );
    Ok(())
}
fn pem_public(executor: &mut impl Executor, root: &Path, key: &Path) -> Result<String> {
    checked(
        executor,
        &Step::new(
            "openssl",
            ["pkey", "-in", key.to_str().unwrap(), "-pubout"],
            30,
        ),
        root,
    )
}
fn certificate_public(executor: &mut impl Executor, root: &Path, der: &Path) -> Result<String> {
    checked(
        executor,
        &Step::new(
            "openssl",
            [
                "x509",
                "-inform",
                "DER",
                "-in",
                der.to_str().unwrap(),
                "-pubkey",
                "-noout",
            ],
            30,
        ),
        root,
    )
}
fn bind_certificate(
    executor: &mut impl Executor,
    root: &Path,
    temp: &Path,
    key: &Path,
    cert: &Value,
    team: &str,
) -> Result<()> {
    let content = STANDARD.decode(
        cert["attributes"]["certificateContent"]
            .as_str()
            .context("Apple returned no certificate content")?,
    )?;
    let der = temp.join("certificate.der");
    fsutil::atomic(&der, &content)?;
    ensure!(
        pem_public(executor, root, key)?.trim() == certificate_public(executor, root, &der)?.trim(),
        "Apple certificate does not match the owned private key"
    );
    let subject = checked(
        executor,
        &Step::new(
            "openssl",
            [
                "x509",
                "-inform",
                "DER",
                "-in",
                der.to_str().unwrap(),
                "-subject",
                "-nameopt",
                "RFC2253",
                "-noout",
            ],
            30,
        ),
        root,
    )?;
    let subject = subject
        .trim()
        .strip_prefix("subject=")
        .unwrap_or(subject.trim())
        .trim();
    let ou = regex::Regex::new(r"(?:^|,)OU=([A-Za-z0-9]+)(?:,|$)")?;
    ensure!(
        ou.captures(subject).is_some_and(|m| &m[1] == team),
        "Distribution certificate belongs to another Apple team; check team_id and API credentials before continuing"
    );
    Ok(())
}
fn certificate_candidates(api: &mut impl Api) -> Result<Vec<Value>> {
    api::list(
        api,
        "/v1/certificates",
        &api::query(&[
            ("filter[certificateType]", "DISTRIBUTION"),
            ("limit", "200"),
        ]),
    )
}

/// Sync owns only its vault and generated resources. It never revokes a certificate.
pub fn sync(app: &mut App, api: &mut impl Api, executor: &mut impl Executor) -> Result<Value> {
    sync_with_password(app, api, executor, &password()?)
}
pub fn sync_with_password(
    app: &mut App,
    api: &mut impl Api,
    executor: &mut impl Executor,
    password: &str,
) -> Result<Value> {
    app.require_signing()?;
    ensure!(
        password.len() >= 16,
        "Signing vault password must contain at least 16 characters"
    );
    let vault_path = path(app)?;
    let temp = tempfile::tempdir()?;
    let key = temp.path().join("private.pem");
    let csr = temp.path().join("request.csr");
    let mut state = if vault_path.exists() {
        vault::read(&vault_path, password)?
    } else {
        checked(
            executor,
            &Step::new(
                "openssl",
                ["genrsa", "-out", key.to_str().unwrap(), "2048"],
                30,
            ),
            &app.root,
        )?;
        let private = fs::read_to_string(&key)?;
        json!({"schema_version":1,"team_id":app.config["team_id"],"bundle_ids":app.config["targets"].as_array().unwrap().iter().map(|t|t["bundle_id"].clone()).collect::<Vec<_>>(),"private_key_pem":private,"certificate":null,"profiles":{},"profile_intents":{}})
    };
    validate_vault(app, &state)?;
    fsutil::atomic(&key, state["private_key_pem"].as_str().unwrap().as_bytes())?;
    checked(
        executor,
        &Step::new(
            "openssl",
            [
                "req",
                "-new",
                "-key",
                key.to_str().unwrap(),
                "-out",
                csr.to_str().unwrap(),
                "-subj",
                "/CN=ios-release distribution",
            ],
            30,
        ),
        &app.root,
    )?;
    // Persist the private key before the first POST. A lost response can be reconciled by public key.
    vault::write(&vault_path, &state, password)?;
    let candidates = certificate_candidates(api)?;
    let mut matching = Vec::new();
    let public = pem_public(executor, &app.root, &key)?;
    for cert in candidates {
        if current(
            cert["attributes"]["expirationDate"].as_str().unwrap_or(""),
            30,
        ) {
            let der = temp.path().join("candidate.der");
            fsutil::atomic(
                &der,
                &STANDARD.decode(
                    cert["attributes"]["certificateContent"]
                        .as_str()
                        .context("Certificate content missing")?,
                )?,
            )?;
            if certificate_public(executor, &app.root, &der)?.trim() == public.trim() {
                matching.push(cert);
            }
        }
    }
    if matching.len() > 1
        && let Some(owned) = state["certificate"]["id"].as_str()
        && matching.iter().any(|c| c["id"] == owned)
    {
        matching.retain(|c| c["id"] == owned);
    }
    ensure!(
        matching.len() <= 1,
        "More than one usable certificate matches this vault; select the owned certificate explicitly"
    );
    let certificate = if let Some(cert) = matching.pop() {
        cert
    } else {
        api.request("POST","/v1/certificates",&[],Some(&json!({"data":{"type":"certificates","attributes":{"certificateType":"DISTRIBUTION","csrContent":fs::read_to_string(&csr)?}}})))?["data"].clone()
    };
    ensure!(
        current(
            certificate["attributes"]["expirationDate"]
                .as_str()
                .unwrap_or(""),
            0
        ),
        "Apple certificate is expired or has no expiration"
    );
    bind_certificate(
        executor,
        &app.root,
        temp.path(),
        &key,
        &certificate,
        app.text("team_id")?,
    )?;
    let certificate_id = api::id(&certificate)?.to_owned();
    state["certificate"] = certificate.clone();
    vault::write(&vault_path, &state, password)?;
    let mut result = Vec::new();
    let targets = app.config["targets"].as_array().unwrap().clone();
    for (index, target) in targets.iter().enumerate() {
        let bundle = target["bundle_id"].as_str().unwrap();
        let mut bundles = api::list(
            api,
            "/v1/bundleIds",
            &api::query(&[("filter[identifier]", bundle), ("limit", "200")]),
        )?;
        bundles.retain(|b| b["attributes"]["identifier"] == bundle);
        ensure!(bundles.len() <= 1, "Apple returned ambiguous bundle IDs");
        let record = if let Some(b) = bundles.pop() {
            b
        } else {
            api.request("POST","/v1/bundleIds",&[],Some(&json!({"data":{"type":"bundleIds","attributes":{"identifier":bundle,"name":target["name"].as_str().unwrap_or(bundle),"platform":"IOS"}}})))?["data"].clone()
        };
        let bundle_id = api::id(&record)?.to_owned();
        for capability in target["capabilities"].as_array().into_iter().flatten() {
            let kind = capability
                .as_str()
                .context("Bundle capabilities must be names")?;
            let existing = api::list(
                api,
                &format!("/v1/bundleIds/{bundle_id}/bundleIdCapabilities"),
                &[],
            )?;
            if !existing
                .iter()
                .any(|c| c["attributes"]["capabilityType"] == kind)
            {
                api.request("POST","/v1/bundleIdCapabilities",&[],Some(&json!({"data":{"type":"bundleIdCapabilities","attributes":{"capabilityType":kind},"relationships":{"bundleId":{"data":{"type":"bundleIds","id":bundle_id}}}}})))?;
            }
        }
        let cached = &state["profiles"][bundle];
        let mut profile = if let Some(id) = cached["id"].as_str() {
            let live = api::optional(
                api,
                &format!("/v1/profiles/{id}"),
                &api::query(&[("include", "bundleId,certificates")]),
            )?;
            live.filter(|v| profile_matches(v, &bundle_id, &certificate_id))
                .map(|live| live["data"].clone())
        } else {
            None
        };
        if profile.is_none()
            && let Some(name) = target["profile"].as_str().filter(|s| !s.is_empty())
        {
            let candidates = api::list(
                api,
                "/v1/profiles",
                &api::query(&[
                    ("filter[name]", name),
                    ("include", "bundleId,certificates"),
                    ("limit", "200"),
                ]),
            )?;
            let matches = candidates
                .into_iter()
                .filter(|p| profile_matches(&json!({"data":p}), &bundle_id, &certificate_id))
                .collect::<Vec<_>>();
            ensure!(
                matches.len() <= 1,
                "Configured profile name has multiple usable matches"
            );
            profile = matches.into_iter().next();
        }
        if profile.is_none() {
            let name = if let Some(name) = state["profile_intents"][bundle].as_str() {
                name.to_owned()
            } else {
                use sha2::Digest;
                let digest = fsutil::hex(
                    sha2::Sha256::digest(serde_json::to_vec(&target["entitlements"])?).as_ref(),
                );
                // Stable, certificate-bound names let fresh CI runs reconcile renewals
                // from the same encrypted private key without creating a profile per run.
                let name = format!("ios-release {bundle} {certificate_id} {}", &digest[..16]);
                state["profile_intents"][bundle] = json!(name);
                vault::write(&vault_path, &state, password)?;
                name
            };
            let values = api::list(
                api,
                "/v1/profiles",
                &api::query(&[
                    ("filter[name]", &name),
                    ("include", "bundleId,certificates"),
                    ("limit", "200"),
                ]),
            )?;
            let values = values
                .into_iter()
                .filter(|p| profile_matches(&json!({"data":p}), &bundle_id, &certificate_id))
                .collect::<Vec<_>>();
            ensure!(values.len() <= 1, "Ambiguous owned provisioning profile");
            if let Some(value) = values.first() {
                let live = api.request(
                    "GET",
                    &format!("/v1/profiles/{}", api::id(value)?),
                    &api::query(&[("include", "bundleId,certificates")]),
                    None,
                )?;
                ensure!(
                    profile_matches(&live, &bundle_id, &certificate_id),
                    "Owned provisioning profile changed; inspect signing status before renewing"
                );
                profile = Some(live["data"].clone());
            } else {
                profile=Some(api.request("POST","/v1/profiles",&[],Some(&json!({"data":{"type":"profiles","attributes":{"name":name,"profileType":"IOS_APP_STORE"},"relationships":{"bundleId":{"data":{"type":"bundleIds","id":bundle_id}},"certificates":{"data":[{"type":"certificates","id":certificate_id}]}}}})))?["data"].clone());
            }
        }
        let profile = profile.unwrap();
        let live = api.request(
            "GET",
            &format!("/v1/profiles/{}", api::id(&profile)?),
            &api::query(&[("include", "bundleId,certificates")]),
            None,
        )?;
        ensure!(
            profile_matches(&live, &bundle_id, &certificate_id),
            "Created profile does not bind this bundle and signing certificate"
        );
        let profile = live["data"].clone();
        ensure!(
            profile["attributes"]["profileState"] == "ACTIVE"
                && current(
                    profile["attributes"]["expirationDate"]
                        .as_str()
                        .unwrap_or(""),
                    0
                ),
            "Provisioning profile is not active"
        );
        validate_profile(app, target, &profile, &certificate, temp.path(), executor)?;
        ensure!(
            profile["attributes"]["profileContent"].as_str().is_some(),
            "Apple returned no provisioning profile"
        );
        app.config["targets"][index]["profile"] = profile["attributes"]["name"].clone();
        state["profiles"][bundle] = profile.clone();
        state["profile_intents"]
            .as_object_mut()
            .unwrap()
            .remove(bundle);
        vault::write(&vault_path, &state, password)?;
        result.push(json!({"bundle_id":bundle,"profile_id":profile["id"],"profile":profile["attributes"]["name"],"expires":profile["attributes"]["expirationDate"]}));
    }
    app.save()?;
    let report = json!({"certificate_id":certificate_id,"certificate_expires":certificate["attributes"]["expirationDate"],"profiles":result,"vault":vault_path,"renewal_window_days":30,"revoked_certificates":0});
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(report)
}
fn profile_matches(response: &Value, bundle: &str, certificate: &str) -> bool {
    let p = &response["data"];
    p["attributes"]["profileState"] == "ACTIVE"
        && p["attributes"]["profileType"] == "IOS_APP_STORE"
        && current(p["attributes"]["expirationDate"].as_str().unwrap_or(""), 30)
        && p["relationships"]["bundleId"]["data"]["id"] == bundle
        && p["relationships"]["certificates"]["data"]
            .as_array()
            .is_some_and(|v| v.len() == 1 && v[0]["id"] == certificate)
}
pub fn validate_profile_plist(
    app: &App,
    target: &Value,
    profile: &Value,
    certificate: &Value,
    decoded: &plist::Value,
) -> Result<()> {
    let dict = decoded
        .as_dictionary()
        .context("Malformed provisioning profile")?;
    let text = |key: &str| dict.get(key).and_then(plist::Value::as_string);
    ensure!(
        text("UUID") == profile["attributes"]["uuid"].as_str()
            && text("Name") == profile["attributes"]["name"].as_str(),
        "Provisioning profile identity differs from Apple resource"
    );
    ensure!(
        dict.get("ExpirationDate")
            .and_then(plist::Value::as_date)
            .is_some_and(|d| std::time::SystemTime::from(d) > std::time::SystemTime::now()),
        "Provisioning profile expired"
    );
    ensure!(
        dict.get("TeamIdentifier")
            .and_then(plist::Value::as_array)
            .is_some_and(
                |ids| ids.len() == 1 && ids[0].as_string() == app.config["team_id"].as_str()
            ),
        "Profile belongs to another Apple team"
    );
    ensure!(
        !dict.contains_key("ProvisionedDevices")
            && dict
                .get("ProvisionsAllDevices")
                .is_none_or(|v| v.as_boolean() == Some(false)),
        "Profile is not for App Store distribution"
    );
    let entitlements = dict
        .get("Entitlements")
        .and_then(plist::Value::as_dictionary)
        .context("Profile has no entitlements")?;
    ensure!(
        entitlements
            .get("application-identifier")
            .and_then(plist::Value::as_string)
            == Some(&format!(
                "{}.{}",
                app.text("team_id")?,
                target["bundle_id"].as_str().context("Missing bundle ID")?
            ))
            && entitlements
                .get("get-task-allow")
                .is_none_or(|v| v.as_boolean() == Some(false)),
        "Profile does not authorize this app for distribution"
    );
    let der = STANDARD.decode(
        certificate["attributes"]["certificateContent"]
            .as_str()
            .context("Certificate content missing")?,
    )?;
    ensure!(
        dict.get("DeveloperCertificates")
            .and_then(plist::Value::as_array)
            .is_some_and(|certs| certs.len() == 1 && certs[0].as_data() == Some(der.as_slice())),
        "Profile does not bind the owned certificate"
    );
    let actual = serde_json::to_value(plist::Value::Dictionary(entitlements.clone()))?;
    for (key, value) in target["entitlements"]
        .as_object()
        .context("Declare target entitlements")?
    {
        let permitted = &actual[key];
        let authorized = permitted == value
            || (permitted.as_str() == Some("*") && value.is_string())
            || (permitted.as_array().is_some_and(|p| {
                value.as_array().is_some_and(|wanted| {
                    wanted
                        .iter()
                        .all(|v| p.contains(v) || p.iter().any(|w| w.as_str() == Some("*")))
                })
            }));
        ensure!(
            authorized,
            "Profile does not authorize entitlement {key}; configure its app groups or containers in Apple Developer, then rerun signing sync"
        );
    }
    Ok(())
}
fn validate_profile(
    app: &App,
    target: &Value,
    profile: &Value,
    certificate: &Value,
    temp: &Path,
    executor: &mut impl Executor,
) -> Result<()> {
    let cms = temp.join("profile.mobileprovision");
    let decoded = temp.join("profile.plist");
    fsutil::atomic(
        &cms,
        &STANDARD.decode(
            profile["attributes"]["profileContent"]
                .as_str()
                .context("Profile content missing")?,
        )?,
    )?;
    checked(
        executor,
        &Step::new(
            "openssl",
            [
                "smime",
                "-verify",
                "-inform",
                "DER",
                "-noverify",
                "-binary",
                "-in",
                cms.to_str().unwrap(),
                "-out",
                decoded.to_str().unwrap(),
            ],
            30,
        ),
        &app.root,
    )?;
    validate_profile_plist(
        app,
        target,
        profile,
        certificate,
        &plist::Value::from_file(decoded)?,
    )
}
pub fn status(app: &App) -> Result<Value> {
    let value = vault::read(&path(app)?, &password()?)?;
    validate_vault(app, &value)?;
    let cert = &value["certificate"];
    Ok(
        json!({"certificate_id":cert["id"],"certificate_expires":cert["attributes"]["expirationDate"],"renewal_due":!current(cert["attributes"]["expirationDate"].as_str().unwrap_or(""),30),"profiles":value["profiles"].as_object().map(|p|p.iter().map(|(bundle,value)|json!({"bundle_id":bundle,"profile_id":value["id"],"expires":value["attributes"]["expirationDate"],"renewal_due":!current(value["attributes"]["expirationDate"].as_str().unwrap_or(""),30)})).collect::<Vec<_>>())}),
    )
}

/// Import an existing distribution identity without issuing or revoking a certificate.
pub fn import(
    app: &App,
    p12: &Path,
    password_env: &str,
    executor: &mut impl Executor,
) -> Result<()> {
    app.require_signing()?;
    let target = path(app)?;
    ensure!(
        !target.exists(),
        "A signing vault already exists; import will not replace its private key"
    );
    let password = password()?;
    ensure!(
        password.len() >= 16,
        "Signing vault password must contain at least 16 characters"
    );
    ensure!(
        std::env::var(password_env).is_ok(),
        "Set {password_env} to the password for the existing .p12 identity"
    );
    let temp = tempfile::tempdir()?;
    let extracted = temp.path().join("extracted.pem");
    let private = temp.path().join("private.pem");
    checked(
        executor,
        &Step::new(
            "openssl",
            [
                "pkcs12",
                "-in",
                p12.to_str().context("Invalid identity path")?,
                "-nocerts",
                "-nodes",
                "-passin",
                &format!("env:{password_env}"),
                "-out",
                extracted.to_str().unwrap(),
            ],
            30,
        ),
        &app.root,
    )?;
    checked(
        executor,
        &Step::new(
            "openssl",
            [
                "pkey",
                "-in",
                extracted.to_str().unwrap(),
                "-out",
                private.to_str().unwrap(),
            ],
            30,
        ),
        &app.root,
    )?;
    let value = json!({"schema_version":1,"team_id":app.config["team_id"],"bundle_ids":app.config["targets"].as_array().unwrap().iter().map(|t|t["bundle_id"].clone()).collect::<Vec<_>>(),"private_key_pem":fs::read_to_string(private)?,"certificate":null,"profiles":{},"profile_intents":{}});
    vault::write(&target, &value, &password)?;
    println!(
        "Imported the existing private identity into the encrypted vault. Run signing sync to reconcile its certificate and app profiles."
    );
    Ok(())
}

pub struct Installed {
    directory: tempfile::TempDir,
    keychain: PathBuf,
    profiles: Vec<(PathBuf, String)>,
    search_list_added: bool,
}
impl Installed {
    pub fn install(app: &App, executor: &mut impl Executor) -> Result<Self> {
        let state = vault::read(&path(app)?, &password()?)?;
        validate_vault(app, &state)?;
        ensure!(
            current(
                state["certificate"]["attributes"]["expirationDate"]
                    .as_str()
                    .unwrap_or(""),
                0
            ),
            "Signing certificate expired; run signing sync"
        );
        let directory = tempfile::tempdir()?;
        let keychain = directory.path().join("signing.keychain-db");
        let mut owned = Self {
            directory,
            keychain,
            profiles: vec![],
            search_list_added: false,
        };
        let private = owned.directory.path().join("private.pem");
        let der = owned.directory.path().join("certificate.der");
        let pem = owned.directory.path().join("certificate.pem");
        let p12 = owned.directory.path().join("identity.p12");
        fsutil::atomic(
            &private,
            state["private_key_pem"].as_str().unwrap().as_bytes(),
        )?;
        bind_certificate(
            executor,
            &app.root,
            owned.directory.path(),
            &private,
            &state["certificate"],
            app.text("team_id")?,
        )?;
        fsutil::atomic(
            &der,
            &STANDARD.decode(
                state["certificate"]["attributes"]["certificateContent"]
                    .as_str()
                    .context("Missing signing certificate")?,
            )?,
        )?;
        checked(
            executor,
            &Step::new(
                "openssl",
                [
                    "x509",
                    "-inform",
                    "DER",
                    "-in",
                    der.to_str().unwrap(),
                    "-out",
                    pem.to_str().unwrap(),
                ],
                30,
            ),
            &app.root,
        )?;
        let pass = uuid::Uuid::new_v4().to_string();
        let secret = |value: &str| Arg::Secret {
            secret_env: value.to_owned(),
        };
        let env = "IOS_RELEASE_TEMP_KEYCHAIN_PASSWORD";
        let mut export = Step::new(
            "openssl",
            [
                "pkcs12",
                "-export",
                "-inkey",
                private.to_str().unwrap(),
                "-in",
                pem.to_str().unwrap(),
                "-out",
                p12.to_str().unwrap(),
                "-passout",
                &format!("env:{env}"),
            ],
            30,
        );
        export.env.insert(env.into(), pass.clone());
        checked(executor, &export, &app.root)?;
        for args in [
            vec![
                "create-keychain".into(),
                "-p".into(),
                secret(env),
                owned.keychain.to_string_lossy().into_owned().into(),
            ],
            vec![
                "unlock-keychain".into(),
                "-p".into(),
                secret(env),
                owned.keychain.to_string_lossy().into_owned().into(),
            ],
            vec![
                "set-keychain-settings".into(),
                "-lut".into(),
                "21600".into(),
                owned.keychain.to_string_lossy().into_owned().into(),
            ],
            vec![
                "import".into(),
                p12.to_string_lossy().into_owned().into(),
                "-k".into(),
                owned.keychain.to_string_lossy().into_owned().into(),
                "-P".into(),
                secret(env),
                "-T".into(),
                "/usr/bin/codesign".into(),
            ],
            vec![
                "set-key-partition-list".into(),
                "-S".into(),
                "apple-tool:,apple:,codesign:".into(),
                "-s".into(),
                "-k".into(),
                secret(env),
                owned.keychain.to_string_lossy().into_owned().into(),
            ],
        ] {
            let mut step = Step::new("security", std::iter::empty::<String>(), 30);
            step.args = args;
            step.env.insert(env.into(), pass.clone());
            checked(executor, &step, &app.root)?;
        }
        let listed = checked(
            executor,
            &Step::new("security", ["list-keychains", "-d", "user"], 30),
            &app.root,
        )?;
        let mut paths = parse_keychains(&listed);
        paths.push(owned.keychain.to_string_lossy().into_owned());
        let args = ["list-keychains", "-d", "user", "-s"]
            .into_iter()
            .map(String::from)
            .chain(paths);
        checked(executor, &Step::new("security", args, 30), &app.root)?;
        owned.search_list_added = true;
        let home = PathBuf::from(std::env::var_os("HOME").context("Missing host home")?);
        let folder = home.join("Library/Developer/Xcode/UserData/Provisioning Profiles");
        fs::create_dir_all(&folder)?;
        for target_config in app.config["targets"].as_array().unwrap() {
            let profile = &state["profiles"][target_config["bundle_id"].as_str().unwrap()];
            validate_profile(
                app,
                target_config,
                profile,
                &state["certificate"],
                owned.directory.path(),
                executor,
            )?;
            let name = profile["attributes"]["uuid"]
                .as_str()
                .context("Profile UUID missing")?;
            crate::config::filename(name)?;
            let data = STANDARD.decode(
                profile["attributes"]["profileContent"]
                    .as_str()
                    .context("Profile content missing")?,
            )?;
            let target = folder.join(format!("{name}.mobileprovision"));
            if target.exists() {
                ensure!(
                    fs::read(&target)? == data,
                    "Installed profile UUID has different bytes"
                );
            } else {
                fsutil::atomic(&target, &data)?;
                owned
                    .profiles
                    .push((target.clone(), fsutil::sha256(&target)?));
            }
        }
        Ok(owned)
    }
    pub fn keychain(&self) -> &Path {
        &self.keychain
    }
}
fn parse_keychains(value: &str) -> Vec<String> {
    value
        .lines()
        .map(|line| line.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}
impl Drop for Installed {
    fn drop(&mut self) {
        // Remove only our entry, preserving keychains another process added while we ran.
        if self.search_list_added
            && let Ok(value) = std::process::Command::new("security")
                .args(["list-keychains", "-d", "user"])
                .output()
        {
            let paths = parse_keychains(&String::from_utf8_lossy(&value.stdout))
                .into_iter()
                .filter(|p| Path::new(p) != self.keychain);
            let _ = std::process::Command::new("security")
                .args(["list-keychains", "-d", "user", "-s"])
                .args(paths)
                .output();
        }
        let _ = std::process::Command::new("security")
            .args(["delete-keychain"])
            .arg(&self.keychain)
            .output();
        for (path, checksum) in &self.profiles {
            if fsutil::sha256(path).is_ok_and(|actual| actual == *checksum) {
                let _ = fs::remove_file(path);
            }
        }
    }
}
