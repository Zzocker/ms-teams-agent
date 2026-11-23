//! Demo data for Rune's headless scenes (screenshots, tests).

use crate::*;

pub(crate) fn demo(s: &mut State) {
    let chat = |id: &str, title: &str, preview: &str, time: &str| Chat { id: id.into(), channel: false, title: title.into(), avatar_id: String::new(), avatar_members: Vec::new(), preview: preview.into(), time: time.into(), unread: id == "b", latest: None };
    let msg = |sender: &str, text: &str, time: &str, mine| Msg { id: time.into(), server_id: time.into(), sender: sender.into(), sender_id: String::new(), text: text.into(), time: time.into(), mine, reply: None, thread_root: None, images: vec![], links: vec![], files: vec![], raw: String::new(), edited: false, deleted: false, reactions: vec![], mentions: vec![] };
    s.screen = Screen::Main {
        session: Arc::new(Session {
            user: auth::User { name: "Ada Lovelace".into(), email: "ada@example.com".into(), id: "me".into() },
            ..Session::default()
        }),
        chats: vec![
            chat("a", "Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z"),
            Chat { avatar_members: ["Charles Babbage", "Ada Lovelace", "Mary Somerville"].into_iter().map(|name| auth::User { name: name.into(), ..Default::default() }).collect(), ..chat("b", "Design review", "Moved to Thursday", "2026-11-14T09:02:00Z") },
            chat("c", "Mary Somerville", "Thanks!", "2026-11-12T17:45:00Z"),
        ],
    };
    s.selected = Some("a".into());
    s.messages = Some(Ok(Page { older: None, msgs: vec![
        msg("Charles Babbage", "Did you get a chance to look at the notes?", "2026-11-14T10:20:00Z", false),
        msg("Ada Lovelace", "Yes. Note G is done: it computes the Bernoulli numbers.\nI'll send it tonight.", "2026-11-14T10:24:00Z", true),
        msg("Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z", false),
        msg("Charles Babbage", "And a faster carry.", "2026-11-14T10:32:00Z", false),
    ] }));
}

fn insights_demo(s: &mut State) {
    use chrono::{Local, NaiveDate, TimeZone};

    demo(s);
    s.conversation_section = ConversationSection::Analytics;
    if let Screen::Main { chats, .. } = &mut s.screen {
        chats[1].avatar_members.retain(|member| member.name != "Ada Lovelace");
        chats.push(Chat {
            id: "d".into(), title: "Release planning".into(), unread: false,
            ..chats[1].clone()
        });
        chats.swap(2, 3);
    }
    let today = NaiveDate::from_ymd_opt(2026, 11, 14).unwrap();
    let events = [("a", 54, 19, 10), ("b", 121, 51, 14), ("c", 36, 11, 9), ("d", 82, 28, 16)]
        .into_iter().flat_map(|(id, total, sent, hour)| (0..total).map(move |i| {
            let date = today - chrono::Days::new((i % 7) as u64);
            let time = Local.from_local_datetime(&date.and_hms_opt(hour + i % 2, i % 60, 0).unwrap())
                .earliest().unwrap();
            (id, time, i < sent)
        }));
    crate::insights::demo(s, today, events);
}

#[cfg(feature = "image")]
pub(crate) const IMAGE_DEMO: &str = "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-demo/views/imgo";

#[cfg(feature = "image")]
pub(crate) fn image_demo(s: &mut State) {
    demo(s);
    // Small enough (under 64 x 64) that rune decodes it on this thread, ready for the snapshot.
    let (w, h) = (48u32, 32u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, 180, 255]).collect();
    let image = rune::ImageCache::new(Arc::new(|| {})).rgba(w, h, px);
    s.images.insert(IMAGE_DEMO.into(), Some(image));
    if let Some(Ok(page)) = &mut s.messages {
        page.msgs[2].images = vec![IMAGE_DEMO.into(), "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-loading/views/imgo".into()];
    }
}

