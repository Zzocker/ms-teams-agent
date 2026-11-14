//! Teams' private chat service (undocumented; the Teams web client's own API): chats and their messages.

use crate::auth::{Session, User, agent};
use serde_json::Value;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub avatar_id: String,
    pub avatar_members: Vec<User>,
    pub preview: String,
    /// ISO 8601, UTC.
    pub time: String,
    pub unread: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Team {
    pub id: String,
    pub title: String,
    pub channels: Vec<Chat>,
}

pub fn teams(s: &Session) -> Result<Vec<Team>, String> {
    let access = crate::auth::teams_access()?;
    let region = s.middle_tier.rsplit('/').next().unwrap_or("").split('-').next().unwrap_or("");
    if !matches!(region, "amer" | "emea" | "apac") {
        return Err("Unknown Teams service region.".into());
    }
    let v = answer(agent()
        .get(&format!("https://teams.microsoft.com/api/csa/{region}/api/v3/teams/users/me?isPrefetch=false&enableMembershipSummary=true"))
        .header("Authorization", &format!("Bearer {access}"))
        .header("Authentication", &auth(s))
        .header("x-skypetoken", &s.skype_token)
        .call())?;
    parse_teams(&v)
}

fn parse_teams(v: &Value) -> Result<Vec<Team>, String> {
    let teams = v["teams"].as_array().ok_or("Teams response has no team list")?;
    teams.iter().map(|team| {
        let id = ["id", "threadId"].into_iter()
            .find_map(|key| team[key].as_str().filter(|id| !id.trim().is_empty()))
            .ok_or_else(|| format!("Team has no ID (fields: {})", team.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()))?;
        let channels = team["channels"].as_array().ok_or("Team has no channel list")?;
        Ok(Team {
            id: id.into(),
            title: team["displayName"].as_str().unwrap_or("Team").into(),
            channels: channels.iter().map(|channel| {
                let id = ["id", "threadId"].into_iter()
                    .find_map(|key| channel[key].as_str().filter(|id| !id.trim().is_empty()))
                    .ok_or("Channel has no ID")?;
                Ok(Chat {
                    id: id.into(),
                    title: channel["displayName"].as_str().unwrap_or("Channel").into(),
                    avatar_id: String::new(),
                    avatar_members: Vec::new(),
                    preview: String::new(),
                    time: String::new(),
                    unread: false,
                })
            }).collect::<Result<_, String>>()?,
        })
    }).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    /// The client message id the sender gave it (the server's id if none), so a message
    /// shown as it's sent keeps its identity when the server's copy replaces it.
    pub id: String,
    pub server_id: String,
    pub sender: String,
    pub sender_id: String,
    pub text: String,
    pub time: String,
    pub mine: bool,
    pub reply: Option<Reply>,
    /// Preview URLs of the pictures it carries (pasted screenshots, image files), from [`ams_preview`].
    pub images: Vec<String>,
    /// Web links in `text`: byte ranges and their URLs, from [`html_to_rich`].
    pub links: Vec<(Range<usize>, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub id: String,
    pub sender: String,
    pub time: String,
    pub text: String,
}

fn url(s: &Session, path: &str) -> String {
    format!("{}/v1/users/ME/{path}", s.chat_service)
}

fn auth(s: &Session) -> String {
    format!("skypetoken={}", s.skype_token)
}

fn answer(res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    let mut res = res.map_err(|e| e.to_string())?;
    let status = res.status();
    if status == 401 {
        return Err("Teams session expired; restart the app.".into());
    }
    let v: Value = res.body_mut().read_json().map_err(|e| format!("chat service {status}: {e}"))?;
    if !status.is_success() {
        return Err(format!("chat service {status}: {}", v["message"].as_str().unwrap_or("")));
    }
    Ok(v)
}

fn get(s: &Session, path: &str) -> Result<Value, String> {
    answer(agent().get(&url(s, path)).header("Authentication", &auth(s)).call())
}

/// The media store derives the client platform from this and rejects requests it can't parse.
const MEDIA_USER_AGENT: &str = "Teams/24295.605.3225.8804/49";

/// Stores a pasted PNG in the media store, readable by `chat_id`'s members, and returns its
/// [`ams_preview`] URL for the message to show.
pub fn upload_image(s: &Session, chat_id: &str, png: &[u8]) -> Result<String, String> {
    let media = &s.media;
    if media.is_empty() { return Err("no media store for this account".into()); }
    let token = format!("skype_token {}", s.skype_token);
    let body = serde_json::json!({"type": "pish/image", "permissions": {chat_id: ["read"]}});
    let mut res = agent().post(&format!("{media}/v1/objects/")).header("Authorization", &token).header("User-Agent", MEDIA_USER_AGENT)
        .send_json(&body).map_err(|e| e.to_string())?;
    let status = res.status();
    let v: Value = res.body_mut().read_json().map_err(|e| format!("media store {status}: {e}"))?;
    let id = v["id"].as_str().filter(|_| status.is_success()).ok_or(format!("media store {status}"))?;
    let res = agent().put(&format!("{media}/v1/objects/{}/content/imgpsh", encode(id))).header("Authorization", &token)
        .header("User-Agent", MEDIA_USER_AGENT).header("Content-Type", "application/octet-stream").send(png).map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("image upload {}", res.status())); }
    ams_preview(&format!("{media}/v1/objects/{id}/views/imgo")).ok_or("unexpected media store address".into())
}

