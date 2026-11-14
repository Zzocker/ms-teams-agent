//! Microsoft identity platform device-code sign-in, the stored session, and the signed-in user.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

// `.default` asks for the permissions already pre-consented for this client (not a named scope
// like `User.Read`, which forces incremental consent Microsoft blocks between first-party apps —
// AADSTS65002). `offline_access` still gets us a refresh token for the session.
const SCOPE: &str = "https://graph.microsoft.com/.default offline_access";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct User {
    pub name: String,
    pub email: String,
    pub job_title: String,
    pub office: String,
    pub id: String,
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

fn agent() -> ureq::Agent {
    // 4xx carries the OAuth error JSON we need to read, so it isn't an Err.
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into()
}

fn client_id() -> String {
    // Azure CLI public client: pre-authorized for Microsoft Graph and allows device-code flow.
    std::env::var("TEAMS_CLIENT_ID").unwrap_or_else(|_| "04b07795-8ddb-461a-bbee-02f9e1bf7b46".into())
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

pub fn sign_out() {
    if let Ok(p) = session_file() {
        let _ = std::fs::remove_file(p);
    }
}

fn me(access: &str) -> Result<User, String> {
    let mut res = agent()
        .get("https://graph.microsoft.com/v1.0/me")
        .header("Authorization", &format!("Bearer {access}"))
        .call()
        .map_err(|e| e.to_string())?;
    let status = res.status();
    let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Graph /me: {}", v["error"]["message"].as_str().unwrap_or(status.as_str())));
    }
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Ok(User {
        name: s("displayName"),
        email: if s("mail").is_empty() { s("userPrincipalName") } else { s("mail") },
        job_title: s("jobTitle"),
        office: s("officeLocation"),
        id: s("id"),
    })
}

/// Signs in from the stored session. `Ok(None)`: there is none, or it expired (then it's deleted);
/// `Err`: couldn't tell (offline, misconfigured), the session is kept.
pub fn resume() -> Result<Option<User>, String> {
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
            me(&access).map(Some)
        }
        _ => {
            sign_out();
            Ok(None)
        }
    }
}

/// The device-code flow. `on_code(user_code, verification_url)` is called once the user has
/// something to type; `live` is checked between polls so a cancelled sign-in stops and saves nothing.
pub fn device_login(on_code: impl FnOnce(String, String), live: impl Fn() -> bool) -> Result<User, String> {
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
                return me(&access);
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
}
