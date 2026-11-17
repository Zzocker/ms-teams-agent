//! Teams Trouter / Socket.IO push delivery. No periodic chat or message fetches.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::Read;
use std::net::TcpStream;
use std::sync::{Arc, atomic::{AtomicU64, Ordering::SeqCst}, mpsc};
use std::time::{Duration, Instant};
use tungstenite::{Message, client::IntoClientRequest, stream::MaybeTlsStream};

pub enum Update {
    Session(Arc<Session>),
    Connected(Vec<Chat>),
    Message { chat: Chat, message: Option<Msg>, server_id: String, new: bool },
    Status(Option<String>),
    Typing { chat: String, user: String, name: String, active: bool },
}

/// Stop at sign-out, even if an old connection completes after a new account signs in.
pub fn listen(session: Arc<Session>, attempt: Arc<AtomicU64>, mine: u64,
    send: mpsc::SyncSender<(u64, Update)>, wake: rune::Waker) {
    std::thread::spawn(move || {
        let mut session = session;
        let mut renewed = Instant::now();
        let live = || attempt.load(SeqCst) == mine;
        let emit = |update| {
            if !live() { return Err("cancelled".to_owned()); }
            send.try_send((mine, update)).map_err(|_| "event queue full".to_owned())?;
            wake();
            Ok(())
        };
        let mut delay = 1;
        let mut seen = std::collections::VecDeque::new();
        while live() {
            let started = Instant::now();
            let result = (|| {
                if renewed.elapsed() >= Duration::from_secs(40 * 60) {
                    session = Arc::new(crate::auth::renew(&live)?);
                    renewed = Instant::now();
                    emit(Update::Session(session.clone()))?;
                }
                connection(&session, &live, &emit, &mut seen)
            })();
            if !live() { break; }
            if let Err(error) = result {
                let _ = emit(Update::Status(Some(format!("Live updates disconnected ({error}); reconnecting…"))));
            }
            if started.elapsed() > Duration::from_secs(60) { delay = 1; }
            for _ in 0..delay * 4 {
                if !live() { return; }
                std::thread::sleep(Duration::from_millis(250));
            }
            delay = (delay * 2).min(60);
        }
    });
}

fn trusted_router(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else { return false };
    uri.scheme_str() == Some("https") && uri.host().is_some_and(|h|
        h == "go.trouter.teams.microsoft.com" || h.ends_with(".trouter.teams.microsoft.com"))
        && uri.port_u16().is_none_or(|p| p == 443 || p == 3443)
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
}