/// Sends `text` and pictures (from [`upload_image`], with their pixel sizes) to a chat as the signed-in user.
/// `id` is from [`new_msg_id`].
pub fn send(s: &Session, chat_id: &str, id: &str, text: &str, images: &[(String, (u32, u32))]) -> Result<(), String> {
    let body = serde_json::json!({
        "content": message_html(text, images),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "clientmessageid": id,
        "imdisplayname": s.user.name,
    });
    answer(agent().post(&url(s, &format!("conversations/{}/messages", encode(chat_id)))).header("Authentication", &auth(s)).send_json(&body)).map(|_| ())
}

/// A fresh client message id. The service drops a repeat of one, so it must differ per message.
pub fn new_msg_id() -> String {
    // Nanoseconds: two sends in one millisecond still differ.
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_string()
}

/// Now as the service writes `composetime`: `2026-11-14T10:23:45.123Z`.
pub fn now() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    iso(ms)
}

fn iso(ms: i64) -> String {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's days_from_civil, inverted).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

/// `fresh` (the latest page) with the rows of `old` from before it kept above it, so a reload
/// doesn't drop pages scrolled back to.
pub fn merge(old: Page, fresh: Page) -> Page {
    let Some(first) = fresh.msgs.first() else { return fresh };
    let mut msgs: Vec<Msg> = old.msgs.into_iter()
        .filter(|m| m.time < first.time && !fresh.msgs.iter().any(|f| f.id == m.id))
        .collect();
    // Kept rows reach back to `old`'s start, so its link is the one before them.
    let older = if msgs.is_empty() { fresh.older } else { old.older };
    msgs.extend(fresh.msgs);
    Page { msgs, older }
}

/// Plain text as the HTML a Teams message carries; what [`html_to_text`] undoes.
/// A message's HTML: the text, then each picture as Teams sends a pasted one.
fn message_html(text: &str, images: &[(String, (u32, u32))]) -> String {
    images.iter().fold(text_to_html(text), |mut html, (url, (w, h))| {
        html.push_str(&format!(r#"<div><img src="{url}" itemscope="" itemtype="http://schema.skype.com/AMSImage" style="width:{w}px; height:{h}px"></div>"#));
        html
    })
}

pub fn text_to_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>")
}

/// The chats with guessed titles; [`name_chats`] gives them their people's names.
pub fn chats(s: &Session) -> Result<Vec<Chat>, String> {
    Ok(parse_chats(&get(s, "conversations?view=msnp24Equivalent&pageSize=100")?, &s.user))
}

/// Resolves names and group avatar members after the initial chat list shows.
pub fn name_chats(s: &Session, mut chats: Vec<Chat>) -> Vec<Chat> {
    let me = format!("8:orgid:{}", s.user.id);
    // Chats carry no names. Who each is named after: a 1:1's other person (in its id), or the
    // group members for their avatar collage and untitled chat names.
    // The member lookups run at once: one after another they add up.
    let people: Vec<Vec<String>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = chats.iter().map(|c| match peer(&c.id, &s.user.id) {
            Some(_) if c.avatar_id.starts_with("28:") => Ok(vec![c.avatar_id.clone()]),
            Some(p) => Ok(vec![format!("8:orgid:{p}")]),
            None if c.id.ends_with("@thread.v2") => Err(scope.spawn(|| members(s, &c.id).unwrap_or_default().into_iter().filter(|m| *m != me).collect())),
            None => Ok(vec![]),
        }).collect();
        jobs.into_iter().map(|j| j.unwrap_or_else(|h| h.join().unwrap_or_default())).collect()
    });
    for (chat, people) in chats.iter_mut().zip(&people) {
        if chat.id.ends_with("@thread.v2") {
            chat.avatar_members = people.iter().take(3).map(|mri| User {
                id: avatar_id(mri), name: "?".into(), ..User::default()
            }).collect();
        }
    }
    let mut all: Vec<&String> = people.iter().flatten().collect();
    all.sort();
    all.dedup();
    if all.is_empty() {
        return chats;
    }
    // A failed lookup isn't worth losing the chat list over: the guessed titles stay.
    let Ok(v) = answer(agent()
        .post(&format!("{}/beta/users/fetchShortProfile?isMailAddress=false&enableGuest=true&skypeTeamsInfo=true", s.middle_tier))
        .header("Authorization", &format!("Bearer {}", s.access))
        .send_json(&all))
    else {
        return chats;
    };
    let empty = vec![];
    let profiles = v["value"].as_array().unwrap_or(&empty);
    let name = |mri: &str| profiles.iter().find(|p| p["mri"] == mri).and_then(|p| p["displayName"].as_str()).filter(|n| !n.is_empty());
    for (c, people) in chats.iter_mut().zip(&people) {
        let names: Vec<&str> = people.iter().filter_map(|p| name(p)).collect();
        for member in &mut c.avatar_members {
            let mri = if member.id.starts_with("28:") { member.id.clone() } else { format!("8:orgid:{}", member.id) };
            if let Some(name) = name(&mri) { member.name = name.into(); }
        }
        if !names.is_empty() && (c.title == UNTITLED || peer(&c.id, &s.user.id).is_some()) {
            c.title = names.join(", ");
        }
    }
    chats
}