/// A Loop page with a preview of its first page, and a Word file that isn't shared with us.
#[cfg(feature = "image")]
pub(crate) fn file_demo(s: &mut State) {
    demo(s);
    let file = |name: &str, kind: &str, url: &str| teams::File { name: name.into(), kind: kind.into(), url: url.into(), preview: teams::sharepoint_thumbnail(url) };
    let page = file("Engine Notes.loop", "loop", "https://contoso.sharepoint.com/:fl:/g/notes");
    let doc = file("Analytical Engine SLA.docx", "docx", "https://contoso-my.sharepoint.com/p/SLA.docx");
    // A page of grey text lines on white, small enough that rune decodes it on this thread.
    let w = 60u32;
    let px: Vec<u8> = (0..w * w).flat_map(|i| { let (x, y) = (i % w, i / w); if y % 6 == 3 && x > 4 && x < 30 + y % 25 { [90, 90, 90, 255] } else { [255; 4] } }).collect();
    s.images.insert(page.preview.clone().unwrap(), Some(rune::ImageCache::new(Arc::new(|| {})).rgba(w, w, px)));
    s.images.insert(doc.preview.clone().unwrap(), None);
    if let Some(Ok(p)) = &mut s.messages {
        p.msgs[0].files = vec![page];
        p.msgs[2].files = vec![doc];
    }
}

