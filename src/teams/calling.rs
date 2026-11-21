//! Outbound audio signaling used by Teams Web's Flight Proxy and message broker.

mod sdp;

use super::Session;
use crate::call_audio::{AudioPeer, Microphone, Ringback};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::Read;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_BODY: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Progress {
    Ringing,
    Connecting,
    Connected,
}

pub fn direct_peer<'a>(chat: &'a str, me: &str) -> Option<&'a str> {
    let (first, second) = chat
        .strip_prefix("19:")?
        .strip_suffix("@unq.gbl.spaces")?
        .split_once('_')?;
    let me = Uuid::parse_str(me).ok()?;
    let a = Uuid::parse_str(first).ok()?;
    let b = Uuid::parse_str(second).ok()?;
    match (a == me, b == me) {
        (true, false) => Some(second),
        (false, true) => Some(first),
        _ => None,
    }
}

pub fn dial(
    session: &Session,
    chat: &str,
    cancelled: Arc<AtomicBool>,
    microphone: &Microphone,
    update: impl Fn(Progress),
) -> Result<(), String> {
    let peer = direct_peer(chat, &session.user.id).ok_or("This chat cannot be called.")?;
    let start = start_url(&session.middle_tier)?;
    let ringback = Ringback::start(cancelled.clone());
    let access = crate::auth::calling_access(&session.user.id)
        .map_err(|_| "Teams could not authorize audio calling.")?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let relays = teams_relays(session)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let audio = AudioPeer::start(&relays, microphone)?;
    let offer = audio.gather_offer(&cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let offer = sdp::to_teams(&offer?)?;
    let caller = Caller::new(session, offer.local_mid);
    let http = Client::new(access, caller.call_id);
    let payload = start_payload(&caller, peer, &offer.wire);
    let initial = http.request("POST", &start, Some(&payload))?;
    let leave = checked_link(&initial, "/links/leave")?;
    let result = run_call(
        &http, &caller, &audio, &initial, &cancelled, ringback, update,
    );
    drop(audio);
    match result {
        Ok(Ended::Remote(error)) => error.map_or(Ok(()), Err),
        result => {
            let end = http
                .request("POST", leave, Some(&caller.leave()))
                .map(|_| ());
            result.map(|_| ()).and(end)
        }
    }
}

fn start_url(middle_tier: &str) -> Result<String, String> {
    let region = middle_tier
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('-')
        .next()
        .unwrap_or("");
    if !matches!(region, "amer" | "emea" | "apac") {
        return Err("This Teams calling region is unavailable.".into());
    }
    Ok(format!(
        "https://api-{region}.flightproxy.teams.microsoft.com/api/v2/epconv"
    ))
}

fn teams_relays(session: &Session) -> Result<[String; 3], String> {
    let http: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let mut response = http
        .get("https://teams.microsoft.com/trap/tokens/")
        .header("X-Skypetoken", &session.skype_token)
        .header("api-version", "2")
        .call()
        .map_err(|_| "Could not authorize the Teams audio relay.")?;
    let credentials: Value = response
        .body_mut()
        .read_json()
        .map_err(|_| "Teams returned invalid audio relay credentials.")?;
    let token = credentials["tokens"]
        .as_array()
        .and_then(|tokens| {
            tokens.iter().find(|token| {
                token["realm"]
                    .as_str()
                    .is_some_and(|realm| realm.trim_matches('"') == "rtcmedia")
            })
        })
        .ok_or("Teams did not provide audio relay credentials.")?;
    let username = token["username"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("Teams did not provide an audio relay username.")?;
    let password = token["password"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("Teams did not provide an audio relay password.")?;
    let userinfo = format!("{}:{}", super::encode(username), super::encode(password));
    let host = "gateway-world.az.relay.teams.cloud.microsoft";
    Ok([
        format!("turn://{userinfo}@{host}:3478?transport=udp"),
        format!("turn://{userinfo}@{host}:443?transport=tcp"),
        format!("turns://{userinfo}@{host}:443"),
    ])
}

struct Caller {
    participant: Value,
    call_id: Uuid,
    audio_mid: String,
}

impl Caller {
    fn new(session: &Session, audio_mid: String) -> Self {
        Self {
            participant: json!({
                "id":format!("8:orgid:{}", session.user.id),
                "displayName":session.user.name,
                "endpointId":Uuid::new_v4().to_string(),
                "languageId":"en-US"
            }),
            call_id: Uuid::new_v4(),
            audio_mid,
        }
    }

    // In broker mode these are logical delivery addresses. The browser polls the
    // real subscription URL returned by the service instead of hosting a webhook.
    fn callback(&self, route: &str) -> String {
        format!(
            "http://broker.invalid/callAgent/{}/{}/{route}/",
            self.call_id,
            Uuid::new_v4()
        )
    }

    fn acknowledge(&self) -> Value {
        json!({"callAcceptanceAcknowledgement":{"links":{
            "mediaRenegotiation": self.callback("call/mediaRenegotiation"),
            "controlVideoStreaming": self.callback("call/controlVideoStreaming"),
            "updateMediaDescriptions": self.callback("call/updateMediaDescriptions")
        }}})
    }

    fn leave(&self) -> Value {
        json!({
            "participants":{"from":self.participant},
            "conversationTransactionEnd":{"reason":"noError","code":0,"phrase":"ConversationEndNoModalityConnected"},
            "callTransactionEnd":{"code":0,"subCode":0,"phrase":"CallEndReasonLocalUserInitiated"}
        })
    }
}

fn start_payload(caller: &Caller, peer: &str, offer: &str) -> Value {
    let callback = |route| caller.callback(route);
    json!({
        "conversationRequest": {
            "conversationType": null,
            "suppressDialout": false,
            "roster": {"type":"Delta", "rosterUpdate":callback("conversation/rosterUpdate")},
            "properties": {"allowConversationWithoutHost":false,"enableGroupCallEventMessages":true},
            "links": {
                "conversationEnd":callback("conversation/conversationEnd"),
                "conversationUpdate":callback("conversation/conversationUpdate"),
                "localParticipantUpdate":callback("conversation/localParticipantUpdate")
            }
        },
        "participants": {
            "from":caller.participant,
            "to":[{"id":format!("8:orgid:{peer}")}]
        },
        "endpointCapabilities": 16 | 128,
        "callInvitation": {
            "callModalities":["Audio"],
            "links":{
                "progress":callback("call/progress"),
                "mediaAnswer":callback("call/mediaAnswer"),
                "acceptance":callback("call/acceptance"),
                "end":callback("call/end")
            },
            "clientContentForMediaController": {
                "controlVideoStreaming":callback("call/controlVideoStreaming"),
                "csrcInfo":callback("call/csrcInfo")
            },
            "mediaContent":{
                "blob":offer,
                "contentType":"application/sdp-ngc-1.0",
                "requiredFeatures":"nonByPass",
                "mediaLegId":Uuid::new_v4().to_string().to_ascii_uppercase()
            }
        }
    })
}

#[derive(Clone)]
struct Client {
    http: ureq::Agent,
    control: ureq::Agent,
    access: Arc<str>,
    call_id: Uuid,
}

impl Client {
    fn new(access: String, call_id: Uuid) -> Self {
        Self {
            http: ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(24)))
                .build()
                .into(),
            control: ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(10)))
                .build()
                .into(),
            access: Arc::from(access),
            call_id,
        }
    }

    fn request(&self, method: &str, url: &str, payload: Option<&Value>) -> Result<Value, String> {
        if !trusted_url(url) {
            return Err("Teams returned an invalid calling service link.".into());
        }
        let request = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", format!("Bearer {}", self.access))
            .header("X-MS-Migration", "True")
            .header("X-Microsoft-Skype-Client", "27/1.0.0.2023052414")
            .header("X-Microsoft-Skype-Chain-ID", self.call_id.to_string())
            .header("X-Microsoft-Skype-Message-ID", Uuid::new_v4().to_string())
            .header("api-version", "2")
            .header("Content-Type", "application/json")
            .body(payload.map(Value::to_string).unwrap_or_default())
            .map_err(|_| "Could not create the Teams calling request.")?;
        let agent = if method == "GET" {
            &self.http
        } else {
            &self.control
        };
        let mut response = agent
            .run(request)
            .map_err(|_| "The Teams calling service did not respond.")?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "Teams rejected the calling request (HTTP {status})."
            ));
        }
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|_| "Could not read the Teams calling response.")?;
        if body.len() as u64 > MAX_BODY {
            return Err("The Teams calling response exceeded its size limit.".into());
        }
        if body.iter().all(u8::is_ascii_whitespace) {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(&body)
                .map_err(|_| "Teams returned an invalid calling response.".into())
        }
    }
}