/// A chat's members' ids, e.g. `8:orgid:{oid}`.
fn members(s: &Session, chat_id: &str) -> Result<Vec<String>, String> {
    let v = answer(agent().get(&format!("{}/v1/threads/{}?view=msnp24Equivalent", s.chat_service, encode(chat_id))).header("Authentication", &auth(s)).call())?;
    let empty = vec![];
    Ok(v["members"].as_array().unwrap_or(&empty).iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
}

/// Preserve bot MRIs: their GUIDs are not Entra user IDs.
fn avatar_id(from: &str) -> String {
    let mri = from.rsplit('/').next().unwrap_or("");
    if let Some(id) = mri.strip_prefix("8:orgid:") { id.into() }
    else if mri.starts_with("28:") { mri.into() }
    else { String::new() }
}

/// The other person's object id in a 1:1 chat, whose id is `19:{oid}_{oid}@unq.gbl.spaces`.
pub fn peer<'a>(chat_id: &'a str, me: &str) -> Option<&'a str> {
    let (a, b) = chat_id.strip_prefix("19:")?.strip_suffix("@unq.gbl.spaces")?.split_once('_')?;
    Some(if a == me { b } else { a })
}

/// A run of messages, oldest first, and the link to the page before it (`None` at the start of the chat).
#[derive(Clone)]
pub struct Page {
    pub msgs: Vec<Msg>,
    pub older: Option<String>,
}

pub fn messages(s: &Session, chat_id: &str) -> Result<Page, String> {
    let v = get(s, &format!("conversations/{}/messages?view=msnp24Equivalent&pageSize=50&startTime=1", encode(chat_id)))?;
    Ok(page(&v, &s.user.id))
}

pub fn mark_read(s: &Session, chat_id: &str, page: &Page) -> Result<(), String> {
    let Some(last) = page.msgs.iter().filter(|m| m.server_id.parse::<u64>().is_ok())
        .max_by_key(|m| m.server_id.parse::<u64>().unwrap_or(0)) else {
        return Err("No message ID available for the read marker.".into());
    };
    let body = read_marker(last, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis());
    let response = agent().put(&url(s, &format!("conversations/{}/properties?name=consumptionhorizon", encode(chat_id))))
        .header("Authentication", &auth(s)).send_json(&body).map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Could not save read status: {}", response.status()));
    }
    Ok(())
}

fn read_marker(message: &Msg, read_at: u128) -> Value {
    serde_json::json!({"consumptionhorizon": format!("{};{read_at};{}", message.server_id, message.id)})
}

/// The page `link` (a [`Page::older`]) points to.
pub fn older(s: &Session, link: &str) -> Result<Page, String> {
    // The link carries our token, so only follow it to the chat service.
    if !link.starts_with(&s.chat_service) {
        return Err("unexpected link for older messages".into());
    }
    Ok(page(&answer(agent().get(link).header("Authentication", &auth(s)).call())?, &s.user.id))
}

fn page(v: &Value, me: &str) -> Page {
    Page { msgs: parse_messages(v, me), older: v["_metadata"]["backwardLink"].as_str().map(Into::into) }
}

/// The chat with yourself, which Teams shows as "Name (You)".
const NOTES: &str = "48:notes";
/// A chat with no topic and nobody else's last message to name it after.
const UNTITLED: &str = "Chat";

