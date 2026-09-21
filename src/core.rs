// Key-X 核心：路径/常量、审计、AES-GCM 密库（封装格式 v1）、钥匙串、信任库
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::Sha256;

pub const VERSION: &str = "1.3.0";
pub const MODE_KEYCHAIN: &str = "KEYCHAIN";
pub const MODE_PASSWORD: &str = "PBKDF2-SHA256";
pub const KEYCHAIN_SERVICE: &str = "keyx";
pub const DEFAULT_ITER: u32 = 600_000;
pub const DEFAULT_LOCK_MIN: u64 = 30;
pub const DEFAULT_PORT: u16 = 7361;

// ───────────────────────── 路径与常量 ─────────────────────────

pub fn home_dir() -> PathBuf {
    std::env::var("KEYX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".keyx"))
}
pub fn vault_file() -> PathBuf { home_dir().join("vault.json") }
pub fn trust_file() -> PathBuf { home_dir().join("trust.json") }
pub fn audit_file() -> PathBuf { home_dir().join("audit.log") }
pub fn client_file() -> PathBuf { home_dir().join("client.token") }
pub fn pid_file() -> PathBuf { home_dir().join("daemon.pid") }
pub fn daemon_log() -> PathBuf { home_dir().join("daemon.log") }
pub fn port() -> u16 {
    std::env::var("KEYX_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PORT)
}
pub fn lock_minutes() -> u64 {
    std::env::var("KEYX_AUTOLOCK_MIN").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_LOCK_MIN)
}
pub fn default_iterations() -> u32 {
    std::env::var("KEYX_ITERATIONS").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_ITER)
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

pub fn ensure_home() {
    let _ = fs::create_dir_all(home_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(home_dir(), fs::Permissions::from_mode(0o700));
    }
}

// ───────────────────────── 审计 ─────────────────────────

pub fn audit(event: &str, extra: Value) {
    ensure_home();
    let mut obj = json!({ "ts": now_iso(), "event": event });
    if let (Some(dst), Some(src)) = (obj.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(audit_file()) {
        let _ = writeln!(f, "{}", obj);
    }
}

// ───────────────────────── 错误 ─────────────────────────

#[derive(Debug, Clone)]
pub struct VaultError {
    pub code: &'static str,
    pub message: String,
}
impl VaultError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        VaultError { code, message: message.into() }
    }
}
impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

// ───────────────────────── 基础工具 ─────────────────────────

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut v);
    v
}
pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn derive_key(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password, salt, rounds, &mut out);
    out
}

pub fn aead_encrypt(key: &[u8], iv: &[u8], plain: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::new("INTERNAL", "密钥长度错误"))?;
    cipher
        .encrypt(Nonce::from_slice(iv), Payload { msg: plain, aad: aad.unwrap_or(&[]) })
        .map_err(|_| VaultError::new("INTERNAL", "加密失败"))
}
/// 解密失败返回 Err(None)，调用方按模式翻译成 WRONG_PASSWORD / VAULT_CORRUPT
pub fn aead_decrypt(key: &[u8], iv: &[u8], ct: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::new("INTERNAL", "密钥长度错误"))?;
    cipher
        .decrypt(Nonce::from_slice(iv), Payload { msg: ct, aad: aad.unwrap_or(&[]) })
        .map_err(|_| VaultError::new("DECRYPT_FAIL", ""))
}

// ───────────────────────── 系统钥匙串 ─────────────────────────

pub fn keychain_user() -> String {
    std::env::var("KEYX_KEYCHAIN_USER").unwrap_or_else(|_| "master".into())
}

pub fn keychain_get() -> Option<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_user()).ok()?;
    entry.get_password().ok()
}

pub fn keychain_set(hex: &str) -> Result<(), VaultError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_user())
        .and_then(|e| e.set_password(hex))
        .map_err(|e| VaultError::new("KEYCHAIN_FAIL", format!("钥匙串写入失败：{}", e)))
}

pub fn keychain_delete() {
    if let Ok(e) = keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_user()) {
        let _ = e.delete_credential();
    }
}

/// 平台语义：有钥匙串后端即用。macOS Security 框架始终在（除显式 KEYX_MODE=password）；
/// 其他平台后端不可达（dbus 缺失等）视为不可用，回退主密码模式。
pub fn keychain_available() -> bool {
    if std::env::var("KEYX_MODE").as_deref() == Ok("password") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_user());
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        match keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_user()) {
            Ok(e) => !matches!(e.get_password(), Err(keyring::Error::NoStorageAccess) | Err(keyring::Error::PlatformFailure)),
            Err(_) => false,
        }
    }
}

