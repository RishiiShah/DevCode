use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
pub const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const OAUTH_FILE: &str = "oauth-gemini.json";

const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

#[derive(Serialize)]
struct StoredOAuth {
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

fn base64url_nopad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let (chunks, rest) = bytes.as_chunks::<3>();
    for chunk in chunks {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
    }
    if rest.len() == 1 {
        let n = u32::from(rest[0]) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rest.len() == 2 {
        let n = (u32::from(rest[0]) << 16) | (u32::from(rest[1]) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}

fn random_bytes(n: usize) -> io::Result<Vec<u8>> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    let mut bytes = vec![0; n];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn urlencode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0xf) as usize] as char);
            }
        }
    }
    out
}

pub fn generate_pkce() -> io::Result<Pkce> {
    let verifier = base64url_nopad(&random_bytes(32)?);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64url_nopad(&digest);
    let state = hex_encode(&random_bytes(32)?);
    Ok(Pkce {
        verifier,
        challenge,
        state,
    })
}

pub fn authorize_url(pkce: &Pkce) -> String {
    let scope = SCOPES.join(" ");
    let fields = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("scope", scope.as_str()),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("state", pkce.state.as_str()),
    ];
    let mut query = String::new();
    for (key, value) in fields {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(key);
        query.push('=');
        query.push_str(&urlencode(value));
    }
    format!("{AUTHORIZE_URL}?{query}")
}

pub fn exchange_code(token_url: &str, code: &str, pkce: &Pkce) -> Result<GoogleTokens, String> {
    let response = ureq::post(token_url).send_form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", pkce.verifier.as_str()),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("redirect_uri", REDIRECT_URI),
    ]);
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => return Err(http_error(code, response)),
        Err(err) => return Err(err.to_string()),
    };
    let value: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("token response: {e}"))?;
    let access_token = match value.get("access_token").and_then(|v| v.as_str()) {
        Some(token) if !token.is_empty() => token.to_owned(),
        _ => return Err("token response missing access_token".into()),
    };
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let expires_in = value.get("expires_in").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
            .or_else(|| v.as_str()?.parse().ok())
    });
    Ok(GoogleTokens {
        access_token,
        refresh_token,
        expires_in,
    })
}

fn http_error(code: u16, response: ureq::Response) -> String {
    let body = response.into_string().unwrap_or_default();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return format!("HTTP {code}");
    };
    let error = value.get("error").and_then(|v| v.as_str()).unwrap_or("");
    let desc = value
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match (error.is_empty(), desc.is_empty()) {
        (false, false) => format!("HTTP {code}: {error} {desc}"),
        (false, true) => format!("HTTP {code}: {error}"),
        (true, false) => format!("HTTP {code}: {desc}"),
        (true, true) => format!("HTTP {code}"),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn save_oauth_file(dir: &Path, tokens: &GoogleTokens) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let stored = StoredOAuth {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens
            .expires_in
            .map(|secs| now_secs().saturating_add(secs)),
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = dir.join(format!(".oauth-gemini-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        serde_json::to_writer(&mut file, &stored).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, dir.join(OAUTH_FILE))?;
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Exchange a paste-code and write `oauth-gemini.json`. Never touches `credentials.json`.
pub fn login_gemini(dir: &Path, code: &str, token_url: &str, pkce: &Pkce) -> Result<(), String> {
    let tokens = exchange_code(token_url, code, pkce)?;
    save_oauth_file(dir, &tokens).map_err(|e| e.to_string())?;
    Ok(())
}