pub fn parse_chats(v: &Value, me: &User) -> Vec<Chat> {
    let empty = vec![];
    let mut chats: Vec<Chat> = v["conversations"].as_array().unwrap_or(&empty).iter()
        .filter(|c| {
            let id = c["id"].as_str().unwrap_or("");
            // Other 48: ids are system threads (notifications, call log, drafts); space/topic are team channels.
            !id.is_empty() && (id == NOTES || !id.starts_with("48:")) && !matches!(c["threadProperties"]["threadType"].as_str(), Some("space" | "topic"))
        })
        .map(|c| {
            let last = &c["lastMessage"];
            let topic = c["threadProperties"]["topic"].as_str().unwrap_or("").trim();
            let sender = last["imdisplayname"].as_str().unwrap_or("");
            // shortcut: an untitled chat is named after its last sender who isn't me; the proper name needs a member lookup per chat.
            let you = format!("{} (You)", me.name);
            let title = if c["id"] == NOTES {
                &you
            } else if !topic.is_empty() {
                topic
            } else if !sender.is_empty() && !last["from"].as_str().unwrap_or("").ends_with(&me.id) {
                sender
            } else {
                UNTITLED
            };
            Chat {
                id: c["id"].as_str().unwrap_or("").into(),
                title: title.into(),
                avatar_members: Vec::new(),
                avatar_id: {
                    let sender = avatar_id(last["from"].as_str().unwrap_or(""));
                    if sender.starts_with("28:") { sender } else { String::new() }
                },
                preview: {
                    let content = html_to_text(last["content"].as_str().unwrap_or(""));
                    let text = content.lines().next().unwrap_or("");
                    let sender = if last["from"].as_str().is_some_and(|from| avatar_id(from) == me.id) {
                        "You"
                    } else if peer(c["id"].as_str().unwrap_or(""), &me.id).is_some() {
                        ""
                    } else {
                        sender.trim()
                    };
                    if text.is_empty() || sender.is_empty() { text.into() } else { format!("{sender}: {text}") }
                },
                time: last["composetime"].as_str().unwrap_or("").into(),
                unread: chat_unread(c, &me.id),
            }
        })
        .collect();
    chats.sort_by(|a, b| b.time.cmp(&a.time));
    chats
}

fn chat_unread(chat: &Value, me: &str) -> bool {
    let last = &chat["lastMessage"];
    if last["from"].as_str().is_none_or(|from| from.rsplit(':').next() == Some(me)) {
        return false;
    }
    let read = chat["properties"]["consumptionhorizon"].as_str()
        .and_then(|h| h.split(';').next())
        .and_then(|id| id.parse::<u64>().ok());
    let latest = last["id"].as_str().and_then(|id| id.parse::<u64>().ok());
    matches!((read, latest), (Some(read), Some(latest)) if latest > read)
}

/// Oldest first; only messages people wrote (not typing, joins, renames, calls).
pub fn parse_messages(v: &Value, me: &str) -> Vec<Msg> {
    let empty = vec![];
    let mut msgs: Vec<Msg> = v["messages"].as_array().unwrap_or(&empty).iter()
        .filter(|m| matches!(m["messagetype"].as_str(), Some(t) if t == "Text" || t.starts_with("RichText")))
        .filter_map(|m| {
            let raw = m["content"].as_str().unwrap_or("");
            if raw.is_empty() {
                return None; // deleted
            }
            let (reply, (text, links)) = reply_content(raw);
            let images = images(m, raw);
            Some(Msg {
                id: m["clientmessageid"].as_str().or(m["id"].as_str()).unwrap_or("").into(),
                server_id: m["id"].as_str().unwrap_or("").into(),
                sender: m["imdisplayname"].as_str().unwrap_or("").into(),
                sender_id: avatar_id(m["from"].as_str().unwrap_or("")),
                text: if text.is_empty() && reply.is_none() && images.is_empty() { "[attachment]".into() } else { text },
                time: m["composetime"].as_str().unwrap_or("").into(),
                mine: !me.is_empty() && m["from"].as_str().unwrap_or("").ends_with(me),
                reply,
                images,
                links,
            })
        })
        .collect();
    msgs.sort_by(|a, b| a.time.cmp(&b.time));
    msgs
}

/// Pictures pasted into the message body, then image files' previews, as [`ams_preview`] URLs.
fn images(m: &Value, raw: &str) -> Vec<String> {
    let inline = raw.split("<img").skip(1).filter_map(|tag| {
        let tag = tag.split('>').next()?;
        tag.contains("schema.skype.com/AMSImage").then(|| tag.split_once("src=\"")?.1.split('"').next()).flatten()
    });
    let files: Vec<Value> = m["properties"]["files"].as_str().and_then(|f| serde_json::from_str(f).ok()).unwrap_or_default();
    let previews = files.iter().filter_map(|f| f["filePreview"]["previewUrl"].as_str());
    inline.chain(previews).filter_map(ams_preview).collect()
}

/// The small preview view of a picture in Teams' media store, or `None` for any other URL:
/// fetching it sends the Teams token, so only these hosts may get it.
pub fn ams_preview(url: &str) -> Option<String> {
    let (host, path) = url.strip_prefix("https://")?.split_once('/')?;
    let id = path.strip_prefix("v1/objects/")?.split('/').next()?;
    let host_ok = host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && (host.ends_with(".asm.skype.com") || host.ends_with(".asyncgw.teams.microsoft.com"));
    let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    (host_ok && id_ok).then(|| format!("https://{host}/v1/objects/{id}/views/imgo"))
}