fn connection(s: &Session, live: &impl Fn() -> bool, emit: &impl Fn(Update) -> Result<(), String>,
    seen: &mut std::collections::VecDeque<String>) -> Result<(), String> {
    let endpoint = uuid::Uuid::new_v4().to_string();
    let http: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false)
        .max_redirects(0).timeout_global(Some(Duration::from_secs(30))).build().into();
    let info = answer(http.post(&format!("https://go.trouter.teams.microsoft.com/v4/a?epid={endpoint}"))
        .header("X-Skypetoken", &s.skype_token).send_empty()).map_err(|_| "router discovery failed")?;
    let base = info["socketio"].as_str().ok_or("missing router address")?;
    let path = info["surl"].as_str().ok_or("missing delivery address")?;
    if !trusted_router(base) || !trusted_router(path) { return Err("untrusted router address".into()); }
    let params = info["connectparams"].as_object().ok_or("missing connection parameters")?;
    let mut query = format!("v=v4&epid={endpoint}&auth=true&timeout=40");
    let client = serde_json::json!({"cv":"2024.23.01.2","ua":"TeamsCDL","hr":"","v":"27/1.0.0.2023052414"});
    query.push_str(&format!("&tc={}&con_num={}_1", encode(&client.to_string()), new_msg_id()));
    for (key, value) in params {
        let value = value.as_str().ok_or("invalid connection parameter")?;
        query.push_str(&format!("&{}={}", encode(key), encode(value)));
    }
    if let Some(ccid) = info["ccid"].as_str() { query.push_str(&format!("&ccid={}", encode(ccid))); }
    let mut handshake = http.get(&format!("{}socket.io/1/?{query}", base.trim_end_matches('/').to_owned() + "/"))
        .header("X-Skypetoken", &s.skype_token).call().map_err(|_| "router handshake failed")?;
    if !handshake.status().is_success() { return Err(format!("router handshake {}", handshake.status())); }
    let handshake = handshake.body_mut().read_to_string().map_err(|_| "invalid router handshake")?;
    let sid = handshake.split(':').next().filter(|v| !v.is_empty()).ok_or("missing socket session")?;
    let ws_url = format!("wss://{}socket.io/1/websocket/{}?{query}", base.trim_start_matches("https://"), encode(sid));
    let mut request = ws_url.into_client_request().map_err(|_| "invalid socket address")?;
    request.headers_mut().insert("X-Skypetoken", s.skype_token.parse().map_err(|_| "invalid token")?);
    let uri = request.uri();
    let host = uri.host().ok_or("missing socket host")?;
    use std::net::ToSocketAddrs;
    let addresses = (host, uri.port_u16().unwrap_or(443)).to_socket_addrs().map_err(|_| "router DNS failed")?;
    let tcp = addresses.filter_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(10)).ok()).next().ok_or("router unreachable")?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|_| "socket timeout failed")?;
    tcp.set_write_timeout(Some(Duration::from_secs(10))).map_err(|_| "socket timeout failed")?;
    let config = tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(4 * 1024 * 1024));
    let (mut socket, _) = tungstenite::client_tls_with_config(request, tcp, Some(config), None).map_err(|_| "socket handshake failed")?;
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp.set_read_timeout(Some(Duration::from_secs(1))),
        MaybeTlsStream::Rustls(tls) => tls.sock.set_read_timeout(Some(Duration::from_secs(1))),
        _ => return Err("unsupported socket transport".into()),
    }.map_err(|_| "socket timeout failed")?;
    let mut ping = Instant::now();
    let mut last_received = Instant::now();
    let mut registered = false;
    let opened = Instant::now();
    while live() {
        // Rotate before both the registrar lease and the access token expire.
        if opened.elapsed() >= Duration::from_secs(40 * 60) { break; }
        if ping.elapsed() >= Duration::from_secs(25) {
            socket.send(Message::Text("5:::{\"name\":\"ping\"}".into())).map_err(|_| "socket write failed")?;
            ping = Instant::now();
        }
        if last_received.elapsed() > Duration::from_secs(90) { return Err("socket timed out".into()); }
        let packet = match socket.read() {
            Ok(Message::Text(text)) => { last_received = Instant::now(); text },
            Ok(Message::Close(_)) => return Err("socket closed".into()),
            Ok(_) => { socket.flush().map_err(|_| "socket write failed")?; last_received = Instant::now(); continue; }
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => return Err("socket read failed".into()),
        };
        let kind = packet.split(':').next().unwrap_or("");
        match kind {
            "0" | "7" => return Err("router ended session".into()),
            "1" if !registered => {
                let auth = serde_json::json!({"name":"user.authenticate", "args":[{
                    "headers":{"Authorization":format!("Bearer {}", s.access), "X-MS-Migration":"True"},
                    "connectparams":info["connectparams"]
                }]});
                socket.send(Message::Text(format!("5:::{auth}").into())).map_err(|_| "authentication write failed")?;
                register(&http, s, &endpoint, path)?;
                registered = true;
                // Recover missed messages once after connecting, rather than polling for them.
                emit(Update::Connected(conversations(s)?))?;
                emit(Update::Status(None))?;
            }
            "2" => socket.send(Message::Text(packet)).map_err(|_| "heartbeat write failed")?,
            "3" => {
                let json = packet.splitn(4, ':').nth(3).ok_or("invalid event packet")?;
                let request: Value = serde_json::from_str(json).map_err(|_| "invalid event JSON")?;
                if request.get("method").is_none() { continue; }
                let ack = serde_json::json!({"id":request["id"],"status":200,"body":""});
                socket.send(Message::Text(format!("3:::{ack}").into())).map_err(|_| "event acknowledgement failed")?;
                let body = decode_body(&request)?;
                if body["type"] != "EventMessage" { continue; }
                let key = format!("{}:{}:{}:{}", body["resourceType"], body["resource"]["conversationLink"], body["resource"]["id"], body["time"]);
                if seen.contains(&key) { continue; }
                if let Some(update) = message_update(&body, &s.user) { emit(update)?; }
                seen.push_back(key);
                if seen.len() > 1024 { seen.pop_front(); }
            }
            "5" => {
                if packet.contains("trouter.message_loss") {
                    register(&http, s, &endpoint, path)?;
                    emit(Update::Connected(conversations(s)?))?;
                }
            }
            _ => {}
        }
    }
    let _ = socket.close(None);
    Ok(())
}

