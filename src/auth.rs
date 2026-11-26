use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
pub const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const OAUTH_FILE: &str = "oauth-gemini.json";
pub const OAUTH_KEYRING_ACCOUNT: &str = "oauth-gemini";
pub const OAUTH_SLOT: &str = "gemini:oauth";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOAuth {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
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
    authorize_url_with_redirect(pkce, REDIRECT_URI)
}

pub fn authorize_url_with_redirect(pkce: &Pkce, redirect_uri: &str) -> String {
    let scope = SCOPES.join(" ");
    let fields = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
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
    exchange_code_with_redirect(token_url, code, pkce, REDIRECT_URI)
}

pub fn exchange_code_with_redirect(
    token_url: &str,
    code: &str,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<GoogleTokens, String> {
    let response = ureq::post(token_url).send_form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", pkce.verifier.as_str()),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("redirect_uri", redirect_uri),
    ]);
    parse_token_response(response)
}

pub fn refresh_access_token(token_url: &str, refresh_token: &str) -> Result<GoogleTokens, String> {
    let response = ureq::post(token_url).send_form(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
    ]);
    parse_token_response(response)
}

fn parse_token_response(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<GoogleTokens, String> {
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

pub fn load_oauth(dir: &Path) -> io::Result<Option<StoredOAuth>> {
    let Some(raw) = crate::credentials::load_aux_secret(dir, OAUTH_KEYRING_ACCOUNT, OAUTH_FILE)?
    else {
        return Ok(None);
    };
    let parsed = serde_json::from_str(&raw)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid oauth-gemini.json"))?;
    Ok(Some(parsed))
}

/// Removes stored Gemini OAuth tokens from every store they may live in.
pub fn delete_oauth_gemini(dir: &Path) -> io::Result<()> {
    crate::credentials::delete_aux_secret(dir, OAUTH_KEYRING_ACCOUNT, OAUTH_FILE)
}

pub fn has_oauth_gemini(dir: &Path) -> bool {
    crate::credentials::has_aux_secret(dir, OAUTH_KEYRING_ACCOUNT, OAUTH_FILE)
}

pub fn save_oauth(dir: &Path, stored: &StoredOAuth) -> io::Result<()> {
    let payload = serde_json::to_string(stored).map_err(io::Error::other)?;
    crate::credentials::save_aux_secret(dir, OAUTH_KEYRING_ACCOUNT, OAUTH_FILE, &payload)
}

fn to_stored(tokens: &GoogleTokens, project_id: Option<String>) -> StoredOAuth {
    StoredOAuth {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens
            .expires_in
            .map(|secs| now_secs().saturating_add(secs)),
        project_id,
    }
}

/// Returns a non-expired Gemini OAuth grant, refreshing via `refresh_token` if expired/near-expiry.
pub fn valid_gemini_oauth(dir: &Path, token_url: &str) -> Result<Option<StoredOAuth>, String> {
    let Some(stored) = load_oauth(dir).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let still_valid = stored
        .expires_at
        .is_none_or(|exp| now_secs().saturating_add(60) < exp);
    if still_valid {
        return Ok(Some(stored));
    }
    force_refresh_gemini_oauth(dir, token_url, &stored).map(Some)
}

/// Refreshes the stored Gemini OAuth grant unconditionally using its `refresh_token`.
pub fn force_refresh_gemini_oauth(
    dir: &Path,
    token_url: &str,
    stored: &StoredOAuth,
) -> Result<StoredOAuth, String> {
    let refresh = stored.refresh_token.as_deref().ok_or_else(|| {
        "Gemini OAuth token expired; run `devcode auth login gemini --oauth`".to_string()
    })?;
    let tokens = refresh_access_token(token_url, refresh)?;
    let updated = StoredOAuth {
        access_token: tokens.access_token,
        refresh_token: tokens
            .refresh_token
            .or_else(|| stored.refresh_token.clone()),
        expires_at: tokens
            .expires_in
            .map(|secs| now_secs().saturating_add(secs)),
        project_id: stored.project_id.clone(),
    };
    save_oauth(dir, &updated).map_err(|e| e.to_string())?;
    Ok(updated)
}

pub fn update_gemini_project_id(dir: &Path, project_id: &str) -> Result<(), String> {
    let Some(mut stored) = load_oauth(dir).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    stored.project_id = Some(project_id.to_owned());
    save_oauth(dir, &stored).map_err(|e| e.to_string())
}

/// Exchange a paste-code and write `oauth-gemini.json`. Never touches `credentials.json`.
pub fn login_gemini(dir: &Path, code: &str, token_url: &str, pkce: &Pkce) -> Result<(), String> {
    login_gemini_with_redirect(dir, code, token_url, pkce, REDIRECT_URI)
}

pub fn login_gemini_with_redirect(
    dir: &Path,
    code: &str,
    token_url: &str,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<(), String> {
    let tokens = exchange_code_with_redirect(token_url, code, pkce, redirect_uri)?;
    let existing_project = load_oauth(dir).ok().flatten().and_then(|s| s.project_id);
    save_oauth(dir, &to_stored(&tokens, existing_project)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Short-lived localhost callback listener for browser OAuth redirects.
pub struct LoopbackServer {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl LoopbackServer {
    pub fn bind() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/oauth2callback"),
        })
    }

    pub fn wait_for_code(self, expected_state: &str, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    if let Some(result) = handle_callback_connection(&mut stream, expected_state) {
                        return result;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("OAuth callback timed out".into());
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(format!("OAuth callback listener error: {e}")),
            }
        }
    }
}

fn handle_callback_connection(
    stream: &mut std::net::TcpStream,
    expected_state: &str,
) -> Option<Result<String, String>> {
    let mut reader = BufReader::new(&*stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    if !path.starts_with("/oauth2callback") {
        let _ = write_http_html(
            stream,
            "404 Not Found",
            "<html><body>Not found</body></html>",
        );
        return None;
    }
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params = parse_query(query);
    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        let _ = write_http_html(
            stream,
            "400 Bad Request",
            "<html><body>Authentication failed. You may close this tab.</body></html>",
        );
        return Some(Err(if desc.is_empty() {
            format!("Google OAuth error: {err}")
        } else {
            format!("Google OAuth error: {err} ({desc})")
        }));
    }
    if params.get("state").map(String::as_str) != Some(expected_state) {
        let _ = write_http_html(
            stream,
            "400 Bad Request",
            "<html><body>State mismatch. You may close this tab.</body></html>",
        );
        return Some(Err("OAuth state mismatch".into()));
    }
    let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
        let _ = write_http_html(
            stream,
            "400 Bad Request",
            "<html><body>Missing authorization code.</body></html>",
        );
        return Some(Err("missing authorization code in callback".into()));
    };
    let _ = write_http_html(
        stream,
        "200 OK",
        "<html><body>Authentication succeeded. You may close this tab and return to DevCode.</body></html>",
    );
    Some(Ok(code.clone()))
}

fn write_http_html(stream: &mut std::net::TcpStream, status: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn parse_query(query: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(urldecode(k), urldecode(v));
    }
    map
}

fn urldecode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