// ───────────────────────── 密库（封装格式 v1）─────────────────────────

pub fn load_store() -> Result<Option<Value>, VaultError> {
    let p = vault_file();
    if !p.exists() {
        return Ok(None);
    }
    let txt = fs::read_to_string(&p).map_err(|_| VaultError::new("VAULT_CORRUPT", "密库文件损坏"))?;
    let v: Value = serde_json::from_str(&txt).map_err(|_| VaultError::new("VAULT_CORRUPT", "密库文件损坏"))?;
    Ok(Some(v))
}

pub fn write_store(store: &Value) -> Result<(), VaultError> {
    ensure_home();
    let tmp = vault_file().with_extension("tmp");
    let text = serde_json::to_string_pretty(store).unwrap_or_default();
    fs::write(&tmp, text).map_err(|e| VaultError::new("IO", format!("密库写入失败：{}", e)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, vault_file()).map_err(|e| VaultError::new("IO", format!("密库落盘失败：{}", e)))?;
    Ok(())
}

pub fn new_key_id(existing: &HashSet<String>) -> String {
    loop {
        let kid = format!("kx_{}", to_hex(&random_bytes(4)));
        if !existing.contains(&kid) {
            return kid;
        }
    }
}

pub fn resolve_create_mode(body: &Value) -> Result<String, VaultError> {
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| std::env::var("KEYX_MODE").ok())
        .unwrap_or_else(|| "auto".into());
    let mode = match mode.as_str() {
        "keychain" => MODE_KEYCHAIN.to_string(),
        "password" => MODE_PASSWORD.to_string(),
        other => other.to_string(),
    };
    if mode == "auto" {
        return Ok(if keychain_available() { MODE_KEYCHAIN.into() } else { MODE_PASSWORD.into() });
    }
    if mode == MODE_KEYCHAIN && !keychain_available() {
        return Err(VaultError::new(
            "NO_KEYRING",
            "当前环境没有可用的系统钥匙串（服务器/SSH？）。请设置 KEYX_MODE=password 使用主密码模式。",
        ));
    }
    Ok(mode)
}

pub fn vault_create(password: Option<&str>, mode: &str) -> Result<(Vec<u8>, Value), VaultError> {
    let salt = random_bytes(16);
    let iter: u32;
    let key: Vec<u8>;
    if mode == MODE_KEYCHAIN {
        if keychain_get().is_some() {
            return Err(VaultError::new(
                "KEYCHAIN_OCCUPIED",
                "系统钥匙串中已有 keyx 主密钥，但没有对应的密库文件。若确认旧密库已废弃，运行 `keyx keychain-reset` 清除后再创建；否则可设置 KEYX_MODE=password 使用主密码模式。",
            ));
        }
        key = random_bytes(32);
        keychain_set(&to_hex(&key))?;
        iter = 1;
    } else {
        let pw = match password {
            Some(p) if p.chars().count() >= 8 => p,
            _ => return Err(VaultError::new("WEAK", "主密码至少 8 位")),
        };
        iter = default_iterations();
        key = derive_key(pw.as_bytes(), &salt, iter).to_vec();
    }
    let data = json!({ "entries": [], "meta": { "createdAt": now_iso(), "kdfIterations": iter } });
    let iv = random_bytes(12);
    let aad = if mode == MODE_KEYCHAIN { Some(salt.as_slice()) } else { None };
    let blob = aead_encrypt(&key, &iv, data.to_string().as_bytes(), aad)?;
    write_store(&json!({
        "app": "key-x", "format": 1, "kdf": mode,
        "iterations": iter,
        "salt": B64.encode(&salt), "iv": B64.encode(&iv), "data": B64.encode(&blob),
        "updatedAt": now_iso(),
    }))?;
    Ok((key, data))
}