pub(crate) const SCENES: &[(&str, fn(&mut State))] = &[
    ("login", |s| s.screen = Screen::Login(None)),
    ("error", |s| s.screen = Screen::Login(Some("chat service 403: Forbidden".into()))),
    ("code", |s| s.screen = Screen::Code { user_code: "ABCD1234".into(), url: "https://microsoft.com/devicelogin".into() }),
    ("main", demo),
    ("analytics", insights_demo),
    ("mentions", mention_demo),
    ("conversation-empty", |s| { demo(s); s.selected = None; s.messages = None; }),
    ("conversation-error", |s| { demo(s); s.messages = Some(Err("Check your connection and try again.".into())); }),
    ("conversation-loading", |s| { demo(s); s.messages = None; }),
    ("messages-empty", |s| { demo(s); s.messages = Some(Ok(Page { msgs: vec![], older: None })); }),
    ("message-states", message_states_demo),
    ("message-deleted", |s| { message_states_demo(s); if let Some(Ok(page)) = &mut s.messages { message_state::tombstone(&mut page.msgs[1]); } }),
    ("message-restored", |s| {
        message_states_demo(s);
        if let Some(Ok(page)) = &mut s.messages { page.msgs[1].edited = false; }
    }),
    ("message-sending", |s| { message_states_demo(s); if let Some(Ok(page)) = &mut s.messages { page.msgs[1].server_id.clear(); page.msgs[1].edited = false; } }),
    ("message-restore-error", |s| {
        message_states_demo(s);
        if let Some(Ok(page)) = &mut s.messages { message_state::tombstone(&mut page.msgs[1]); }
        s.send_error = Some("Could not restore message: chat service 403 Forbidden".into());
    }),

    ("updates", |s| { demo(s); s.settings_panel = true; s.settings_updates = true; }),
    ("updates-current", |s| {
        demo(s); s.settings_panel = true; s.settings_updates = true;
        s.release_status = releases::Status::Current;
        s.release_checked_at = Some(chrono::Local::now());
    }),
    ("updates-available", |s| {
        demo(s); s.settings_panel = true; s.settings_updates = true;
        s.release_status = releases::Status::Available(releases::Release {
            version: semver::Version::new(0, 1, 1),
            url: "https://github.com/Zzocker/ms-teams-agent/releases/tag/v0.1.1".into(),
        });
        s.release_checked_at = Some(chrono::Local::now());
    }),
    ("updates-error", |s| { demo(s); s.settings_panel = true; s.settings_updates = true; s.release_status = releases::Status::Failed; }),
    ("reply", |s| {
        demo(s);
        if let Some(Ok(page)) = &mut s.messages {
            let original = page.msgs[0].clone();
            page.msgs[1].reply = Some(teams::Reply {
                id: original.server_id, sender: original.sender, sender_id: original.sender_id, time: original.time, text: original.text,
            });
            // A short answer: the quote must still widen the bubble.
            page.msgs[1].text = "Well done".into();
        }
    }),
    ("copy-link", |s| {
        demo(s);
        if let Some(Ok(page)) = &mut s.messages {
            page.msgs.truncate(1);
            let (text, links) = teams::html_to_rich(r#"<p>Test data: café 🧪<br>Spec: <a href="https://example.com/specification?version=14&amp;mode=test">https://example.com/spec...</a><br>Notes: <a href="https://example.org/notes/full-document">https://example.org/notes…</a><br>Named link: <a href="https://example.net/guide">Read the guide</a><br>Literal dots: wait...</p>"#);
            (page.msgs[0].text, page.msgs[0].links) = (text, links);
            s.menu = Some(page.msgs[0].id.clone());
        }
    }),
    ("link", |s| {
        demo(s);
        if let Some(Ok(page)) = &mut s.messages {
            let (text, links) = teams::html_to_rich(r#"<p>Spec is up: <a href="https://example.com/verify?Context=650251703871&amp;X-Amz-Credential=AKIAZH53PVUYPP7M4GOK%2F20261109%2Fca-central-1%2Fses%2Faws4_request&amp;X-Amz-Signature=49758107ee3a9fab9dfe9f0ad5179b6e076d0736e4b6d9dede6f027c4f60e5c4">https://example.com/verify?Context=650251703871&amp;X-Amz-Credential=AKIAZH53PVUYPP7M4GOK%2F20261109%2Fca-central-1%2Fses%2Faws4_request&amp;X-Amz-Signature=49758107ee3a9fab9dfe9f0ad5179b6e076d0736e4b6d9dede6f027c4f60e5c4</a></p>"#);
            (page.msgs[0].text, page.msgs[0].links) = (text, links);
        }
    }),
    #[cfg(feature = "markdown")]
    ("markdown", |s| {
        demo(s);
        if let Some(Ok(page)) = &mut s.messages {
            page.msgs[0].text = teams::html_to_rich(r#"<pre class="language-markdown"><code>**Release&nbsp;checklist**<br><br>-&nbsp;Bump&nbsp;`version`&nbsp;in&nbsp;**Cargo.toml**<br>&nbsp;&nbsp;-&nbsp;then&nbsp;run&nbsp;*all*&nbsp;tests<br><br>```<br>cargo&nbsp;test&nbsp;--all-features<br>```<br><br>#&nbsp;Done</code></pre>"#).0;
        }
    }),
    ("edit", |s| {
        demo(s);
        if let Some(Ok(page)) = &s.messages && let Some(m) = page.msgs.iter().rev().find(|m| m.mine).cloned() {
            s.draft = TextEdit::new(&format!("{} (edited)", m.text));
            s.editing = Some(m);
        }
    }),
    ("menu", |s| {
        demo(s);
        s.menu = s.messages.as_ref().and_then(|r| r.as_ref().ok()).and_then(|p| p.msgs.iter().rev().find(|m| m.mine)).map(|m| m.id.clone());
    }),
    ("editor", |s| {
        demo(s);
        s.editor = true;
        s.draft = TextEdit::new("this is in plain text\n```markdown\n# Here Markdown Starts\n- with **bold** and `code`\n```\n```sh\ncargo test\n```");
    }),
    #[cfg(feature = "image")]
    ("file", file_demo),
    #[cfg(feature = "image")]
    ("image", image_demo),
    #[cfg(feature = "image")]
    ("viewer", |s| {
        image_demo(s);
        s.viewer = Some(IMAGE_DEMO.into());
    }),
    ("workspace", workspace_demo),
    ("channel-posts", channel_posts_demo),
    ("teams", |s| {
        workspace_demo(s);
        s.conversation_section = ConversationSection::Teams;
        s.selected = None;
        s.messages = None;
    }),
    ("loading", |s| {
        demo(s);
        s.messages = None;
    }),
    ("search", |s| {
        demo(s);
        s.search = TextEdit::new("char");
    }),
    ("search-preview", |s| {
        demo(s);
        s.search = TextEdit::new("gears");
        message_search::preview_changed(s, &mut Tx::default());
    }),
    ("message-search", |s| {
        demo(s);
        s.search = TextEdit::new("engine");
        message_search::open_global(s, &mut Tx::default());
    }),
    ("pick", |s| {
        demo(s);
        (s.selected, s.messages) = (None, None);
    }),
];

/// Chats, teams and a saved favorite for local previews.
pub(crate) fn workspace_demo(s: &mut State) {
    demo(s);
    s.favorites.entry("me".into()).or_default().insert("a".into());
    let channel = |id: &str, title: &str| Chat {
        channel: true,
        avatar_id: String::new(), avatar_members: Vec::new(),
        id: id.into(), title: title.into(), preview: String::new(), time: String::new(), unread: false, latest: None,
    };
    s.teams = Some(Ok(vec![
        teams::Team { id: "development".into(), title: "Development".into(), channels: vec![
            channel("backend", "Backend"), channel("mobile", "Mobile application"), channel("notifications", "Notifications"),
        ] },
        teams::Team { id: "trential".into(), title: "Trential".into(), channels: vec![
            channel("general", "General"), channel("announcements", "Announcements"), channel("random", "Random"),
        ] },
    ]));
    s.collapsed_teams.insert("trential".into());
}

/// Sample channel posts, including an outgoing post and a message with a quote.
pub(crate) fn channel_posts_demo(s: &mut State) {
    workspace_demo(s);
    s.conversation_section = ConversationSection::Teams;
    s.selected = Some("backend".into());
    s.messages = Some(Ok(Page {
        older: None,
        msgs: teams::parse_messages(&serde_json::json!({"messages": [
            {"id":"100", "clientmessageid":"channel-opening", "from":"8:orgid:charles", "imdisplayname":"Charles Babbage",
             "composetime":"2026-11-20T08:40:00Z", "messagetype":"RichText/Html",
             "content":"<p>The staging rollout is scheduled for Thursday. Please flag any migrations that need a second review.</p>"},
            {"id":"101", "rootMessageId":"101", "clientmessageid":"channel-update", "from":"8:orgid:me", "imdisplayname":"Ada Lovelace",
             "composetime":"2026-11-20T09:05:00Z", "messagetype":"RichText/Html",
             "content":"<p>The schema and rollback checks passed. I’ll post the final checklist after the review.</p>"},
            {"id":"102", "rootMessageId":"101", "clientmessageid":"channel-review", "from":"8:orgid:mary", "imdisplayname":"Mary Somerville",
             "composetime":"2026-11-20T09:21:00Z", "messagetype":"RichText/Html",
             "content":"<p>I can review the checklist this afternoon. Could you share it here once it’s ready?</p>"},
            {"id":"103", "clientmessageid":"channel-confirm", "from":"8:orgid:charles", "imdisplayname":"Charles Babbage",
             "composetime":"2026-11-20T09:31:00Z", "messagetype":"RichText/Html",
             "content":"<p>I’ll share the checklist here after the review.</p>"}
        ]}), "me"),
    }));
    if let Some(Ok(page)) = &mut s.messages {
        let quoted = &page.msgs[2];
        page.msgs[3].reply = Some(teams::Reply {
            id: quoted.server_id.clone(), sender: quoted.sender.clone(), sender_id: quoted.sender_id.clone(),
            time: quoted.time.clone(), text: quoted.text.clone(),
        });
    }
}

/// Synthetic service-shaped data; no account, credentials, or network.
pub(crate) fn message_states_demo(s: &mut State) {
    demo(s);
    s.messages = Some(Ok(Page { older: None, msgs: teams::parse_messages(&serde_json::json!({"messages":[
        {"id":"100", "clientmessageid":"sample-incoming", "from":"8:orgid:charles", "imdisplayname":"Charles Babbage", "composetime":"2026-11-17T10:20:00Z", "messagetype":"Text", "content":"The design review is now at 3 PM.", "properties":{"edittime":"1790000000000"}},
        {"id":"101", "clientmessageid":"sample-outgoing", "from":"8:orgid:me", "imdisplayname":"Ada Lovelace", "composetime":"2026-11-17T10:24:00Z", "messagetype":"Text", "content":"Thanks, I updated the notes. See you there!", "properties":{"edittime":"1790000001000"}}
    ]}), "me") }));
    s.message_state.receipt_chat = Some("a".into());
    s.message_state.seen.insert("charles".into(), 101);
    s.message_state.typing("a".into(), "charles".into(), "Charles Babbage".into(), true);
}

/// Service-shaped mentions alongside a plain name, Unicode, formatting and a web link.
pub(crate) fn mention_demo(s: &mut State) {
    demo(s);
    s.messages = Some(Ok(Page { older: None, msgs: teams::parse_messages(&serde_json::json!({"messages":[
        {"id":"100", "from":"8:orgid:charles", "imdisplayname":"Charles Babbage", "composetime":"2026-11-17T10:20:00Z", "messagetype":"RichText/Html",
         "content": "<p>**Review** — 你好 <span itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">Ada Lovelace</span> and <span itemtype=\"http://schema.skype.com/Mention\" itemid=\"1\">Mary Somerville</span>, please check the <a href=\"https://example.com/notes\">engine notes</a>.<br>Ada Lovelace here is ordinary text.</p>",
         "properties":{"mentions":serde_json::json!([
             {"itemid":"0", "mri":"8:orgid:11111111-2222-3333-4444-555555555555"},
             {"itemid":"1", "mri":"8:orgid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}
         ]).to_string()}}
    ]}), "me") }));
}
