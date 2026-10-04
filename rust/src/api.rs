//! Apple REST transport. Tokens and upload capabilities never enter logs.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::pkcs8::DecodePrivateKey;
use reqwest::blocking::{Body, Client};
use reqwest::{Method, Url};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const APPLE: &str = "https://api.appstoreconnect.apple.com";
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

pub trait Api {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        data: Option<&Value>,
    ) -> Result<Value>;
    fn transfer(&mut self, file: &Path, operations: &Value) -> Result<()>;
}

#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub method: String,
    pub path: String,
    pub codes: String,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Apple {} {}: HTTP {} {}. Check API role and account agreements; mutations are not retried automatically",
            self.method, self.path, self.status, self.codes
        )
    }
}
impl std::error::Error for HttpError {}
pub fn optional(
    api: &mut impl Api,
    path: &str,
    query: &[(String, String)],
) -> Result<Option<Value>> {
    match api.request("GET", path, query, None) {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error
                .downcast_ref::<HttpError>()
                .is_some_and(|e| e.status == 404) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub fn list(api: &mut impl Api, path: &str, query: &[(String, String)]) -> Result<Vec<Value>> {
    let mut page = api.request("GET", path, query, None)?;
    let mut values = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..100 {
        values.extend(
            page["data"]
                .as_array()
                .context("Apple returned an invalid resource list")?
                .iter()
                .cloned(),
        );
        let Some(next) = page["links"]["next"].as_str() else {
            return Ok(values);
        };
        let url = Url::parse(next).context("Invalid Apple pagination")?;
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("api.appstoreconnect.apple.com")
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "Apple pagination left its API origin"
        );
        ensure!(
            seen.insert(next.to_owned()),
            "Apple pagination repeated a page"
        );
        let path = url.path().to_owned();
        let query = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        page = api.request("GET", &path, &query, None)?;
    }
    bail!("Apple pagination exceeded its bound")
}

pub fn query(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn id(value: &Value) -> Result<&str> {
    let id = value["id"].as_str().context("Missing Apple resource ID")?;
    ensure!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "Invalid Apple resource ID"
    );
    Ok(id)
}

pub struct Credentials {
    pub key_id: String,
    pub issuer_id: String,
    key: SigningKey,
}

impl Credentials {
    pub fn load() -> Result<Self> {
        if std::env::var("IOS_RELEASE_API_KEY_ID").is_err()
            && std::env::var("APP_STORE_CONNECT_API_KEY_ID").is_err()
        {
            let saved = crate::onboarding::local_credentials()?;
            let key = std::fs::read_to_string(
                saved["key_file"]
                    .as_str()
                    .context("Local Apple key path missing")?,
            )?;
            return Self::from_pem(
                saved["key_id"]
                    .as_str()
                    .context("Local key ID missing")?
                    .into(),
                saved["issuer_id"]
                    .as_str()
                    .context("Local issuer ID missing")?
                    .into(),
                &key,
            );
        }
        fn setting(new: &str, legacy: &str) -> Result<String> {
            std::env::var(new)
                .or_else(|_| std::env::var(legacy))
                .with_context(|| format!("Set {new}; run ios-release auth login for local use"))
        }
        let key_id = setting("IOS_RELEASE_API_KEY_ID", "APP_STORE_CONNECT_API_KEY_ID")?;
        let issuer_id = setting(
            "IOS_RELEASE_API_ISSUER_ID",
            "APP_STORE_CONNECT_API_KEY_ISSUER_ID",
        )?;
        let pem = if let Ok(path) = std::env::var("IOS_RELEASE_API_KEY_FILE") {
            std::fs::read_to_string(path).context("Cannot read the Apple API private key file")?
        } else {
            setting(
                "IOS_RELEASE_API_KEY_CONTENT",
                "APP_STORE_CONNECT_API_KEY_CONTENT",
            )?
            .replace("\\n", "\n")
        };
        Self::from_pem(key_id, issuer_id, &pem)
    }
    pub fn from_pem(key_id: String, issuer_id: String, pem: &str) -> Result<Self> {
        ensure!(
            key_id.chars().all(|c| c.is_ascii_alphanumeric()) && !key_id.is_empty(),
            "Invalid API key ID"
        );
        ensure!(
            !issuer_id.is_empty(),
            "An Apple team API issuer ID is required"
        );
        let key = SigningKey::from_pkcs8_pem(pem)
            .context("Apple API key must be a valid P-256 .p8 private key")?;
        Ok(Self {
            key_id,
            issuer_id,
            key,
        })
    }
    pub fn token(&self, now: u64) -> Result<String> {
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(
            &json!({"alg":"ES256","kid":self.key_id,"typ":"JWT"}),
        )?);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"iss":self.issuer_id,"iat":now.saturating_sub(10),"exp":now+600,"aud":"appstoreconnect-v1"}))?);
        let message = format!("{header}.{payload}");
        let signature: Signature = self.key.sign(message.as_bytes());
        Ok(format!(
            "{message}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

pub struct Apple {
    client: Client,
    credentials: Credentials,
}
impl Apple {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(300))
                .user_agent(concat!("ios-release/", env!("CARGO_PKG_VERSION")))
                .build()?,
            credentials: Credentials::load()?,
        })
    }
}
impl Api for Apple {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        data: Option<&Value>,
    ) -> Result<Value> {
        ensure!(
            path.starts_with("/v1/") || path.starts_with("/v2/"),
            "Unexpected Apple API path"
        );
        let url = Url::parse(&format!("{APPLE}{path}"))?;
        ensure!(
            url.host_str() == Some("api.appstoreconnect.apple.com")
                && url.query().is_none()
                && url.fragment().is_none(),
            "Invalid Apple API path"
        );
        let method = Method::from_bytes(method.as_bytes())?;
        ensure!(
            [Method::GET, Method::POST, Method::PATCH, Method::DELETE].contains(&method),
            "Unsupported Apple API operation"
        );
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut request = self
            .client
            .request(method.clone(), url)
            .query(query)
            .bearer_auth(self.credentials.token(now)?);
        if let Some(data) = data {
            request = request.json(data);
        }
        // A lost mutation response is ambiguous: never automatically repeat a POST.
        let response = request.send().map_err(|_|anyhow::anyhow!("Apple {method} {path} did not return a response; reconcile status before repeating a mutation"))?;
        let status = response.status();
        let mut raw = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut raw)?;
        ensure!(
            raw.len() as u64 <= MAX_RESPONSE,
            "Apple API response exceeded its bound"
        );
        let value: Value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).context("Apple returned malformed JSON")?
        };
        if !status.is_success() {
            // Only public error codes, never response bodies containing credentials.
            let codes = value["errors"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e["code"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            return Err(HttpError {
                status: status.as_u16(),
                method: method.to_string(),
                path: path.into(),
                codes,
            }
            .into());
        }
        Ok(value)
    }
    fn transfer(&mut self, file: &Path, operations: &Value) -> Result<()> {
        let ranges = validate_operations(file.metadata()?.len(), operations)?;
        for operation in ranges {
            let url = Url::parse(operation["url"].as_str().context("Missing upload URL")?)?;
            let offset = operation["offset"].as_u64().unwrap();
            let length = operation["length"].as_u64().unwrap();
            let mut input = File::open(file)?;
            input.seek(SeekFrom::Start(offset))?;
            let mut request = self
                .client
                .put(url)
                .body(Body::sized(input.take(length), length));
            for header in operation["requestHeaders"].as_array().into_iter().flatten() {
                request = request.header(
                    header["name"].as_str().context("Invalid upload header")?,
                    header["value"].as_str().context("Invalid upload header")?,
                );
            }
            let response = request.send().map_err(|_| {
                anyhow::anyhow!("Apple asset transfer failed; resume the recorded reservation")
            })?;
            ensure!(
                response.status().is_success(),
                "Apple asset transfer returned HTTP {}; resume the recorded reservation",
                response.status()
            );
        }
        Ok(())
    }
}