/// Split Teams' embedded reply preview from the new message; malformed HTML stays visible.
fn reply_content(raw: &str) -> (Option<Reply>, (String, Vec<(Range<usize>, String)>)) {
    let parse = || {
        let start = raw.find("<blockquote")?;
        let open_end = start + raw[start..].find('>')?;
        if !raw[start..open_end].contains("schema.skype.com/Reply") { return None; }
        let end = open_end + raw[open_end..].find("</blockquote>")?;
        let id = raw[start..open_end].split_once("itemid=").and_then(|(_, tail)| {
            let quote = tail.chars().next()?;
            matches!(quote, '\'' | '"').then(|| tail[1..].split(quote).next().unwrap_or("").to_owned())
        }).unwrap_or_default();
        let quoted = &raw[open_end + 1..end];
        // shortcut: nested reply previews stay plain text; extend parsing if Teams sends them.
        if quoted.contains("<blockquote") { return None; }
        let element = |tag: &str| -> Option<(&str, &str)> {
            let start = quoted.find(&format!("<{tag}"))?;
            let open_end = start + quoted[start..].find('>')?;
            let end = open_end + quoted[open_end..].find(&format!("</{tag}>"))?;
            Some((&quoted[start..open_end], &quoted[open_end + 1..end]))
        };
        let sender = element("strong").map(|(_, body)| html_to_text(body)).unwrap_or_default();
        let time = element("span").and_then(|(tag, _)| {
            let tail = tag.split_once("itemid=")?.1;
            let quote = tail.chars().next()?;
            if !matches!(quote, '\'' | '"') { return None; }
            tail[1..].split(quote).next()?.parse::<i64>().ok().filter(|ms| *ms >= 0).map(iso)
        }).unwrap_or_default();
        let text = html_to_text(&quoted[quoted.find("<p")?..]);
        let mut body = raw[..start].to_owned();
        body.push_str(&raw[end + "</blockquote>".len()..]);
        Some((Some(Reply { id, sender, time, text }), html_to_rich(&body)))
    };
    parse().unwrap_or_else(|| (None, html_to_rich(raw)))
}

/// Teams message HTML as plain text: tags dropped, block ends as newlines, common entities decoded.
pub fn html_to_text(html: &str) -> String {
    html_to_rich(html).0
}

/// [`html_to_text`] plus its `http(s)` links: byte ranges of the text and where they point.
// shortcut: no formatting or mentions; extend here when the message view gets them.
pub fn html_to_rich(html: &str) -> (String, Vec<(Range<usize>, String)>) {
    let decode = |s: &str| s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    let mut out = String::new();
    let mut links = Vec::new();
    let mut open: Option<(usize, String)> = None;
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        // Newlines between tags are source formatting, not content; keeping them doubles every line break.
        if !(rest[..i].trim().is_empty() && rest[..i].contains('\n')) {
            out.push_str(&decode(&rest[..i]));
        }
        let Some(j) = rest[i..].find('>') else { break };
        let inner = &rest[i + 1..i + j];
        let tag = inner.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
        if matches!(tag.as_str(), "br" | "p" | "div" | "li" | "tr") && !out.ends_with('\n') {
            out.push('\n');
        }
        if tag == "a" {
            if inner.starts_with('/') {
                if let Some((start, href)) = open.take().filter(|(start, _)| out.len() > *start) {
                    links.push((start..out.len(), href));
                }
            } else {
                // Only web links: the URL goes to the system's opener on a click.
                open = inner.split_once("href=\"").and_then(|(_, v)| v.split('"').next()).map(decode)
                    .filter(|h| h.starts_with("https://") || h.starts_with("http://"))
                    .map(|h| (out.len(), h));
            }
        }
        rest = &rest[i + j + 1..];
    }
    if !rest.contains('<') {
        out.push_str(&decode(rest));
    }
    let lead = out.len() - out.trim_start().len();
    let text = out.trim().to_string();
    let links = links.into_iter()
        .map(|(r, h)| (r.start.saturating_sub(lead).min(text.len())..r.end.saturating_sub(lead).min(text.len()), h))
        .filter(|(r, _)| !r.is_empty())
        .collect();
    (text, links)
}