pub fn vault_unlock(password: Option<&str>) -> Result<(Vec<u8>, Value, String), VaultError> {
    let corrupt = || VaultError::new("VAULT_CORRUPT", "密库文件损坏");
    let store = load_store()?.ok_or_else(|| VaultError::new("NO_VAULT", "还没有密码库"))?;
    let app_ok = store.get("app").and_then(|v| v.as_str()) == Some("key-x");
    let has = |f: &str| store.get(f).and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if !app_ok || !has("data") || !has("salt") {
        return Err(VaultError::new("VAULT_CORRUPT", "不是有效的 Key-X 密库"));
    }
    let iterations = store.get("iterations").and_then(|v| v.as_u64()).unwrap_or(0);
    if iterations < 1 || iterations > u32::MAX as u64 {
        return Err(VaultError::new("VAULT_CORRUPT", "KDF 参数非法"));
    }
    let iterations = iterations as u32;
    let kdf = store.get("kdf").and_then(|v| v.as_str()).unwrap_or(MODE_PASSWORD).to_string();
    let salt = B64.decode(store["salt"].as_str().unwrap()).map_err(|_| corrupt())?;
    let key: Vec<u8>;
    if kdf == MODE_KEYCHAIN {
        let hx = keychain_get().ok_or_else(|| {
            VaultError::new(
                "KEYCHAIN_MISSING",
                "系统钥匙串中没有 keyx 主密钥（密库可能来自其他机器）。请在原机器用 `keyx export --include-key` 导出后在本机 `keyx import`，或删除本密库重新录入。",
            )
        })?;
        key = from_hex(&hx).ok_or_else(corrupt)?;
    } else {
        let pw = password.ok_or_else(|| VaultError::new("LOCKED", "需要主密码"))?;
        key = derive_key(pw.as_bytes(), &salt, iterations).to_vec();
    }
    let iv = B64.decode(store["iv"].as_str().unwrap_or("")).map_err(|_| corrupt())?;
    let ct = B64.decode(store["data"].as_str().unwrap_or("")).map_err(|_| corrupt())?;
    let aad = if kdf == MODE_KEYCHAIN { Some(salt.as_slice()) } else { None };
    let plain = match aead_decrypt(&key, &iv, &ct, aad) {
        Ok(p) => p,
        Err(_) if kdf == MODE_PASSWORD => {
            return Err(VaultError::new("WRONG_PASSWORD", "主密码错误，或密文被篡改"));
        }
        Err(_) => {
            return Err(VaultError::new("VAULT_CORRUPT", "钥匙串密钥与此密库不匹配，或密文被篡改"));
        }
    };
    let data: Value = serde_json::from_slice(&plain).map_err(|_| VaultError::new("VAULT_CORRUPT", "密库内部结构异常"))?;
    if data.get("entries").and_then(|v| v.as_array()).is_none() {
        return Err(VaultError::new("VAULT_CORRUPT", "密库内部结构异常"));
    }
    if data.get("meta").and_then(|m| m.get("kdfIterations")).and_then(|v| v.as_u64()) != Some(iterations as u64) {
        return Err(VaultError::new("VAULT_CORRUPT", "KDF 参数与密文不一致，数据可能被篡改"));
    }
    Ok((key, data, kdf))
}

pub fn vault_save(key: &[u8], data: &Value) -> Result<(), VaultError> {
    let mut store = load_store()?.ok_or_else(|| VaultError::new("NO_VAULT", "还没有密码库"))?;
    let iv = random_bytes(12);
    let aad = if store.get("kdf").and_then(|v| v.as_str()) == Some(MODE_KEYCHAIN) {
        Some(B64.decode(store["salt"].as_str().unwrap_or("")).map_err(|_| VaultError::new("VAULT_CORRUPT", "密库文件损坏"))?)
    } else {
        None
    };
    let blob = aead_encrypt(key, &iv, data.to_string().as_bytes(), aad.as_deref())?;
    store["iv"] = json!(B64.encode(&iv));
    store["data"] = json!(B64.encode(&blob));
    store["updatedAt"] = json!(now_iso());
    write_store(&store)
}

// ───────────────────────── 信任库 ─────────────────────────

pub fn trust_load() -> Vec<Value> {
    fs::read_to_string(trust_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn trust_save(entries: &[Value]) {
    ensure_home();
    let tmp = trust_file().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(entries).unwrap_or_default()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        let _ = fs::rename(&tmp, trust_file());
    }
}

pub fn trust_allowed(project: &str, key_id: &str) -> bool {
    trust_load().iter().any(|e| {
        e.get("project").and_then(|v| v.as_str()) == Some(project)
            && e.get("keys")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|k| k.as_str() == Some(key_id)))
                .unwrap_or(false)
    })
}
