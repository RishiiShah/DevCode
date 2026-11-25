use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::credentials;

pub const CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
pub const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

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
    let mut file = File::open("/dev/urandom")?;
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

pub fn exchange_code(token_url: &str, code: &str, pkce: &Pkce) -> Result<String, String> {
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
    match value.get("access_token").and_then(|v| v.as_str()) {
        Some(token) if !token.is_empty() => Ok(token.to_owned()),
        _ => Err("token response missing access_token".into()),
    }
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

pub fn login_gemini(dir: &Path, code: &str, token_url: &str, pkce: &Pkce) -> Result<(), String> {
    let token = exchange_code(token_url, code, pkce)?;
    let mut entries = credentials::load(dir).map_err(|e| e.to_string())?;
    entries.insert("gemini".to_owned(), token);
    credentials::save(dir, &entries).map_err(|e| e.to_string())?;
    Ok(())
}