fn register(http: &ureq::Agent, s: &Session, endpoint: &str, path: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "clientDescription":{"appId":"TeamsCDLWebWorker","aesKey":"","languageId":"en-US",
            "platform":"edge","templateKey":"TeamsCDLWebWorker_2.1","platformUIVersion":"27/1.0.0.2023052414"},
        "registrationId":endpoint,"nodeId":"",
        "transports":{"TROUTER":[{"context":"","path":path,"ttl":3600}]}
    });
    let response = http.post("https://teams.microsoft.com/registrar/prod/V2/registrations")
        .header("X-Skypetoken", &s.skype_token).header("Authorization", &format!("Bearer {}", s.access))
        .send_json(&body).map_err(|_| "notification registration failed")?;
    if !response.status().is_success() { return Err(format!("notification registration {}", response.status())); }
    Ok(())
}

fn unzip(encoded: &str, gzip: bool) -> Result<Vec<u8>, String> {
    let bytes = STANDARD.decode(encoded).map_err(|_| "invalid encoded event")?;
    if !gzip { return Ok(bytes); }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_slice()).take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut out).map_err(|_| "invalid compressed event")?;
    if out.len() > 4 * 1024 * 1024 { return Err("event too large".into()); }
    Ok(out)
}

fn decode_body(request: &Value) -> Result<Value, String> {
    let body = request["body"].as_str().ok_or("missing event body")?;
    let bytes = if request["headers"]["X-Microsoft-Skype-Content-Encoding"] == "gzip" { unzip(body, true)? } else { body.as_bytes().to_vec() };
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid event body")?;
    for (key, gzip) in [("cp", true), ("gp", false)] {
        if let Some(encoded) = body[key].as_str() {
            return serde_json::from_slice(&unzip(encoded, gzip)?).map_err(|_| "invalid packed event".into());
        }
    }
    Ok(body)
}

