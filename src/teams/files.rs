//! Document attachments use OneDrive items and explicit chat-member permissions.
use super::*;
use crate::attachments::DraftFile;

const CHAT_FILES: &str = "Microsoft Teams Chat Files";

pub(crate) fn upload_file(s: &Session, chat: &str, draft: &mut DraftFile) -> Result<Value, String> {
    // Files from the old AMS implementation aren't valid Teams document attachments.
    if draft.uploaded.as_ref().is_some_and(|f| f["@type"] != "http://schema.skype.com/File") {
        draft.uploaded = None;
    }
    if let Some(card) = &draft.uploaded && card["permissionScope"] == "SpecificPeople" { return Ok(card.clone()); }
    let file = checked_file(draft)?;
    let recipients = if chat == NOTES {
        vec![]
    } else if let Some(other) = peer(chat, &s.user.id) {
        recipient_ids([format!("8:orgid:{other}")], &s.user.id)?
    } else if chat.ends_with("@thread.v2") {
        recipient_ids(members(s, chat)?, &s.user.id)?
    } else {
        return Err("File attachments are supported in chats; channel file uploads are not available yet.".into());
    };
    let site = match &draft.uploaded {
        Some(card) => card["fileInfo"]["siteUrl"].as_str().ok_or("Uploaded file has no OneDrive site.")?.to_owned(),
        None => {
            let token = crate::auth::graph_access()?;
            let profile = answer(agent().get("https://graph.microsoft.com/v1.0/me?$select=mySite")
                .header("Authorization", &format!("Bearer {token}")).call())?;
            profile["mySite"].as_str().filter(|v| !v.is_empty()).ok_or("This account has no OneDrive site.")?.to_owned()
        }
    };
    let (site, host) = personal_site(&site)?;
    let token = crate::auth::sharepoint_access(&host)?;
    upload_to(&site, &token, &recipients, draft, file)
}

fn checked_file(draft: &DraftFile) -> Result<std::fs::File, String> {
    let file = std::fs::File::open(&draft.path).map_err(|e| format!("{}: {e}", draft.name))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > crate::attachments::MAX_FILE_SIZE { return Err("Choose a regular file of 100 MiB or smaller.".into()); }
    if meta.len() != draft.size || meta.modified().ok() != Some(draft.modified) { return Err(format!("{} changed; remove it and attach it again.", draft.name)); }
    Ok(file)
}

fn personal_site(url: &str) -> Result<(String, String), String> {
    let site = url.trim_end_matches('/');
    let (host, path) = site.strip_prefix("https://").and_then(|v| v.split_once('/')).ok_or("Invalid OneDrive site.")?;
    let user = path.strip_prefix("personal/").ok_or("Invalid OneDrive personal site.")?;
    if !media::sharepoint(host) || user.is_empty() || !user.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
        return Err("Invalid OneDrive site address.".into());
    }
    Ok((site.into(), host.into()))
}

fn recipient_ids(members: impl IntoIterator<Item = String>, me: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for member in members {
        let id = member.strip_prefix("8:orgid:").ok_or("Cannot grant file access to a non-organization chat member.")?;
        if uuid::Uuid::parse_str(id).is_err() { return Err("Invalid chat member ID.".into()); }
        if id != me && !ids.iter().any(|i| i == id) { ids.push(id.to_owned()); }
    }
    Ok(ids)
}

fn upload_name(name: &str) -> Result<String, String> {
    let name: String = name.chars().map(|c| if c.is_control() || "\"*:<>?/\\|".contains(c) { '_' } else { c }).collect();
    let name = name.trim().trim_end_matches(['.', ' ']);
    if name.is_empty() { return Err("Invalid OneDrive file name.".into()); }
    Ok(name.into())
}