/// A path segment, percent-encoded as JavaScript's `encodeURIComponent` does.
fn encode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn bot_avatar_identity_is_preserved() {
        let mri = "28:ca9e26b7-dce5-44a0-b2b7-a70a3d65ce25";
        let from = format!("https://chat/v1/users/ME/contacts/{mri}");
        let chats = super::parse_chats(&serde_json::json!({"conversations": [{
            "id": "19:me_bot@unq.gbl.spaces",
            "lastMessage": {"from": from, "imdisplayname": "GitHub Notifications"}
        }]}), &crate::auth::User::default());
        assert_eq!(chats[0].avatar_id, mri);
        let messages = super::parse_messages(&serde_json::json!({"messages": [{
            "from": from, "messagetype": "Text", "content": "Notification"
        }]}), "me");
        assert_eq!(messages[0].sender_id, mri);
    }

    #[test]
    fn read_marker_keeps_server_and_client_message_ids_separate() {
        let message = super::Msg {
            id: "client-123".into(), server_id: "101".into(), sender: "Other".into(), sender_id: String::new(),
            text: "Hello".into(), time: String::new(), mine: false, reply: None, images: vec![], links: vec![],
        };
        let marker = super::read_marker(&message, 999);
        assert_eq!(marker["consumptionhorizon"], "101;999;client-123");
        let chat = serde_json::json!({
            "properties": marker,
            "lastMessage": {"id": "101", "from": "8:orgid:other"}
        });
        assert!(!super::chat_unread(&chat, "me"));
    }

    #[test]
    fn unread_chats_compare_server_message_ids_with_read_horizon() {
        let mut chat = serde_json::json!({
            "properties": {"consumptionhorizon": "100;999;50"},
            "lastMessage": {"id": "101", "from": "8:orgid:other"}
        });
        assert!(super::chat_unread(&chat, "me"));
        chat["lastMessage"]["id"] = "100".into();
        assert!(!super::chat_unread(&chat, "me"));
        chat["lastMessage"]["id"] = "99".into();
        assert!(!super::chat_unread(&chat, "me"));
        chat["lastMessage"]["id"] = "101".into();
        chat["lastMessage"]["from"] = "8:orgid:me".into();
        assert!(!super::chat_unread(&chat, "me"));
        chat["lastMessage"]["from"] = "8:orgid:other".into();
        chat["properties"]["consumptionhorizon"] = "invalid".into();
        assert!(!super::chat_unread(&chat, "me"));
        chat["properties"] = serde_json::Value::Null;
        assert!(!super::chat_unread(&chat, "me"));
    }

    use super::*;
    use serde_json::json;

    const ME: &str = "8:orgid:me-oid";

    #[test]
    fn teams_keep_channels_under_their_team() {
        let v = json!({"teams": [
            {"threadId": "development", "displayName": "Development", "channels": [
                {"id": "backend", "displayName": "Backend"},
                {"id": "general", "displayName": "General", "isFavorite": false}
            ]},
            {"threadId": "empty", "displayName": "Empty", "channels": []}
        ]});
        let teams = parse_teams(&v).unwrap();
        assert_eq!(teams.len(), 2);
        assert_eq!(teams[0].title, "Development");
        assert_eq!(teams[0].channels.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["backend", "general"]);
        assert!(teams[1].channels.is_empty());
        assert!(parse_teams(&json!({"teams": []})).unwrap().is_empty());
        assert!(parse_teams(&json!({"error": "denied"})).is_err());
        assert!(parse_teams(&json!({"teams": [{"threadId": "x", "channels": [{"displayName": "Missing ID"}]}]})).is_err());
    }

    #[test]
    fn teams_accept_id_and_thread_id_variants() {
        for team_key in ["id", "threadId"] {
            for channel_key in ["id", "threadId"] {
                let mut team = json!({"displayName": "Development", "channels": [{"displayName": "Backend"}]});
                team[team_key] = json!("team-id");
                team["channels"][0][channel_key] = json!("channel-id");
                let teams = parse_teams(&json!({"teams": [team]})).unwrap();
                assert_eq!(teams[0].id, "team-id");
                assert_eq!(teams[0].channels[0].id, "channel-id");
            }
        }
        let teams = parse_teams(&json!({"teams": [{"id": "", "threadId": "fallback", "channels": []}]})).unwrap();
        assert_eq!(teams[0].id, "fallback");
        assert!(parse_teams(&json!({"teams": [{"id": " ", "channels": []}]})).is_err());
    }

    #[test]
    fn chats_skip_system_threads_and_channels_and_name_untitled_chats() {
        let v = json!({"conversations": [
            {"id": "48:notifications", "lastMessage": {}},
            {"id": "48:notes", "lastMessage": {"imdisplayname": "Me", "composetime": "2026-11-12T00:00:00Z"}},
            {"id": "19:c@thread.tacv2", "threadProperties": {"threadType": "space"}, "lastMessage": {}},
            {"id": "19:a_b@unq.gbl.spaces", "lastMessage": {"imdisplayname": "Bob", "from": "https://x/contacts/8:orgid:bob",
                "content": "<p>hi &amp; bye</p><p>second</p>", "composetime": "2026-11-14T10:00:00Z"}},
            {"id": "19:g@thread.v2", "threadProperties": {"topic": "Design"}, "lastMessage": {"composetime": "2026-11-14T11:00:00Z"}},
            {"id": "19:m_b@unq.gbl.spaces", "lastMessage": {"imdisplayname": "Me", "from": "https://x/contacts/8:orgid:me-oid", "composetime": "2026-11-13T00:00:00Z"}},
        ]});
        let c = parse_chats(&v, &User { name: "Me".into(), id: "me-oid".into(), ..User::default() });
        assert_eq!(c.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Design", "Bob", "Chat", "Me (You)"]);
        assert_eq!(c[1].preview, "hi & bye");
        assert_eq!(peer("19:bob_me-oid@unq.gbl.spaces", "me-oid"), Some("bob"));
        assert_eq!(peer("19:me-oid_bob@unq.gbl.spaces", "me-oid"), Some("bob"));
        assert_eq!(peer("19:g@thread.v2", "me-oid"), None);
    }

    #[test]
    fn chat_previews_label_self_and_handle_missing_sender_or_content() {
        let v = json!({"conversations": [
            {"id": "self", "lastMessage": {"from": "https://x/contacts/8:orgid:me", "imdisplayname": "My Name", "content": "Hello"}},
            {"id": "unknown", "lastMessage": {"content": "Hello"}},
            {"id": "empty", "lastMessage": {"imdisplayname": "Bob"}},
            {"id": "19:me_bob@unq.gbl.spaces", "lastMessage": {"from": "8:orgid:me", "imdisplayname": "My Name", "content": "Hello"}},
            {"id": "19:group@thread.v2", "lastMessage": {"from": "8:orgid:bob", "imdisplayname": "Bob", "content": "Hello"}}
        ]});
        let chats = parse_chats(&v, &User { id: "me".into(), ..User::default() });
        assert_eq!(chats.iter().map(|c| c.preview.as_str()).collect::<Vec<_>>(), ["You: Hello", "Hello", "", "You: Hello", "Bob: Hello"]);
    }

    #[test]
    fn html_source_newlines_between_blocks_are_not_blank_lines() {
        let raw = "<p>Date: 15th Nov</p>\r\n<p>Time: 4:00</p>\n<p>&nbsp;</p>\n<p>Reason: x</p>";
        assert_eq!(super::html_to_text(raw), "Date: 15th Nov\nTime: 4:00\n \nReason: x");
    }

    #[test]
    fn quoted_reply_keeps_preview_separate_from_body() {
        let raw = r#"<div><blockquote itemscope="" itemtype="http://schema.skype.com/Reply" itemid="1600457867820"><strong itemprop="mri">Bob &amp; Alice</strong><span itemprop="time" itemid="1600457867820"></span><p itemprop="preview">Original &lt;message&gt;</p><p>Second line</p></blockquote><p>My reply</p></div>"#;
        let v = json!({"messages": [{"messagetype": "RichText/Html", "content": raw}]});
        let msgs = parse_messages(&v, "me");
        assert_eq!(msgs[0].text, "My reply");
        let reply = msgs[0].reply.as_ref().unwrap();
        assert_eq!(reply.id, "1600457867820");
        assert_eq!(reply.sender, "Bob & Alice");
        assert_eq!(reply.time, iso(1600457867820));
        assert_eq!(reply.text, "Original <message>\nSecond line");
        let only_quote = raw.replace("<p>My reply</p>", "");
        assert_eq!(reply_content(&only_quote).1.0, "");
        for plain in ["<p>Normal message</p>", "<blockquote>Ordinary quote</blockquote>", "<blockquote itemtype='http://schema.skype.com/Reply'>Broken"] {
            assert_eq!(reply_content(plain), (None, html_to_rich(plain)));
        }
        let missing_time = raw.replace("itemid=\"1600457867820\"", "");
        assert_eq!(reply_content(&missing_time).0.unwrap().time, "");
        let ids = json!({"messages": [{"id": "server-id", "clientmessageid": "client-id", "messagetype": "Text", "content": "Sent"}]});
        let msgs = parse_messages(&ids, "me");
        assert_eq!(msgs[0].id, "client-id");
        assert_eq!(msgs[0].server_id, "server-id");
    }

    #[test]
    fn messages_keep_only_text_oldest_first() {
        let v = json!({"messages": [
            {"messagetype": "RichText/Html", "content": "<div>two</div>", "imdisplayname": "Me", "from": format!("https://x/contacts/{ME}"), "composetime": "2"},
            {"messagetype": "Control/Typing", "content": "", "composetime": "3"},
            {"messagetype": "ThreadActivity/AddMember", "content": "<addmember/>", "composetime": "1"},
            {"messagetype": "Text", "content": "", "composetime": "4"},
            {"messagetype": "Text", "content": "one", "imdisplayname": "Bob", "from": "https://x/contacts/8:orgid:bob", "composetime": "1"},
        ]});
        let m = parse_messages(&v, "me-oid");
        assert_eq!(m[0].sender_id, "bob");
        assert_eq!(m[1].sender_id, "me-oid");
        assert_eq!(m.iter().map(|m| (m.text.as_str(), m.mine)).collect::<Vec<_>>(), [("one", false), ("two", true)]);
    }

    #[test]
    fn images_come_from_inline_pictures_and_file_previews_on_teams_hosts_only() {
        let files = json!([
            {"fileName": "a.png", "filePreview": {"previewUrl": "https://in-api.asm.skype.com/v1/objects/0-cin-d3-41c6/views/imgo"}},
            {"fileName": "b.mov", "filePreview": {"previewUrl": ""}},
            {"fileName": "c.gif", "filePreview": {"previewUrl": "https://evil.example/v1/objects/0-x/views/imgo"}},
        ]).to_string();
        let v = json!({"messages": [
            {"messagetype": "RichText/Html", "composetime": "1", "content": r#"<div><img src="https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-sin-d2-650d/views/imgo" itemscope="" itemtype="http://schema.skype.com/AMSImage" style="width:1170px"><img src="https://x/emoji.png" itemtype="http://schema.skype.com/Emoji"></div>"#},
            {"messagetype": "RichText/Html", "composetime": "2", "content": "<p>Github actions is down</p>", "properties": {"files": files}},
        ]});
        let m = parse_messages(&v, "me");
        assert_eq!(m[0].text, "");
        assert_eq!(m[0].images, ["https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-sin-d2-650d/views/imgo"]);
        assert_eq!(m[1].text, "Github actions is down");
        assert_eq!(m[1].images, ["https://in-api.asm.skype.com/v1/objects/0-cin-d3-41c6/views/imgo"]);
        assert_eq!(ams_preview("https://in-api.asm.skype.com/v1/objects/0-a/views/imgpsh_mobile_save_anim").as_deref(), Some("https://in-api.asm.skype.com/v1/objects/0-a/views/imgo"));
        for bad in ["http://in-api.asm.skype.com/v1/objects/0-a/views/imgo", "https://a@evil.com/x.asm.skype.com/v1/objects/0-a",
            "https://evil.com?.asm.skype.com/v1/objects/0-a", "https://in-api.asm.skype.com/v1/objects/../x", "https://in-api.asm.skype.com/v2/objects/0-a"] {
            assert_eq!(ams_preview(bad), None, "{bad}");
        }
    }

    #[test]
    fn sent_pictures_parse_back_as_the_same_pictures() {
        let url = "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-sin-d2-ab12/views/imgo".to_owned();
        let html = message_html("look <here>", &[(url.clone(), (1170, 2532))]);
        let m = parse_messages(&json!({"messages": [{"messagetype": "RichText/Html", "content": html, "composetime": "1"}]}), "me");
        assert_eq!(m[0].text, "look <here>");
        assert_eq!(m[0].images, [url]);
        assert_eq!(message_html("", &[]), "");
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_794_673_185_538), "2026-11-14T16:19:45.538Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn merge_keeps_older_rows_and_swaps_the_sent_copy() {
        let m = |id: &str, time: &str| Msg { id: id.into(), server_id: id.into(), sender: String::new(), sender_id: String::new(), text: id.into(), time: time.into(), mine: false, reply: None, images: vec![], links: vec![] };
        let old = Page { msgs: vec![m("a", "1"), m("b", "2"), m("c", "3"), m("sent", "9")], older: Some("before-a".into()) };
        let fresh = Page { msgs: vec![m("b", "2"), m("c", "3"), m("sent", "4")], older: Some("before-b".into()) };
        let p = merge(old, fresh);
        assert_eq!(p.msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "sent"]);
        assert_eq!(p.older.as_deref(), Some("before-a"));
    }

    #[test]
    fn html() {
        assert_eq!(html_to_text("<p>a<br>b</p><p>c &lt;d&gt;</p>"), "a\nb\nc <d>");
        assert_eq!(html_to_text("<img src=\"x\">"), "");
        assert_eq!(html_to_text("plain &amp;lt;"), "plain &lt;");
        let typed = "a <b> & c\nd &lt;";
        assert_eq!(text_to_html(typed), "a &lt;b&gt; &amp; c<br>d &amp;lt;");
        assert_eq!(html_to_text(&text_to_html(typed)), typed);
        assert_eq!(encode("19:a_b@unq.gbl.spaces"), "19%3Aa_b%40unq.gbl.spaces");
    }

    #[test]
    fn links_keep_their_text_range_and_only_web_urls() {
        // A real Teams link: the href's entities decode, and the range covers the shown URL.
        let raw = r#"<p>see <a href="https://x.example/?a=1&amp;b=2" itemtype="http://schema.skype.com/HyperLink" title="t">https://x.example/?a=1&amp;b=2</a> &amp; <a href="javascript:alert(1)">js</a></p>"#;
        let (text, links) = super::html_to_rich(raw);
        assert_eq!(text, "see https://x.example/?a=1&b=2 & js");
        assert_eq!(links.len(), 1);
        assert_eq!(&text[links[0].0.clone()], "https://x.example/?a=1&b=2");
        assert_eq!(links[0].1, "https://x.example/?a=1&b=2");
    }
}