fn trusted_url(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    uri.scheme_str() == Some("https")
        && uri
            .host()
            .is_some_and(|host| host.ends_with(".flightproxy.teams.microsoft.com"))
        && uri.port_u16().is_none_or(|port| port == 443)
        && uri
            .authority()
            .is_some_and(|authority| !authority.as_str().contains('@'))
}

fn checked_link<'a>(value: &'a Value, path: &str) -> Result<&'a str, String> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|link| trusted_url(link))
        .ok_or_else(|| format!("Teams did not return a valid {path} link."))
}

fn broker_message(encoded: &str) -> Result<Value, String> {
    fn inflate(encoded: &str) -> Result<String, String> {
        let compressed = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid Teams broker encoding.")?;
        let reader: Box<dyn Read> = if compressed.starts_with(&[0x1f, 0x8b]) {
            Box::new(flate2::read::GzDecoder::new(compressed.as_slice()))
        } else {
            Box::new(flate2::read::ZlibDecoder::new(compressed.as_slice()))
        };
        let mut plain = String::new();
        reader
            .take(MAX_BODY + 1)
            .read_to_string(&mut plain)
            .map_err(|_| "Invalid Teams broker data.")?;
        if plain.len() as u64 > MAX_BODY {
            return Err("Teams broker data exceeded its size limit.".into());
        }
        Ok(plain)
    }

    let message = inflate(encoded)?;
    let (headers, body) = message
        .split_once("\r\n\r\n")
        .or_else(|| message.split_once("\n\n"))
        .ok_or("Invalid Teams broker envelope.")?;
    let compressed =
        headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .any(|(name, value)| {
                name.eq_ignore_ascii_case("content-encoding")
                    && value.trim().eq_ignore_ascii_case("gzip")
            });
    let expanded;
    let body = if compressed {
        expanded = inflate(body.trim())?;
        expanded.as_str()
    } else {
        body
    };
    serde_json::from_str(body).map_err(|_| "Invalid Teams broker event.".into())
}