pub fn validate_operations(size: u64, operations: &Value) -> Result<Vec<&Value>> {
    let mut ranges = operations
        .as_array()
        .context("Missing Apple upload operations")?
        .iter()
        .collect::<Vec<_>>();
    ranges.sort_by_key(|v| v["offset"].as_u64().unwrap_or(u64::MAX));
    let mut end = 0_u64;
    for op in &ranges {
        let url = Url::parse(op["url"].as_str().context("Missing upload URL")?)?;
        ensure!(
            url.scheme() == "https"
                && url.port_or_known_default() == Some(443)
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "Unsafe Apple-issued upload URL"
        );
        ensure!(op["method"] == "PUT", "Unexpected upload method");
        let offset = op["offset"].as_u64().context("Invalid upload offset")?;
        let length = op["length"].as_u64().context("Invalid upload length")?;
        ensure!(
            offset == end && length > 0,
            "Noncontiguous or empty upload range"
        );
        end = end.checked_add(length).context("Upload range overflow")?;
        ensure!(end <= size, "Upload range exceeds the file");
        for h in op["requestHeaders"].as_array().into_iter().flatten() {
            let name = h["name"]
                .as_str()
                .context("Invalid upload header")?
                .to_ascii_lowercase();
            ensure!(
                !["authorization", "cookie", "host", "content-length"].contains(&name.as_str()),
                "Apple upload may not forward credentials or override its host/length"
            );
        }
    }
    ensure!(
        end == size && size > 0,
        "Upload operations do not cover the entire file"
    );
    Ok(ranges)
}