fn message_update(event: &Value, me: &User) -> Option<Update> {
    let new = match event["resourceType"].as_str()? { "NewMessage" => true, "MessageUpdate" => false, _ => return None };
    let r = &event["resource"];
    let kind = r["messagetype"].as_str()?;

    let link = r["conversationLink"].as_str()?;
    let id = link.split("/conversations/").nth(1)?.split('/').next()?;
    // The service supplies percent-encoded conversation links.
    let id = percent_decode(id)?;
    if matches!(kind, "Control/Typing" | "Control/ClearTyping") {
        let user = avatar_id(r["from"].as_str()?);
        if user.is_empty() || user == me.id { return None; }
        return Some(Update::Typing { chat: id, user, name: r["imdisplayname"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("Someone").into(), active: kind == "Control/Typing" });
    }
    if kind != "Text" && !kind.starts_with("RichText") { return None; }
    let wrapper = serde_json::json!({"conversations":[{"id":id,"lastMessage":r,
        "threadProperties": r.get("threadProperties").or_else(|| r.get("properties"))}]});
    let mut chat = parse_conversations(&wrapper, me).pop()?;
    chat.unread = !r["from"].as_str().is_some_and(|from| from.ends_with(&me.id));
    let message = parse_messages(&serde_json::json!({"messages":[r]}), &me.id).pop();
    let server_id = r["id"].as_str()?.to_owned();
    Some(Update::Message { chat, message, server_id, new })
}

fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(b) = input.next() {
        bytes.push(if b == b'%' {
            (input.next()? as char).to_digit(16)? as u8 * 16 + (input.next()? as char).to_digit(16)? as u8
        } else { b });
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a signed-in Teams account and network; registers a temporary push endpoint"]
    fn live_push_connection() {
        let session = crate::auth::resume().expect("resume session").expect("sign in first");
        let started = Instant::now();
        let connected = std::cell::Cell::new(false);
        let live = || started.elapsed() < Duration::from_secs(20);
        let emit = |update| {
            if matches!(update, Update::Connected(_)) { connected.set(true); }
            Ok(())
        };
        connection(&session, &live, &emit, &mut Default::default()).expect("push connection");
        assert!(connected.get(), "router must accept the socket and notification registration");
    }
    #[test]
    fn router_addresses_cannot_leak_credentials() {
        assert!(trusted_router("https://trouter2-region.trouter.teams.microsoft.com:443/"));
        for url in ["http://go.trouter.teams.microsoft.com/", "https://go.trouter.teams.microsoft.com.evil.test/", "https://evil.test/", "https://user@go.trouter.teams.microsoft.com/"] {
            assert!(!trusted_router(url), "{url}");
        }
    }
    #[test]
    fn parses_messages_edits_deletions_and_typing() {
        let mut event = serde_json::json!({"resourceType":"NewMessage", "resource":{
            "id":"123", "clientmessageid":"456", "conversationLink":"https://service/v1/users/ME/conversations/19%3Achat%40thread.v2",
            "messagetype":"Text", "content":"Hello", "from":"8:orgid:other", "composetime":"2026-11-15T00:00:00Z"
        }});
        let Update::Message { chat, message, new, .. } = message_update(&event, &User { id:"me".into(), ..Default::default() }).unwrap() else { panic!() };
        assert_eq!(chat.id, "19:chat@thread.v2");
        assert!(new && chat.unread);
        assert_eq!(message.unwrap().id, "456");
        event["resourceType"] = "MessageUpdate".into();
        event["resource"]["content"] = "".into();
        assert!(matches!(message_update(&event, &User::default()), Some(Update::Message { new:false, message:None, .. })));
        event["resource"]["messagetype"] = "Control/Typing".into();
        assert!(matches!(message_update(&event, &User::default()), Some(Update::Typing { active: true, .. })));
        event["resource"]["messagetype"] = "Control/ClearTyping".into();
        assert!(matches!(message_update(&event, &User::default()), Some(Update::Typing { active: false, .. })));
        assert!(message_update(&event, &User { id: "other".into(), ..Default::default() }).is_none());
        assert_eq!(percent_decode("bad%GG"), None);
    }
    #[test]
    fn channel_pushes_keep_their_identity_without_team_metadata() {
        for (id, properties, channel) in [
            ("19:channel@thread.tacv2", serde_json::json!({}), true),
            ("19:legacy@thread.skype", serde_json::json!({}), true),
            ("opaque-channel", serde_json::json!({"threadType":"topic"}), true),
            ("19:group@thread.v2", serde_json::json!({}), false),
        ] {
            let event = serde_json::json!({"resourceType":"NewMessage", "resource": {
                "id":"1", "conversationLink":format!("https://service/conversations/{}", encode(id)),
                "messagetype":"Text", "content":"hello", "properties":properties
            }});
            let Some(Update::Message { chat, .. }) = message_update(&event, &User::default()) else { panic!() };
            assert_eq!(chat.id, id);
            assert_eq!(chat.channel, channel);
        }
    }
    #[test]
    fn reaction_republishes_parse_counts_without_becoming_new_messages() {
        let mut event = serde_json::json!({"resourceType":"MessageUpdate","resource":{
            "id":"123","conversationLink":"https://service/conversations/a","messagetype":"Text",
            "content":"Test data","from":"8:orgid:other",
            "properties":{"emotions":[{"key":"heart","users":[{"mri":"8:orgid:me"},{"mri":"8:orgid:other"}]}]}
        }});
        let Some(Update::Message { message:Some(message), new, .. }) = message_update(&event, &User { id:"me".into(), ..Default::default() }) else { panic!() };
        assert!(!new);
        assert_eq!(message.reactions[0].users.len(), 2);
        assert!(message.reactions[0].mine("me"));
        event["resource"]["properties"]["emotions"] = serde_json::json!([]);
        let Some(Update::Message { message:Some(message), .. }) = message_update(&event, &User::default()) else { panic!() };
        assert!(message.reactions.is_empty());
    }
    #[test]
    fn decodes_plain_and_packed_payloads() {
        let body = serde_json::json!({"type":"EventMessage"});
        assert_eq!(decode_body(&serde_json::json!({"body":body.to_string()})).unwrap(), body);
        let packed = serde_json::json!({"gp":STANDARD.encode(body.to_string())});
        assert_eq!(decode_body(&serde_json::json!({"body":packed.to_string()})).unwrap(), body);
        assert!(decode_body(&serde_json::json!({"body":"bad json"})).is_err());
    }
}