enum Ended {
    Local,
    Remote(Option<String>),
}

fn early_end_reason(event: &Value, received_answer: bool) -> (String, bool) {
    let end = event
        .get("callEnd")
        .filter(|value| !value.is_null())
        .or_else(|| event.get("conversationEnd"))
        .unwrap_or(&Value::Null);
    let detail = end
        .get("callControllerTransactionEnd")
        .filter(|value| value.is_object())
        .unwrap_or(end);
    let code = detail["code"].as_i64().or_else(|| end["code"].as_i64());
    let subcode = [detail, end]
        .iter()
        .find_map(|end| end["subCode"].as_i64().filter(|code| *code != 0));
    let rejected_sdp = [end, detail]
        .iter()
        .any(|end| end["phrase"] == "SdpParsingFailure");
    let message = match code {
        _ if rejected_sdp => "Teams rejected the audio session description.",
        Some(603) => "The Teams call was declined.",
        _ if received_answer => "Teams ended the call before the audio connection was ready.",
        _ => "Teams ended the call before sending an audio answer.",
    };
    let message = match (code.filter(|code| *code != 0), subcode) {
        (Some(code), Some(subcode)) => format!("{message} Teams code {code}/{subcode}."),
        (Some(code), _) => format!("{message} Teams code {code}."),
        _ => message.into(),
    };
    let specific = rejected_sdp || subcode.is_some() || !matches!(code, None | Some(0 | 1000));
    (message, specific)
}

fn run_call(
    client: &Client,
    caller: &Caller,
    audio: &crate::call_audio::AudioPeer,
    initial: &Value,
    cancelled: &AtomicBool,
    ringback: Ringback,
    update: impl Fn(Progress),
) -> Result<Ended, String> {
    let subscription = checked_link(initial, "/links/subscribe")?.to_owned();
    let (send, receive) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let broker_stop = stop.clone();
    let broker = client.clone();
    std::thread::spawn(move || poll_broker(broker, subscription, &broker_stop, send));
    let result = consume_events(client, caller, audio, &receive, cancelled, ringback, update);
    stop.store(true, Ordering::Relaxed);
    result
}

fn poll_broker(
    client: Client,
    mut subscription: String,
    stop: &AtomicBool,
    send: mpsc::SyncSender<Result<Value, String>>,
) {
    while !stop.load(Ordering::Relaxed) {
        let result = client.request("GET", &subscription, None);
        match result {
            Ok(batch) => {
                if let Some(next) = batch["nextSubscribeUrl"].as_str() {
                    subscription = next.to_owned();
                }
                if send.send(Ok(batch)).is_err() {
                    break;
                }
            }
            error => {
                let _ = send.send(error);
                break;
            }
        }
    }
}

