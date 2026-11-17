use super::*;

pub const REACTIONS: &[(&str, &str)] = &[
    ("like", "👍"), ("heart", "❤️"), ("laugh", "😂"),
    ("surprised", "😮"), ("sad", "😢"), ("angry", "😠"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Reaction {
    pub key: String,
    pub users: Vec<String>,
}

impl Reaction {
    pub fn mine(&self, me: &str) -> bool {
        !me.is_empty() && self.users.iter().any(|u| u.rsplit(':').next() == Some(me))
    }

    pub fn emoji(&self) -> &str {
        // shortcut: uncommon/custom emoji show their server key; add a catalog when those need artwork.
        REACTIONS.iter().find(|(key, _)| *key == self.key).map_or(&self.key, |(_, emoji)| emoji)
    }
}

pub fn parse_reactions(properties: &Value) -> Vec<Reaction> {
    let mut result = Vec::new();
    let value = &properties["emotions"];
    let decoded = value.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok());
    for entry in decoded.as_ref().unwrap_or(value).as_array().into_iter().flatten() {
        let Some(key) = entry["key"].as_str().filter(|k| !k.is_empty()) else { continue };
        let mut users = Vec::new();
        for user in entry["users"].as_array().into_iter().flatten() {
            if let Some(mri) = user["mri"].as_str().filter(|u| !u.is_empty())
                && !users.iter().any(|u| u == mri) { users.push(mri.to_owned()); }
        }
        if !users.is_empty() { result.push(Reaction { key: key.into(), users }); }
    }
    result
}

pub fn react(s: &Session, chat: &str, message: &str, key: &str, remove: bool) -> Result<(), String> {
    if chat.is_empty() || message.is_empty() || key.is_empty() || key.len() > 256 {
        return Err("Invalid reaction target.".into());
    }
    let url = url(s, &format!("conversations/{}/messages/{}/properties?name=emotions", encode(chat), encode(message)));
    let mut body = serde_json::json!({"emotions":{"key":key}});
    if !remove { body["emotions"]["value"] = serde_json::json!(chrono::Utc::now().timestamp_millis()); }
    let response = if remove {
        agent().delete(&url).header("Authentication", &auth(s)).force_send_body().send_json(&body)
    } else {
        agent().put(&url).header("Authentication", &auth(s)).send_json(&body)
    };
    edit_answer(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unique_users_and_string_encoded_emotions() {
        let props = serde_json::json!({"emotions":[{"key":"like","users":[{"mri":"8:orgid:me"},{"mri":"8:orgid:me"},{"mri":"8:orgid:other"},{}]}]});
        let reactions = parse_reactions(&props);
        assert_eq!(reactions[0].users.len(), 2);
        assert!(reactions[0].mine("me"));
        assert!(!reactions[0].mine("e"));
        assert_eq!(reactions[0].emoji(), "👍");
        assert_eq!(parse_reactions(&serde_json::json!({"emotions":props["emotions"].to_string()})), reactions);
        assert!(parse_reactions(&serde_json::json!({"emotions":"bad"})).is_empty());
    }
}
