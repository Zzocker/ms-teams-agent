//! Person mentions in Teams' private chat-service format.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Mention {
    pub range: Range<usize>,
    pub mri: String,
}

pub fn person_mri(mri: &str) -> bool {
    mri.strip_prefix("8:orgid:").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

/// Resolve the whole conversation, rather than the three members kept for its avatar.
pub fn participants(s: &Session, chat: &str) -> Result<Vec<User>, String> {
    let ids = match peer(chat, &s.user.id) {
        Some(id) => vec![format!("8:orgid:{id}")],
        None => members(s, chat)?,
    };
    let ids: Vec<_> = ids.into_iter().filter(|id| person_mri(id)).collect();
    if ids.is_empty() { return Ok(vec![]); }
    let v = answer(agent().post(&format!("{}/beta/users/fetchShortProfile?isMailAddress=false&enableGuest=true&skypeTeamsInfo=true", s.middle_tier))
        .header("Authorization", &format!("Bearer {}", s.access)).send_json(&ids))?;
    let profiles = v["value"].as_array().ok_or("Could not read conversation members.")?;
    let mut users: Vec<_> = profiles.iter().filter_map(|p| {
        let id = p["mri"].as_str()?;
        let name = p["displayName"].as_str()?.trim();
        (ids.iter().any(|known| known == id) && !name.is_empty()).then(|| User {
            id: id.into(), name: name.into(), email: p["email"].as_str().unwrap_or("").into(),
        })
    }).collect();
    users.sort_by_key(|u| u.name.to_lowercase());
    users.dedup_by(|a, b| a.id == b.id);
    Ok(users)
}

/// Placeholders keep code/link conversion intact and never come from the user's text.
pub(super) fn mention_html(text: &str, mentions: &[Mention]) -> String {
    let mut prefix = "TEAMSMENTION".to_string();
    while text.contains(&prefix) { prefix.push('X'); }
    let mut source = String::new();
    let mut at = 0;
    for (i, m) in mentions.iter().enumerate() {
        if let Some(before) = text.get(at..m.range.start) {
            source.push_str(before);
            source.push_str(&format!("{prefix}{i}END"));
            at = m.range.end;
        }
    }
    source.push_str(text.get(at..).unwrap_or(""));
    let mut html = text_to_html(&source);
    for (i, m) in mentions.iter().enumerate() {
        if let Some(label) = text.get(m.range.clone()) {
            let label = label.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
            html = html.replace(&format!("{prefix}{i}END"), &format!(r#"<span itemscope="" itemtype="http://schema.skype.com/Mention" itemid="{i}">{label}</span>"#));
        }
    }
    html
}

pub(super) fn apply_mentions(body: &mut Value, text: &str, mentions: &[Mention]) -> Result<(), String> {
    let mut end = 0;
    let mut metadata = Vec::new();
    for (i, m) in mentions.iter().enumerate() {
        let label = text.get(m.range.clone()).filter(|label| !label.is_empty() && !label.contains(['\n', '\r']))
            .ok_or("Invalid mention text.")?;
        if m.range.start < end || !person_mri(&m.mri) { return Err("Invalid mention identity or range.".into()); }
        end = m.range.end;
        metadata.push(serde_json::json!({"@type":"http://schema.skype.com/Mention", "itemid":i.to_string(),
            "mri":m.mri, "mentionType":"person", "displayName":label.trim_start_matches('@')}));
    }
    body["properties"]["mentions"] = serde_json::json!(serde_json::to_string(&metadata).map_err(|e| e.to_string())?);
    Ok(())
}

pub(super) fn parsed_mentions(m: &Value, links: &mut Vec<(Range<usize>, String)>) -> Vec<Mention> {
    let metadata: Vec<Value> = m["properties"]["mentions"].as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let mut mentions = Vec::new();
    links.retain(|(range, url)| {
        let Some(id) = url.strip_prefix("mention:") else { return true };
        if let Some(m) = metadata.iter().find(|m| m["itemid"].as_str().map(String::from)
            .or_else(|| m["itemid"].as_u64().map(|n| n.to_string())).as_deref() == Some(id))
            && let Some(mri) = m["mri"].as_str().filter(|mri| person_mri(mri)) {
            mentions.push(Mention { range: range.clone(), mri: mri.into() });
        }
        false
    });
    mentions
}

#[cfg(test)]
mod tests {
    use super::*;
    const MRI: &str = "8:orgid:11111111-2222-3333-4444-555555555555";

    #[test]
    fn mentions_round_trip_with_replies_links_unicode_and_edits() {
        let text = "你好 @Alice <&> and @Alice <&> https://example.com\n```rs\nlet x = 1;\n```";
        let label = "@Alice <&>";
        let first = text.find(label).unwrap();
        let second = text.rfind(label).unwrap();
        let mentions = vec![Mention { range: first..first + label.len(), mri: MRI.into() },
            Mention { range: second..second + label.len(), mri: "8:orgid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into() }];
        let quote = r#"<blockquote itemtype="http://schema.skype.com/Reply"><strong>Bob</strong><p>@Alice quoted</p></blockquote>"#;
        let mut body = serde_json::json!({"content":edited_html(quote, text, &[], &mentions), "messagetype":"RichText/Html", "id":"1"});
        apply_mentions(&mut body, text, &mentions).unwrap();
        assert!(body["content"].as_str().unwrap().contains("@Alice &lt;&amp;&gt;</span>"));
        let metadata: Value = serde_json::from_str(body["properties"]["mentions"].as_str().unwrap()).unwrap();
        assert_eq!(metadata[0]["mentionType"], "person");
        assert_eq!(metadata[0]["mri"], MRI);
        assert_eq!(metadata[1]["itemid"], "1");
        body["content"] = serde_json::json!(body["content"].as_str().unwrap().replace(">@Alice &lt;&amp;&gt;</span>", "><span>@Alice &lt;&amp;&gt;</span></span>"));
        let messages = parse_messages(&serde_json::json!({"messages":[body]}), "");
        assert_eq!(messages[0].text, text);
        assert_eq!(messages[0].mentions, mentions);
        assert_eq!(messages[0].links.len(), 1);
        assert_eq!(messages[0].reply.as_ref().unwrap().text, "@Alice quoted");
        let mut edit = serde_json::json!({"content":edited_html(&messages[0].raw, "plain @Alice", &[], &[])});
        apply_mentions(&mut edit, "plain @Alice", &[]).unwrap();
        assert_eq!(edit["properties"]["mentions"], "[]");
        assert!(!edit["content"].as_str().unwrap().contains("schema.skype.com/Mention"));
    }

    #[test]
    fn malformed_mentions_are_rejected_or_kept_as_plain_text() {
        for m in [Mention { range: 1..2, mri: MRI.into() }, Mention { range: 0..3, mri: "javascript:bad".into() }] {
            assert!(apply_mentions(&mut serde_json::json!({}), "你好", &[m]).is_err());
        }
        let m = Mention { range: 0..1, mri: MRI.into() };
        assert!(apply_mentions(&mut serde_json::json!({}), "aa", &[m.clone(), m]).is_err());
        let body = serde_json::json!({"messagetype":"RichText/Html", "content":mention_html("@A", &[Mention { range:0..2, mri:MRI.into() }]), "properties":{"mentions":"invalid"}});
        let messages = parse_messages(&serde_json::json!({"messages":[body]}), "");
        assert_eq!(messages[0].text, "@A");
        assert!(messages[0].mentions.is_empty());
        assert!(messages[0].links.is_empty());
    }
}