fn consume_events(
    client: &Client,
    caller: &Caller,
    audio: &crate::call_audio::AudioPeer,
    receive: &mpsc::Receiver<Result<Value, String>>,
    cancelled: &AtomicBool,
    ringback: Ringback,
    update: impl Fn(Progress),
) -> Result<Ended, String> {
    use gstreamer_webrtc::WebRTCPeerConnectionState::{Connected, Failed};
    let mut accepted = None;
    let mut ringing = false;
    let mut ringback = Some(ringback);
    let mut received_answer = false;
    let mut connected = false;
    let mut pending_end: Option<(Instant, String)> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !cancelled.load(Ordering::Relaxed) {
        if let Some((until, _)) = &pending_end {
            if Instant::now() >= *until {
                break;
            }
        } else {
            audio.check_error()?;
            match audio.connection_state() {
                Connected if !connected => {
                    connected = true;
                    update(Progress::Connected);
                }
                Failed => return Err("The WebRTC audio connection failed.".into()),
                _ => {}
            }
            if !connected && Instant::now() >= accepted.unwrap_or(deadline) {
                return Err(if accepted.is_some() {
                    "Audio did not connect."
                } else {
                    "The call was not answered."
                }
                .into());
            }
        }
        let batch = match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(batch)) => batch,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) if pending_end.is_some() => {
                break;
            }
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("The Teams call event stream ended.".into());
            }
        };
        for entry in batch["message"]
            .as_str()
            .unwrap_or("")
            .split(',')
            .filter(|entry| !entry.is_empty())
        {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(Ended::Local);
            }
            let event = broker_message(entry)?;
            if !event["callEnd"].is_null() || !event["conversationEnd"].is_null() {
                drop(ringback.take());
                if connected {
                    return Ok(Ended::Remote(None));
                }
                let (message, specific) = early_end_reason(&event, received_answer);
                if specific {
                    return Ok(Ended::Remote(Some(message)));
                }
                // A generic conversation-end notification can precede the call controller's reason.
                pending_end
                    .get_or_insert_with(|| (Instant::now() + Duration::from_millis(750), message));
            }
            if pending_end.is_some() {
                continue;
            }
            received_answer |= event
                .pointer("/mediaAnswer/mediaContent/blob")
                .is_some_and(Value::is_string);
            if accepted.is_some() {
                continue;
            }
            if event["callAcceptance"].is_null() {
                if !ringing && (!event["callProgress"].is_null() || !event["mediaAnswer"].is_null())
                {
                    ringing = true;
                    update(Progress::Ringing);
                }
                continue;
            }
            drop(ringback.take());
            let answer = event["callAcceptance"]["mediaContent"]["blob"]
                .as_str()
                .ok_or("Teams accepted the call without an audio answer.")?;
            let ack = checked_link(&event, "/callAcceptance/links/acknowledgement")?;
            audio.accept_answer(&sdp::from_teams(answer, &caller.audio_mid)?)?;
            received_answer = true;
            client.request("POST", ack, Some(&caller.acknowledge()))?;
            accepted = Some(Instant::now() + Duration::from_secs(30));
            update(Progress::Connecting);
        }
    }
    Ok(match pending_end {
        Some((_, error)) => Ended::Remote(Some(error)),
        None => Ended::Local,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn validates_direct_chat_members_and_trusted_service_links() {
        let me = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let chat = format!("19:{me}_{peer}@unq.gbl.spaces");
        assert_eq!(
            direct_peer(&chat, &me.to_string()),
            Some(peer.to_string().as_str())
        );
        assert_eq!(direct_peer(&chat, &Uuid::new_v4().to_string()), None);
        assert!(trusted_url(
            "https://api-apac.flightproxy.teams.microsoft.com/api/v2/epconv"
        ));
        assert!(!trusted_url(
            "https://api.flightproxy.teams.microsoft.com.example.org/api/v2/epconv"
        ));
        assert!(!trusted_url(
            "https://evil.test@api.flightproxy.teams.microsoft.com/api/v2/epconv"
        ));
    }

    #[test]
    fn decodes_the_compressed_http_broker_envelope() {
        let mut writer =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        writer.write_all(b"POST /callAgent/test/call/acceptance/ HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"callAcceptance\":{\"mediaContent\":{\"blob\":\"v=0\"}}}").unwrap();
        let event = broker_message(&STANDARD.encode(writer.finish().unwrap())).unwrap();
        assert_eq!(event["callAcceptance"]["mediaContent"]["blob"], "v=0");
    }

    #[test]
    fn reports_the_call_controller_code_and_negotiation_stage() {
        let event = json!({"callEnd":{
            "code":0,
            "callControllerTransactionEnd":{"code":488,"subCode":1201}
        }});
        assert_eq!(
            early_end_reason(&event, false),
            (
                "Teams ended the call before sending an audio answer. Teams code 488/1201.".into(),
                true
            )
        );
        assert_eq!(
            early_end_reason(&event, true),
            (
                "Teams ended the call before the audio connection was ready. Teams code 488/1201."
                    .into(),
                true
            )
        );
        let parsing_failure = json!({"callEnd":{
            "code":1000, "subCode":7106, "phrase":"SdpParsingFailure"
        }});
        assert_eq!(
            early_end_reason(&parsing_failure, false),
            (
                "Teams rejected the audio session description. Teams code 1000/7106.".into(),
                true
            )
        );
        let generic = json!({"conversationEnd":{"code":1000}});
        assert!(!early_end_reason(&generic, false).1);
    }
}
