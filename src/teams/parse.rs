//! The chat service's JSON as chats, teams and messages.

use super::*;

pub(super) fn parse_teams(v: &Value) -> Result<Vec<Team>, String> {
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
                    channel: true,
                    title: channel["displayName"].as_str().unwrap_or("Channel").into(),
                    avatar_id: String::new(),
                    avatar_members: Vec::new(),
                    preview: String::new(),
                    time: String::new(),
                    unread: false,
                    latest: None,
                })
            }).collect::<Result<_, String>>()?,
        })
    }).collect()
}

/// Preserve bot MRIs: their GUIDs are not Entra user IDs.
pub(super) fn avatar_id(from: &str) -> String {
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

/// The chat with yourself, which Teams shows as "Name (You)".
pub(super) const NOTES: &str = "48:notes";
/// A chat with no topic and nobody else's last message to name it after.
pub(super) const UNTITLED: &str = "Chat";

pub fn parse_chats(v: &Value, me: &User) -> Vec<Chat> {
    parse_conversations(v, me).into_iter().filter(|c| !c.channel).collect()
}

pub(super) fn parse_conversations(v: &Value, me: &User) -> Vec<Chat> {
    let empty = vec![];
    let mut chats: Vec<Chat> = v["conversations"].as_array().unwrap_or(&empty).iter()
        .filter(|c| {
            let id = c["id"].as_str().unwrap_or("");
            // Other 48: ids are system threads (notifications, call log, drafts).
            !id.is_empty() && (id == NOTES || !id.starts_with("48:"))
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
                channel: c["id"].as_str().is_some_and(|id| id.ends_with("@thread.tacv2") || id.ends_with("@thread.skype"))
                    || matches!(c["threadProperties"]["threadType"].as_str(), Some("space" | "topic")),
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
                latest: ReadHorizon::new(last["id"].as_str().unwrap_or(""), last["clientmessageid"].as_str().unwrap_or("")),
            }
        })
        .collect();
    chats.sort_by(|a, b| b.time.cmp(&a.time));
    chats
}

pub(super) fn chat_unread(chat: &Value, me: &str) -> bool {
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
            let deleted = [ &m["deletetime"], &m["properties"]["deletetime"], &m["properties"]["deletionTime"] ].into_iter().any(super::message_flag);
            if raw.is_empty() && !deleted {
                return None; // deleted
            }
            let raw = if deleted { "" } else { raw };
            let (reply, (text, mut links)) = reply_content(raw);
            let mentions = if deleted { vec![] } else { parsed_mentions(m, &mut links) };
            let images = if deleted { vec![] } else { images(m, raw) };
            let files = if deleted { vec![] } else { files(m) };
            Some(Msg {
                id: m["clientmessageid"].as_str().or(m["id"].as_str()).unwrap_or("").into(),
                server_id: m["id"].as_str().unwrap_or("").into(),
                sender: m["imdisplayname"].as_str().unwrap_or("").into(),
                sender_id: avatar_id(m["from"].as_str().unwrap_or("")),
                deleted,
                edited: !deleted && (super::message_flag(&m["properties"]["edittime"]) || super::message_flag(&m["skypeeditedid"])),
                text: if deleted { "This message was deleted.".into() } else if text.is_empty() && reply.is_none() && images.is_empty() && files.is_empty() { "[attachment]".into() } else { text },
                time: m["composetime"].as_str().unwrap_or("").into(),
                mine: !me.is_empty() && m["from"].as_str().unwrap_or("").ends_with(me),
                reply,
                images,
                links,
                files,
                raw: raw.into(), mentions, reactions: if deleted { vec![] } else { parse_reactions(&m["properties"]) },
            })
        })
        .collect();
    msgs.sort_by(|a, b| a.time.cmp(&b.time));
    msgs
}

/// Pictures pasted into the message body, then image files' previews, as [`ams_preview`] URLs.
pub(super) fn images(m: &Value, raw: &str) -> Vec<String> {
    let inline = raw.split("<img").skip(1).filter_map(|tag| {
        let tag = tag.split('>').next()?;
        tag.contains("schema.skype.com/AMSImage").then(|| tag.split_once("src=\"")?.1.split('"').next()).flatten()
    });
    let files: Vec<Value> = m["properties"]["files"].as_str().and_then(|f| serde_json::from_str(f).ok()).unwrap_or_default();
    let previews = files.iter().filter_map(|f| f["filePreview"]["previewUrl"].as_str());
    inline.chain(previews).filter_map(ams_preview).collect()
}