fn upload_to(site: &str, token: &str, recipients: &[String], draft: &mut DraftFile, file: std::fs::File) -> Result<Value, String> {
    let bearer = format!("Bearer {token}");
    if draft.uploaded.is_none() {
        let folder = format!("{site}/_api/v2.0/drive/root:/{}", encode(CHAT_FILES));
        let response = agent().get(&folder).header("Authorization", &bearer).call().map_err(|e| e.to_string())?;
        if response.status() == 404 {
            answer(agent().post(&format!("{site}/_api/v2.0/drive/root/children")).header("Authorization", &bearer)
                .send_json(serde_json::json!({"name":CHAT_FILES,"folder":{},"@microsoft.graph.conflictBehavior":"fail"})))?;
        } else if !response.status().is_success() {
            return Err(format!("Cannot access Teams Chat Files: {}", response.status()));
        }
        let url = format!("{folder}/{}:/content?@name.conflictBehavior=rename&$select=*,sharepointIds,webDavUrl", encode(&upload_name(&draft.name)?));
        let item = answer(agent().put(&url).header("Authorization", &bearer).header("Content-Type", "application/octet-stream")
            .header("application", "Teams_Web").header("scenario", "ShareUploadFile").send(file))?;
        draft.uploaded = Some(file_card(&item, site)?);
    }
    let card = draft.uploaded.as_mut().unwrap();
    if !recipients.is_empty() {
        let id = card["id"].as_str().ok_or("Uploaded file has no ID.")?;
        let url = format!("{site}/_api/v2.0/sites/root/items/{}/driveItem/invite", encode(id));
        let granted = answer(agent().post(&url).header("Authorization", &bearer).header("Prefer", "ReturnSpecificErrorCode,getDefaultLink")
            .send_json(serde_json::json!({"recipients":recipients.iter().map(|id| serde_json::json!({"objectId":id})).collect::<Vec<_>>(),
                "roles":["read"],"sendInvitation":false,"requireSignIn":true})))?;
        let permissions = granted["value"].as_array().filter(|v| !v.is_empty()).ok_or("No file sharing permissions were granted.")?;
        if permissions.iter().any(|p| !p["error"].is_null()) { return Err("Could not grant file access to every chat member.".into()); }
        let permission = permissions.iter().find(|p| p["link"]["webUrl"].as_str().is_some()).ok_or("File sharing returned no link.")?;
        if permission["link"]["scope"].as_str().is_some_and(|scope| scope != "users") {
            return Err("File sharing returned a link broader than the chat members.".into());
        }
        card["fileInfo"]["shareUrl"] = permission["link"]["webUrl"].clone();
        card["fileInfo"]["shareId"] = permission["id"].clone();
    }
    card["permissionScope"] = "SpecificPeople".into();
    Ok(card.clone())
}

