//! Device-code sign-in with the Teams web client, the stored session, and the Teams (Skype) token
//! its private chat service takes.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

// Sign-in starts with the Teams audience; other services request their own tokens on demand.
const SCOPE: &str = "https://api.spaces.skype.com/.default offline_access";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct User {
    pub name: String,
    pub email: String,
    /// Entra object id; the user's Teams id is `8:orgid:{id}`.
    pub id: String,
}

/// A signed-in user and what the chat service needs.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub user: User,
    /// `Authentication: skypetoken=…` for the chat service.
    pub skype_token: String,
    /// The user's region's chat service, e.g. `https://apac.ng.msg.teams.microsoft.com`.
    pub chat_service: String,
    /// `Authorization: Bearer …` for the middle tier (profiles).
    pub access: String,
    /// The user's region's middle tier, e.g. `https://teams.microsoft.com/api/mt/apac`.
    pub middle_tier: String,
    /// The user's region's media store (pictures), e.g. `https://in-prod.asyncgw.teams.microsoft.com`.
    pub media: String,
}

/// What the token endpoint said.
#[derive(Debug, PartialEq)]
pub enum Token {
    Ok { access: String, refresh: String },
    /// Device flow only: the user hasn't finished yet. `slow_down` asks for a longer interval.
    Pending { slow_down: bool },
    /// The grant is dead (refresh token expired or revoked, code expired or declined): sign in again.
    Rejected(String),
}

pub fn parse_token(v: &Value) -> Result<Token, String> {
    if let Some(access) = v["access_token"].as_str() {
        let refresh = v["refresh_token"].as_str().ok_or("no refresh token in the response (offline_access not granted?)")?;
        return Ok(Token::Ok { access: access.into(), refresh: refresh.into() });
    }
    let desc = v["error_description"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
    match v["error"].as_str() {
        Some("authorization_pending") => Ok(Token::Pending { slow_down: false }),
        Some("slow_down") => Ok(Token::Pending { slow_down: true }),
        Some("invalid_grant" | "interaction_required" | "expired_token" | "authorization_declined" | "bad_verification_code") => {
            Ok(Token::Rejected(desc))
        }
        Some(e) => Err(format!("{e}: {desc}")),
        None => Err(format!("unexpected token response: {v}")),
    }
}

/// One agent for the app, so requests reuse its pooled connections instead of a TLS handshake each.
pub(crate) fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    // 4xx carries the error JSON we need to read, so it isn't an Err.
    AGENT.get_or_init(|| ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into()).clone()
}

fn client_id() -> String {
    // Microsoft's Teams desktop client: a native public client, so device code works without a
    // secret (the web client's id fails with AADSTS7000218). TEAMS_CLIENT_ID overrides it.
    std::env::var("TEAMS_CLIENT_ID").unwrap_or_else(|_| "1fec8e78-bce4-4aaf-ab1b-5451cc387264".into())
}

fn endpoint(name: &str) -> String {
    let tenant = std::env::var("TEAMS_TENANT").unwrap_or_else(|_| "organizations".into());
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/{name}")
}

fn post(url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let mut res = agent().post(url).send_form(form.iter().copied()).map_err(|e| e.to_string())?;
    res.body_mut().read_json().map_err(|e| e.to_string())
}

// shortcut: the refresh token is a 0600 file, not the OS keyring; move it there before shipping to other people.
fn session_file() -> Result<PathBuf, String> {
    let dirs = rune::dirs::AppDirs::new("ms-teams-agent").ok_or("no home folder to keep the session in")?;
    Ok(dirs.data().join("refresh_token"))
}

fn save(refresh: &str) -> Result<(), String> {
    use std::io::Write;
    let path = session_file()?;
    std::fs::create_dir_all(path.parent().unwrap_or(&path)).map_err(|e| e.to_string())?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(&path).and_then(|mut f| f.write_all(refresh.as_bytes())).map_err(|e| format!("couldn't save the session: {e}"))
}

static SESSION_UPDATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn sign_out() {
    let _guard = SESSION_UPDATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(p) = session_file() {
        let _ = std::fs::remove_file(p);
    }
}

