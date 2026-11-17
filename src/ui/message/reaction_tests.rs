use super::*;
use std::io::{BufRead, Read, Write};

#[test]
fn reaction_buttons_send_add_remove_and_preserve_counts_on_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (i, stream) in listener.incoming().take(4).enumerate() {
            let mut stream = stream.unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request = line.trim().to_owned();
            let mut length = 0;
            loop {
                line.clear(); reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                if let Some((key, value)) = line.split_once(':')
                    && key.eq_ignore_ascii_case("content-length") { length = value.trim().parse::<usize>().unwrap(); }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            requests.push((request, serde_json::from_slice::<serde_json::Value>(&body).unwrap()));
            let response = if i == 3 { "HTTP/1.1 403 Forbidden\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"message\":\"denied\"}" }
                else { "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n" };
            stream.write_all(response.as_bytes()).unwrap();
        }
        requests
    });
    let mut s = init();
    demo(&mut s);
    if let Screen::Main { session, .. } = &mut s.screen { Arc::make_mut(session).chat_service = format!("http://{address}"); }
    let fixture = serde_json::json!({"messages":[{"id":"123","messagetype":"Text","content":"Test data: reaction check","from":"8:orgid:other",
        "imdisplayname":"Charles Babbage","composetime":"2026-11-14T10:20:00Z",
        "properties":{"emotions":[{"key":"like","users":[{"mri":"8:orgid:other"}]}]}}]});
    let page = Page { msgs: teams::parse_messages(&fixture, "me"), older: None };
    s.history.insert("a".into(), page.clone());
    s.messages = Some(Ok(page));
    s.waker = Some(Arc::new(|| {}));
    let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
    let click = |rt: &mut rune::Runtime<State>, key: &str| {
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    };
    let finish = |rt: &mut rune::Runtime<State>| {
        let start = std::time::Instant::now();
        loop {
            rt.update(|s, tx| { poll(s, 0.0, tx); }); rt.frame(0.0);
            if !rt.state.reactor.is_pending() { break; }
            assert!(start.elapsed().as_secs() < 5, "reaction request timed out");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        rt.update(|s, tx| { poll(s, 0.0, tx); }); rt.frame(0.0);
    };
    click(&mut rt, "reaction/123/like"); finish(&mut rt);
    assert_eq!(rt.state.messages.as_ref().unwrap().as_ref().unwrap().msgs[0].reactions[0].users.len(), 2);
    rt.update(|s, _| s.menu = Some("123".into()));
    click(&mut rt, "react/heart"); finish(&mut rt);
    click(&mut rt, "reaction/123/like"); finish(&mut rt);
    let reactions = &rt.state.messages.as_ref().unwrap().as_ref().unwrap().msgs[0].reactions;
    assert_eq!(reactions[0].users.len(), 1);
    assert!(!reactions[0].mine("me"));
    assert!(reactions[1].mine("me"));
    assert_eq!(rt.state.history.page_mut("a").unwrap().msgs[0].reactions, *reactions);
    rt.update(|s, _| s.menu = Some("123".into()));
    click(&mut rt, "react/sad"); finish(&mut rt);
    assert!(rt.state.send_error.as_ref().unwrap().contains("Reaction not changed"));
    assert_eq!(rt.state.messages.as_ref().unwrap().as_ref().unwrap().msgs[0].reactions.len(), 2);
    let requests = server.join().unwrap();
    for (i, (request, body)) in requests.iter().enumerate() {
        assert_eq!(request, &format!("{} /v1/users/ME/conversations/a/messages/123/properties?name=emotions HTTP/1.1", if i == 2 { "DELETE" } else { "PUT" }));
        assert_eq!(body["emotions"]["key"], ["like", "heart", "like", "sad"][i]);
        assert_eq!(body["emotions"]["value"].is_number(), i != 2);
    }
    if let Ok(path) = std::env::var("REACTION_EVIDENCE") {
        rt.state.send_error = None;
        rt.state.menu = None;
        rt.state.waker = None;
        rune::App::new(AppSpec::new("Reaction regression", (1200.0, 800.0), init, view))
            .init(move || rt.state).args(["--snap".into(), path, "--scale".into(), "1".into()]).run();
    }
}
