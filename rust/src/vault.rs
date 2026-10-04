//! Portable encrypted signing assets. No private key is written to the repository.
use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use std::path::Path;

const AAD: &[u8] = b"ios-release-signing-v1";
pub fn encrypt(value: &Value, password: &str) -> Result<Value> {
    ensure!(
        password.len() >= 16,
        "Signing vault password must contain at least 16 characters"
    );
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let key = derive(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let encrypted = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: &serde_json::to_vec(value)?,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("Cannot encrypt signing vault"))?;
    Ok(
        json!({"schema_version":1,"cipher":"AES-256-GCM","kdf":"scrypt-15-8-1","salt":STANDARD.encode(salt),"nonce":STANDARD.encode(nonce),"ciphertext":STANDARD.encode(encrypted)}),
    )
}
pub fn decrypt(value: &Value, password: &str) -> Result<Value> {
    ensure!(
        value["schema_version"] == 1
            && value["cipher"] == "AES-256-GCM"
            && value["kdf"] == "scrypt-15-8-1",
        "Unsupported signing vault format"
    );
    let field = |name: &str| -> Result<Vec<u8>> {
        STANDARD
            .decode(value[name].as_str().context("Malformed encrypted vault")?)
            .context("Malformed encrypted vault")
    };
    let salt = field("salt")?;
    let nonce = field("nonce")?;
    let content = field("ciphertext")?;
    ensure!(
        salt.len() == 16 && nonce.len() == 12 && content.len() < 16 * 1024 * 1024,
        "Malformed encrypted vault"
    );
    let key = derive(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let plain = cipher
        .decrypt(
            nonce.as_slice().into(),
            Payload {
                msg: &content,
                aad: AAD,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Signing vault authentication failed; password or encrypted assets differ"
            )
        })?;
    serde_json::from_slice(&plain).context("Invalid signing vault contents")
}
fn derive(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0_u8; 32];
    scrypt::scrypt(
        password.as_bytes(),
        salt,
        &scrypt::Params::new(15, 8, 1, 32)?,
        &mut key,
    )?;
    Ok(key)
}
pub fn read(path: &Path, password: &str) -> Result<Value> {
    decrypt(
        &serde_json::from_slice(
            &std::fs::read(path)
                .context("Cannot read signing vault; run ios-release signing sync first")?,
        )?,
        password,
    )
}
pub fn write(path: &Path, value: &Value, password: &str) -> Result<()> {
    crate::fsutil::json(path, &encrypt(value, password)?)
}