fn file_card(item: &Value, site: &str) -> Result<Value, String> {
    let required = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(String::from).ok_or("OneDrive returned incomplete file metadata.");
    let name = required(&item["name"])?;
    let url = required(&item["webUrl"])?;
    let id = required(&item["sharepointIds"]["listItemUniqueId"])?;
    let site_id = required(&item["sharepointIds"]["siteId"])?;
    let kind = crate::attachments::extension(&name);
    Ok(serde_json::json!({"@type":"http://schema.skype.com/File","version":2,"id":id,"itemid":id,
        "fileName":name,"title":name,"fileType":kind,"type":kind,"state":"active","objectUrl":url,"baseUrl":format!("{site}/"),
        "providerData":"","botFileProperties":{},"fileInfo":{"fileUrl":url,"siteUrl":format!("{site}/"),"serverRelativeUrl":"","shareUrl":"","shareId":""},
        "fileChicletState":{"serviceName":"p2p","state":"active"},"filePreview":{"previewUrl":"","previewHeight":0,"previewWidth":0},
        "sharepointIds":{"listItemUniqueId":id,"siteId":site_id}}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("teams-onedrive-{}.txt", uuid::Uuid::new_v4()));
            std::fs::write(&path, b"document bytes").unwrap();
            Self(path)
        }
        fn draft(&self) -> DraftFile {
            let mut draft = DraftFile::new(self.0.clone()).unwrap();
            draft.name = "report:final & plan.PDF".into();
            draft
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }

    /// Inspect requests at the HTTP boundary, independently of the app's file parser.
    fn server(steps: Vec<(&'static str, &'static str, u16, Value)>) -> (String, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut bodies = vec![];
            for (method, path, status, response) in steps {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut input = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                input.read_line(&mut line).unwrap();
                assert!(line.starts_with(&format!("{method} {path}")), "{line}");
                let mut length = 0;
                let mut bearer = false;
                loop {
                    line.clear();
                    input.read_line(&mut line).unwrap();
                    if line == "\r\n" { break; }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = v.trim().parse().unwrap(); }
                    bearer |= line.trim().eq_ignore_ascii_case("Authorization: Bearer test-token");
                }
                assert!(bearer);
                let mut body = vec![0; length];
                input.read_exact(&mut body).unwrap();
                bodies.push(body);
                let body = response.to_string();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            bodies
        });
        (url, worker)
    }

    fn item() -> Value {
        serde_json::json!({"name":"report_final & plan 1.PDF", "webUrl":"https://tenant-my.sharepoint.com/personal/me/Documents/report_final%201.PDF",
            "sharepointIds":{"listItemUniqueId":"11111111-1111-4111-8111-111111111111","siteId":"22222222-2222-4222-8222-222222222222"}})
    }

    #[test]
    fn uploads_a_real_drive_item_and_grants_only_chat_members_access() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        let (site, worker) = server(vec![
            ("GET", "/_api/v2.0/drive/root:/Microsoft%20Teams%20Chat%20Files ", 200, serde_json::json!({"folder":{}})),
            ("PUT", "/_api/v2.0/drive/root:/Microsoft%20Teams%20Chat%20Files/report_final%20%26%20plan.PDF:/content?@name.conflictBehavior=rename", 201, item()),
            ("POST", "/_api/v2.0/sites/root/items/11111111-1111-4111-8111-111111111111/driveItem/invite ", 200,
                serde_json::json!({"value":[{"id":"specific-people","link":{"webUrl":"https://tenant-my.sharepoint.com/:b:/g/abc","scope":"users"}}]})),
        ]);
        let card = upload_to(&site, "test-token", &["peer-id".into()], &mut draft, std::fs::File::open(&fixture.0).unwrap()).unwrap();
        let bodies = worker.join().unwrap();
        assert_eq!(bodies[1], b"document bytes");
        assert_eq!(serde_json::from_slice::<Value>(&bodies[2]).unwrap(), serde_json::json!({"recipients":[{"objectId":"peer-id"}],
            "roles":["read"],"sendInvitation":false,"requireSignIn":true}));
        assert_eq!(card["@type"], "http://schema.skype.com/File");
        assert_eq!(card["version"], 2);
        assert_eq!(card["fileName"], "report_final & plan 1.PDF");
        assert_eq!(card["fileInfo"]["shareId"], "specific-people");
        assert_eq!(card["permissionScope"], "SpecificPeople");
        assert_eq!(card["id"], card["sharepointIds"]["listItemUniqueId"]);
        let mut message = serde_json::json!({"content":"", "messagetype":"RichText/Html"});
        apply_files(&mut message, &[card]);
        assert_eq!(message["properties"]["formatVariant"], "TEAMS");
        let parsed = parse_messages(&serde_json::json!({"messages":[message]}), "me");
        assert!(parsed[0].files[0].url.starts_with("https://tenant-my.sharepoint.com/"));
    }

    #[test]
    fn failed_sharing_keeps_the_upload_and_retry_does_not_upload_again() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft();
        let (site, worker) = server(vec![
            ("GET", "/_api/v2.0/drive/root:", 404, serde_json::json!({})),
            ("POST", "/_api/v2.0/drive/root/children ", 201, serde_json::json!({"folder":{}})),
            ("PUT", "/_api/v2.0/drive/root:", 201, item()),
            ("POST", "/_api/v2.0/sites/root/items/", 403, serde_json::json!({"message":"sharing denied"})),
            ("POST", "/_api/v2.0/sites/root/items/", 200, serde_json::json!({"value":[{"id":"share", "link":{"webUrl":"https://tenant-my.sharepoint.com/:u:/abc","scope":"users"}}]})),
        ]);
        assert!(upload_to(&site, "test-token", &["peer-id".into()], &mut draft, std::fs::File::open(&fixture.0).unwrap()).is_err());
        assert!(draft.uploaded.is_some());
        assert!(draft.uploaded.as_ref().unwrap()["permissionScope"].is_null());
        let card = upload_to(&site, "test-token", &["peer-id".into()], &mut draft, std::fs::File::open(&fixture.0).unwrap()).unwrap();
        assert_eq!(card["permissionScope"], "SpecificPeople");
        let bodies = worker.join().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bodies[1]).unwrap()["@microsoft.graph.conflictBehavior"], "fail");
        assert_eq!(bodies[3], bodies[4]);
    }

    #[test]
    fn site_names_recipients_and_incomplete_metadata_are_checked() {
        assert_eq!(personal_site("https://tenant-my.sharepoint.com/personal/me_example_com/").unwrap().1, "tenant-my.sharepoint.com");
        for url in ["https://evil.example/personal/me", "https://tenant.sharepoint.com.evil.example/personal/me",
            "https://me@tenant.sharepoint.com/personal/me", "https://tenant.sharepoint.com/personal/me?redirect=evil",
            "https://tenant.sharepoint.com/personal/../elsewhere", "https://tenant.sharepoint.com/sites/general"] {
            assert!(personal_site(url).is_err(), "{url}");
        }
        assert_eq!(upload_name(" report:final?.pdf. ").unwrap(), "report_final_.pdf");
        assert!(upload_name("...").is_err());
        assert!(file_card(&serde_json::json!({"id":"ams-id","name":"report.pdf"}), "https://tenant.sharepoint.com/personal/me").is_err());
        let me = "11111111-1111-4111-8111-111111111111";
        let other = "22222222-2222-4222-8222-222222222222";
        assert_eq!(recipient_ids([format!("8:orgid:{me}"),format!("8:orgid:{other}"),format!("8:orgid:{other}")], me).unwrap(), vec![other]);
        assert!(recipient_ids(["8:live:external".into()], me).is_err());
    }

    #[test]
    fn incomplete_or_overbroad_sharing_never_marks_the_attachment_ready() {
        for granted in [serde_json::json!({"value":[]}),
            serde_json::json!({"value":[{"id":"partial","error":{"code":"accessDenied"}}]}),
            serde_json::json!({"value":[{"id":"everyone","link":{"webUrl":"https://tenant.sharepoint.com/share","scope":"anonymous"}}]})] {
            let fixture = Fixture::new();
            let mut draft = fixture.draft();
            draft.uploaded = Some(file_card(&item(), "https://tenant.sharepoint.com/personal/me").unwrap());
            let (site, worker) = server(vec![("POST", "/_api/v2.0/sites/root/items/", 200, granted)]);
            assert!(upload_to(&site, "test-token", &["peer".into()], &mut draft, std::fs::File::open(&fixture.0).unwrap()).is_err());
            assert!(draft.uploaded.as_ref().unwrap()["permissionScope"].is_null());
            worker.join().unwrap();
        }
    }

    #[test]
    #[ignore = "requires a signed-in Teams account and network; only reads OneDrive metadata, sends no messages"]
    fn live_onedrive_folder_access() {
        let _session = crate::auth::resume().unwrap().expect("Sign in first");
        let token = crate::auth::graph_access().unwrap();
        let profile = answer(agent().get("https://graph.microsoft.com/v1.0/me?$select=mySite")
            .header("Authorization", &format!("Bearer {token}")).call()).unwrap();
        let (site, host) = personal_site(profile["mySite"].as_str().unwrap()).unwrap();
        let token = crate::auth::sharepoint_access(&host).unwrap();
        let folder = answer(agent().get(&format!("{site}/_api/v2.0/drive/root:/{}", encode(CHAT_FILES)))
            .header("Authorization", &format!("Bearer {token}")).call()).unwrap();
        assert!(folder["folder"].is_object());
        assert_eq!(folder["name"], CHAT_FILES);
    }
}
