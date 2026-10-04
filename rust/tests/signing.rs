use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use ios_release_native::{
    api::{Api, HttpError},
    config::App,
    fsutil,
    process::{Native, Step, checked},
    signing, vault,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
const PASSWORD: &str = "test signing vault password";
fn app() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({"schema_version":2,"project":"Example.xcodeproj","scheme":"Example","team_id":"ABCDE12345","targets":[{"name":"Example","bundle_id":"dev.example.app","profile":"","entitlements":{},"tracking":false,"non_exempt_encryption":false}],"app_store":{"bundle_id":"dev.example.app"}});
    fsutil::json(&dir.path().join(".ios-release.json"), &config).unwrap();
    let app = App::load(dir.path(), Path::new(".ios-release.json")).unwrap();
    (dir, app)
}
fn future(days: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::days(days))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}
struct SigningServer {
    root: PathBuf,
    temp: tempfile::TempDir,
    certificates: Vec<Value>,
    profiles: Vec<Value>,
    lost: Option<String>,
    counts: BTreeMap<String, usize>,
}
impl SigningServer {
    fn new(app: &App) -> Self {
        Self {
            root: app.root.clone(),
            temp: tempfile::tempdir().unwrap(),
            certificates: vec![],
            profiles: vec![],
            lost: None,
            counts: BTreeMap::new(),
        }
    }
    fn identity(&mut self) -> Result<Value> {
        let vault = vault::read(&self.root.join(".ios-release/signing.vault"), PASSWORD)?;
        let key = self.temp.path().join("key.pem");
        let pem = self.temp.path().join("certificate.pem");
        let der = self.temp.path().join("certificate.der");
        fsutil::atomic(&key, vault["private_key_pem"].as_str().unwrap().as_bytes())?;
        checked(
            &mut Native,
            &Step::new(
                "openssl",
                [
                    "req",
                    "-new",
                    "-x509",
                    "-key",
                    key.to_str().unwrap(),
                    "-out",
                    pem.to_str().unwrap(),
                    "-days",
                    "365",
                    "-subj",
                    "/OU=ABCDE12345/CN=Fixture Distribution",
                ],
                30,
            ),
            self.temp.path(),
        )?;
        checked(
            &mut Native,
            &Step::new(
                "openssl",
                [
                    "x509",
                    "-in",
                    pem.to_str().unwrap(),
                    "-outform",
                    "DER",
                    "-out",
                    der.to_str().unwrap(),
                ],
                30,
            ),
            self.temp.path(),
        )?;
        Ok(
            json!({"type":"certificates","id":format!("cert-{}",self.certificates.len()+1),"attributes":{"certificateType":"DISTRIBUTION","certificateContent":STANDARD.encode(fs::read(der)?),"expirationDate":future(365)}}),
        )
    }
    fn profile(&mut self, body: &Value) -> Result<Value> {
        let id = format!("profile-{}", self.profiles.len() + 1);
        let uuid = uuid::Uuid::new_v4().to_string();
        let name = body["data"]["attributes"]["name"].as_str().unwrap();
        let cert_id = body["data"]["relationships"]["certificates"]["data"][0]["id"]
            .as_str()
            .unwrap();
        let certificate = self
            .certificates
            .iter()
            .find(|c| c["id"] == cert_id)
            .unwrap();
        let mut dict = plist::Dictionary::new();
        dict.insert("UUID".into(), uuid.clone().into());
        dict.insert("Name".into(), name.into());
        dict.insert(
            "TeamIdentifier".into(),
            plist::Value::Array(vec!["ABCDE12345".into()]),
        );
        dict.insert(
            "ExpirationDate".into(),
            plist::Value::Date((SystemTime::now() + Duration::from_secs(365 * 86400)).into()),
        );
        dict.insert(
            "DeveloperCertificates".into(),
            plist::Value::Array(vec![plist::Value::Data(
                STANDARD.decode(
                    certificate["attributes"]["certificateContent"]
                        .as_str()
                        .unwrap(),
                )?,
            )]),
        );
        let mut entitlements = plist::Dictionary::new();
        entitlements.insert(
            "application-identifier".into(),
            "ABCDE12345.dev.example.app".into(),
        );
        entitlements.insert("get-task-allow".into(), false.into());
        dict.insert("Entitlements".into(), entitlements.into());
        let decoded = self.temp.path().join("profile.plist");
        let cms = self.temp.path().join("profile.cms");
        plist::Value::Dictionary(dict).to_file_xml(&decoded)?;
        checked(
            &mut Native,
            &Step::new(
                "openssl",
                [
                    "smime",
                    "-sign",
                    "-binary",
                    "-nodetach",
                    "-signer",
                    self.temp.path().join("certificate.pem").to_str().unwrap(),
                    "-inkey",
                    self.temp.path().join("key.pem").to_str().unwrap(),
                    "-in",
                    decoded.to_str().unwrap(),
                    "-outform",
                    "DER",
                    "-out",
                    cms.to_str().unwrap(),
                ],
                30,
            ),
            self.temp.path(),
        )?;
        Ok(
            json!({"type":"profiles","id":id,"attributes":{"uuid":uuid,"name":name,"profileType":"IOS_APP_STORE","profileState":"ACTIVE","expirationDate":future(365),"profileContent":STANDARD.encode(fs::read(cms)?)},"relationships":body["data"]["relationships"]}),
        )
    }
    fn count(&self, key: &str) -> usize {
        *self.counts.get(key).unwrap_or(&0)
    }
}
impl Api for SigningServer {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        support::validate(method, path, query, body)?;
        let route = format!("{method} {path}");
        *self.counts.entry(route.clone()).or_default() += 1;
        let value = match (method, path) {
            ("GET", "/v1/certificates") => json!({"data":self.certificates}),
            ("POST", "/v1/certificates") => {
                let cert = self.identity()?;
                self.certificates.push(cert.clone());
                json!({"data":cert})
            }
            ("GET", "/v1/bundleIds") => {
                json!({"data":[{"id":"bundle","attributes":{"identifier":"dev.example.app","platform":"IOS"}}]})
            }
            ("GET", "/v1/profiles") => {
                let name = query
                    .iter()
                    .find(|(k, _)| k == "filter[name]")
                    .unwrap()
                    .1
                    .as_str();
                json!({"data":self.profiles.iter().filter(|p|p["attributes"]["name"]==name).collect::<Vec<_>>()})
            }
            ("POST", "/v1/profiles") => {
                let profile = self.profile(body.unwrap())?;
                self.profiles.push(profile.clone());
                json!({"data":profile})
            }
            ("GET", path) if path.starts_with("/v1/profiles/") => {
                let id = path.strip_prefix("/v1/profiles/").unwrap();
                if let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|p| p["id"] == id && p["attributes"]["profileState"] != "DELETED")
                {
                    json!({"data":profile})
                } else {
                    return Err(HttpError {
                        status: 404,
                        method: method.into(),
                        path: path.into(),
                        codes: "NOT_FOUND".into(),
                    }
                    .into());
                }
            }
            _ => bail!("Unexpected signing request {route}"),
        };
        if self.lost.as_deref() == Some(&route) {
            self.lost = None;
            bail!("Fixture lost response");
        }
        Ok(value)
    }
    fn transfer(&mut self, _: &Path, _: &Value) -> Result<()> {
        bail!("Signing must not upload assets")
    }
}
#[test]
fn certificates_and_profiles_reconcile_after_lost_post_responses() {
    for lost in ["POST /v1/certificates", "POST /v1/profiles"] {
        let (_root, mut app) = app();
        let mut server = SigningServer::new(&app);
        server.lost = Some(lost.into());
        assert!(signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).is_err());
        signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
        assert_eq!(server.count("POST /v1/certificates"), 1);
        assert_eq!(server.count("POST /v1/profiles"), 1);
        assert!(
            !app.config["targets"][0]["profile"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn repeated_sync_reuses_valid_assets_and_new_runner_state() {
    let (_root, mut app) = app();
    let original = app.clone();
    let mut server = SigningServer::new(&app);
    signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
    let saved = vault::read(&signing::path(&app).unwrap(), PASSWORD).unwrap();
    let old = json!({"schema_version":1,"team_id":saved["team_id"],"bundle_ids":saved["bundle_ids"],"private_key_pem":saved["private_key_pem"],"certificate":null,"profiles":{},"profile_intents":{}});
    vault::write(&signing::path(&app).unwrap(), &old, PASSWORD).unwrap();
    app = original;
    signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
    assert_eq!(server.count("POST /v1/certificates"), 1);
    assert_eq!(server.count("POST /v1/profiles"), 1);
}
#[test]
fn missing_or_expiring_profiles_renew_without_revoking_certificates() {
    for state in ["DELETED", "EXPIRING"] {
        let (_root, mut app) = app();
        let mut server = SigningServer::new(&app);
        signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
        if state == "DELETED" {
            server.profiles[0]["attributes"]["profileState"] = json!("DELETED");
        } else {
            server.profiles[0]["attributes"]["expirationDate"] = json!(future(29));
        }
        signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
        assert_eq!(server.count("POST /v1/certificates"), 1);
        assert_eq!(server.count("POST /v1/profiles"), 2);
        assert_eq!(server.count("DELETE /v1/certificates/cert-1"), 0);
    }
}
#[test]
fn expiring_owned_certificate_gets_a_new_profile_without_revocation() {
    let (_root, mut app) = app();
    let mut server = SigningServer::new(&app);
    signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
    server.certificates[0]["attributes"]["expirationDate"] = json!(future(29));
    signing::sync_with_password(&mut app, &mut server, &mut Native, PASSWORD).unwrap();
    assert_eq!(server.count("POST /v1/certificates"), 2);
    assert_eq!(server.count("POST /v1/profiles"), 2);
    assert_eq!(server.count("DELETE /v1/certificates/cert-1"), 0);
}
#[test]
fn profile_contents_reject_wrong_team_bundle_certificate_and_debugging() {
    let (_root, app) = app();
    let profile = json!({"attributes":{"uuid":"UUID","name":"Profile"}});
    let cert = json!({"attributes":{"certificateContent":STANDARD.encode(b"certificate")}});
    let mut dict = plist::Dictionary::new();
    dict.insert("UUID".into(), "UUID".into());
    dict.insert("Name".into(), "Profile".into());
    dict.insert(
        "TeamIdentifier".into(),
        plist::Value::Array(vec!["ABCDE12345".into()]),
    );
    dict.insert(
        "ExpirationDate".into(),
        plist::Value::Date((SystemTime::now() + Duration::from_secs(86400)).into()),
    );
    dict.insert(
        "DeveloperCertificates".into(),
        plist::Value::Array(vec![plist::Value::Data(b"certificate".to_vec())]),
    );
    let mut entitlements = plist::Dictionary::new();
    entitlements.insert(
        "application-identifier".into(),
        "ABCDE12345.dev.example.app".into(),
    );
    entitlements.insert("get-task-allow".into(), false.into());
    dict.insert("Entitlements".into(), entitlements.into());
    let valid = plist::Value::Dictionary(dict);
    assert!(
        signing::validate_profile_plist(&app, &app.config["targets"][0], &profile, &cert, &valid)
            .is_ok()
    );
    for field in [
        "TeamIdentifier",
        "DeveloperCertificates",
        "ExpirationDate",
        "ProvisionedDevices",
    ] {
        let mut invalid = valid.clone();
        invalid
            .as_dictionary_mut()
            .unwrap()
            .insert(field.into(), "wrong".into());
        assert!(
            signing::validate_profile_plist(
                &app,
                &app.config["targets"][0],
                &profile,
                &cert,
                &invalid
            )
            .is_err()
        );
    }
    for (key, value) in [
        (
            "application-identifier",
            plist::Value::String("ABCDE12345.wrong.app".into()),
        ),
        ("get-task-allow", plist::Value::Boolean(true)),
    ] {
        let mut invalid = valid.clone();
        invalid
            .as_dictionary_mut()
            .unwrap()
            .get_mut("Entitlements")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .insert(key.into(), value);
        assert!(
            signing::validate_profile_plist(
                &app,
                &app.config["targets"][0],
                &profile,
                &cert,
                &invalid
            )
            .is_err()
        );
    }
}
mod support;