/// The user from the access token's claims (the token came straight from Microsoft over TLS,
/// so it isn't verified here), then the Teams token and region for them.
fn session(access: &str) -> Result<Session, String> {
    let claims = access.split('.').nth(1).and_then(b64url).and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .ok_or("the access token isn't a JWT")?;
    let c = |k: &str| claims[k].as_str().unwrap_or("").to_string();
    let user = User {
        name: c("name"),
        email: [c("upn"), c("unique_name"), c("preferred_username")].into_iter().find(|s| !s.is_empty()).unwrap_or_default(),
        id: c("oid"),
    };
    let mut res = agent()
        .post("https://teams.microsoft.com/api/authsvc/v1.0/authz")
        .header("Authorization", &format!("Bearer {access}"))
        .send_empty()
        .map_err(|e| e.to_string())?;
    let status = res.status();
    let v: Value = res.body_mut().read_json().map_err(|e| format!("Teams authz {status}: {e}"))?;
    match (v["tokens"]["skypeToken"].as_str(), v["regionGtms"]["chatService"].as_str()) {
        (Some(t), Some(cs)) => Ok(Session {
            user,
            skype_token: t.into(),
            chat_service: cs.trim_end_matches('/').into(),
            access: access.into(),
            middle_tier: v["regionGtms"]["middleTier"].as_str().unwrap_or("").trim_end_matches('/').into(),
            media: v["regionGtms"]["amsV2"].as_str().or(v["regionGtms"]["ams"].as_str()).unwrap_or("").trim_end_matches('/').into(),
        }),
        _ => Err(format!("Teams authz {status}: {}", v["message"].as_str().or(v["errorCode"].as_str()).unwrap_or("no Teams token in the response"))),
    }
}