/// Shared files without a media-store picture, once each (Teams can repeat an entry dozens of times).
pub(super) fn files(m: &Value) -> Vec<File> {
    let all: Vec<Value> = m["properties"]["files"].as_str().and_then(|f| serde_json::from_str(f).ok()).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    all.iter()
        .filter(|f| f["filePreview"]["previewUrl"].as_str().and_then(ams_preview).is_none())
        .filter_map(|f| {
            let info = &f["fileInfo"];
            // Some file URLs carry raw spaces, which the system opener refuses.
            let url = [&info["shareUrl"], &f["objectUrl"], &info["fileUrl"]].into_iter()
                .find_map(|v| v.as_str().filter(|u| u.starts_with("https://")))?.replace(' ', "%20");
            Some(File {
                name: f["fileName"].as_str().or(f["title"].as_str())?.into(),
                kind: f["fileType"].as_str().unwrap_or("").to_ascii_lowercase(),
                preview: sharepoint_thumbnail(&url),
                url,
            })
        })
        .filter(|f| seen.insert(f.url.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "8:orgid:me-oid";

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

    #[test]
    fn conversation_list_retains_a_valid_read_horizon_without_loading_messages() {
        let value = json!({"conversations":[{"id":"19:group@thread.v2", "lastMessage":{
            "id":"123", "clientmessageid":"client-456", "content":"hello"
        }}]});
        let chats = parse_chats(&value, &User::default());
        assert_eq!(chats[0].latest, ReadHorizon::new("123", "client-456"));
    }

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
    fn reconnect_conversations_keep_channels_but_chat_list_excludes_them() {
        let value = json!({"conversations":[
            {"id":"19:channel@thread.tacv2"},
            {"id":"19:legacy@thread.skype"},
            {"id":"opaque-channel", "threadProperties":{"threadType":"space"}},
            {"id":"19:group@thread.v2"},
            {"id":"19:me_other@unq.gbl.spaces"}
        ]});
        let all = parse_conversations(&value, &User::default());
        assert_eq!(all.iter().map(|c| c.channel).collect::<Vec<_>>(), [true, true, true, false, false]);
        assert_eq!(parse_chats(&value, &User::default()).len(), 2);
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
    fn files_are_cards_once_each_with_previews_on_sharepoint_only() {
        let loop_ = json!({"fileName": "Notes.loop", "fileType": "loop", "filePreview": {"previewUrl": ""},
            "fileInfo": {"shareUrl": "https://contoso.sharepoint.com/:fl:/g/x?e=1"}, "objectUrl": "https://contoso.sharepoint.com/x.loop"});
        let doc = json!({"fileName": "SLA.docx", "fileType": "DOCX", "fileInfo": {"fileUrl": "https://contoso-my.sharepoint.com/p/My SLA.docx"}});
        let files = json!([
            loop_, doc, doc,
            {"fileName": "a.png", "fileInfo": {"shareUrl": "https://contoso.sharepoint.com/a"}, "filePreview": {"previewUrl": "https://in-api.asm.skype.com/v1/objects/0-a/views/imgo"}},
            {"fileName": "elsewhere.pdf", "fileType": "pdf", "objectUrl": "https://evil.example/f.pdf"},
            {"fileName": "nowhere.pdf", "objectUrl": "javascript:alert(1)"},
        ]).to_string();
        let v = json!({"messages": [{"messagetype": "RichText/Html", "composetime": "1", "content": "<p></p>", "properties": {"files": files}}]});
        let m = &parse_messages(&v, "me")[0];
        assert_eq!(m.text, "", "files aren't an [attachment]");
        let got: Vec<_> = m.files.iter().map(|f| (f.name.as_str(), f.kind.as_str(), f.url.as_str(), f.preview.is_some())).collect();
        assert_eq!(got, [
            ("Notes.loop", "loop", "https://contoso.sharepoint.com/:fl:/g/x?e=1", true),
            ("SLA.docx", "docx", "https://contoso-my.sharepoint.com/p/My%20SLA.docx", true),
            ("elsewhere.pdf", "pdf", "https://evil.example/f.pdf", false),
        ]);
        let preview = m.files[0].preview.as_deref().unwrap();
        assert_eq!(preview, "https://contoso.sharepoint.com/_api/v2.1/shares/u!aHR0cHM6Ly9jb250b3NvLnNoYXJlcG9pbnQuY29tLzpmbDovZy94P2U9MQ/driveItem/thumbnails/0/large/content");
        assert_eq!(thumbnail_host(preview), Some("contoso.sharepoint.com"));
        for bad in ["https://evil.example/_api/v2.1/shares/u!aHR0/driveItem/thumbnails/0/large/content",
            "https://a@evil.com/x.sharepoint.com/_api/v2.1/shares/u!aHR0/driveItem/thumbnails/0/large/content",
            "https://contoso.sharepoint.com/_api/v2.1/shares/u!../../x/driveItem/thumbnails/0/large/content",
            "https://contoso.sharepoint.com/_api/v2.1/me/drive"] {
            assert_eq!(thumbnail_host(bad), None, "{bad}");
        }
        assert_eq!(b64url(b"https://a"), "aHR0cHM6Ly9h");
        assert_eq!(b64url(b"ab"), "YWI");
    }
}