fn b64url(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut buf, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.bytes().take_while(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6 | v as u32) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Signs in from the stored session. `Ok(None)`: there is none, or it expired (then it's deleted);
/// `Err`: couldn't tell (offline, misconfigured), the session is kept.
pub fn resume() -> Result<Option<Session>, String> {
    let Ok(refresh) = std::fs::read_to_string(session_file()?) else { return Ok(None) };
    let v = post(&endpoint("token"), &[
        ("grant_type", "refresh_token"),
        ("client_id", &client_id()),
        ("refresh_token", refresh.trim()),
        ("scope", SCOPE),
    ])?;
    match parse_token(&v)? {
        Token::Ok { access, refresh } => {
            // Refresh tokens roll: each use extends the session, so only a long absence expires it.
            save(&refresh)?;
            session(&access).map(Some)
        }
        _ => {
            sign_out();
            Ok(None)
        }
    }
}

/// Renew the background connection without restoring a session that signed out during the request.
pub fn renew(live: impl Fn() -> bool) -> Result<Session, String> {
    let refresh = std::fs::read_to_string(session_file()?).map_err(|e| e.to_string())?;
    let v = post(&endpoint("token"), &[
        ("grant_type", "refresh_token"), ("client_id", &client_id()),
        ("refresh_token", refresh.trim()), ("scope", SCOPE),
    ])?;
    let Token::Ok { access, refresh } = parse_token(&v)? else { return Err("Sign in again to restore live updates.".into()); };
    {
        let _guard = SESSION_UPDATE.lock().unwrap_or_else(|e| e.into_inner());
        if !live() { return Err("cancelled".into()); }
        save(&refresh)?;
    }
    session(&access)
}

/// CSA uses a different audience from chat; request it only when Teams is opened.
pub fn teams_access() -> Result<String, String> {
    access_for("https://chatsvcagg.teams.microsoft.com/.default offline_access")
}

pub fn graph_access() -> Result<String, String> {
    access_for("https://graph.microsoft.com/.default offline_access")
}

/// For a SharePoint site's files API (file previews). Every preview needs one, so it's kept
/// while it's fresh.
#[cfg_attr(not(feature = "image"), allow(dead_code))]
pub fn sharepoint_access(host: &str) -> Result<String, String> {
    // Access tokens last 60 to 90 minutes.
    const FRESH: Duration = Duration::from_secs(45 * 60);
    static TOKENS: std::sync::Mutex<Vec<(String, String, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    let mut tokens = TOKENS.lock().map_err(|e| e.to_string())?;
    tokens.retain(|(_, _, at)| at.elapsed() < FRESH);
    if let Some((_, token, _)) = tokens.iter().find(|(h, _, _)| h == host) {
        return Ok(token.clone());
    }
    let token = access_for(&format!("https://{host}/.default offline_access"))?;
    tokens.push((host.into(), token.clone(), std::time::Instant::now()));
    Ok(token)
}

/// An access token for `scope`'s audience from the stored session; each audience needs its own.
fn access_for(scope: &str) -> Result<String, String> {
    let refresh = std::fs::read_to_string(session_file()?).map_err(|e| e.to_string())?;
    let v = post(&endpoint("token"), &[
        ("grant_type", "refresh_token"),
        ("client_id", &client_id()),
        ("refresh_token", refresh.trim()),
        ("scope", scope),
    ])?;
    match parse_token(&v)? {
        Token::Ok { access, refresh } => {
            save(&refresh)?;
            Ok(access)
        }
        Token::Rejected(e) => Err(e),
        Token::Pending { .. } => Err("Teams authorization is pending.".into()),
    }
}

/// The device-code flow. `on_code(user_code, verification_url)` is called once the user has
/// something to type; `live` is checked between polls so a cancelled sign-in stops and saves nothing.
pub fn device_login(on_code: impl FnOnce(String, String), live: impl Fn() -> bool) -> Result<Session, String> {
    let client_id = client_id();
    let v = post(&endpoint("devicecode"), &[("client_id", &client_id), ("scope", SCOPE)])?;
    let (Some(device_code), Some(user_code), Some(url)) = (v["device_code"].as_str(), v["user_code"].as_str(), v["verification_uri"].as_str())
    else {
        return Err(v["error_description"].as_str().unwrap_or("unexpected device code response").lines().next().unwrap_or("").into());
    };
    on_code(user_code.into(), url.into());
    let mut interval = v["interval"].as_u64().unwrap_or(5);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if !live() {
            return Err("cancelled".into());
        }
        let v = post(&endpoint("token"), &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", &client_id),
            ("device_code", device_code),
        ])?;
        match parse_token(&v)? {
            Token::Pending { slow_down } => interval += 5 * slow_down as u64,
            Token::Rejected(why) => return Err(if why.is_empty() { "Sign-in expired or was declined.".into() } else { why }),
            Token::Ok { access, refresh } => {
                if !live() {
                    return Err("cancelled".into());
                }
                save(&refresh)?;
                return session(&access);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn token_responses() {
        assert_eq!(
            parse_token(&json!({"access_token": "a", "refresh_token": "r"})),
            Ok(Token::Ok { access: "a".into(), refresh: "r".into() })
        );
        assert_eq!(parse_token(&json!({"error": "authorization_pending"})), Ok(Token::Pending { slow_down: false }));
        assert_eq!(parse_token(&json!({"error": "slow_down"})), Ok(Token::Pending { slow_down: true }));
        assert_eq!(
            parse_token(&json!({"error": "invalid_grant", "error_description": "AADSTS700082: expired\r\nTrace ID: x"})),
            Ok(Token::Rejected("AADSTS700082: expired".into()))
        );
        // Not a dead session: a misconfiguration or outage must not sign the user out.
        assert!(parse_token(&json!({"error": "invalid_client"})).is_err());
        assert!(parse_token(&json!({"access_token": "a"})).is_err());
    }

    #[test]
    fn base64url() {
        assert_eq!(b64url("eyJuYW1lIjoiw6kifQ").unwrap(), r#"{"name":"é"}"#.as_bytes());
        assert_eq!(b64url("YQ==").unwrap(), b"a");
        assert_eq!(b64url("-_8").unwrap(), [0xfb, 0xff]);
        assert!(b64url("a.b").is_none());
    }
}
