//! Pictures and files in messages, and the full-window image viewer.

use crate::*;

/// Queue only media near the viewport; layout reports don't rebuild the view on every scroll frame.
#[cfg(feature = "image")]
pub(crate) fn nearby_previews(s: &State) -> impl Iterator<Item = String> + '_ {
    let [vx, vy, vw, vh] = s.preview_viewport.get();
    s.messages.as_ref().and_then(|r| r.as_ref().ok()).into_iter()
        .flat_map(|p| &p.msgs)
        .filter(move |m| s.preview_rows.get(&m.id).is_some_and(|&[x, y, w, h]| {
            vw > 0.0 && vh > 0.0 && w > 0.0 && h > 0.0 && x < vx + vw && x + w > vx
                && y < vy + vh + 200.0 && y + h > vy - 200.0
        }))
        .flat_map(|m| m.images.iter().cloned().chain(m.files.iter().filter_map(|f| f.preview.clone()))
            .chain(teams::animated_emojis(&m.raw).into_iter().map(|(url, _, _)| url)))
}

#[cfg(feature = "image")]
pub(crate) fn message_emoji(s: &State, url: &str, alt: &str, size: u16) -> El {
    let tile = el().size(f32::from(size), f32::from(size)).role("img").label(alt);
    match s.images.get(url) {
        Some(Some(image)) => tile.pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        Some(None) => tile.center().child(text(alt).font(tok::TextStyle { size: f32::from(size) * 0.8, ..type_::BODY })),
        None => tile.radius(8.0).fill(tok::theme(dark()).hover).help("Loading emoji"),
    }
}

/// A picture's tile in a message, as Teams lays them out side by side.
#[cfg(feature = "image")]
pub(crate) const IMAGE_TILE: (f32, f32) = (256.0, 198.0);

#[cfg(feature = "image")]
pub(crate) fn message_image(s: &State, url: &str) -> El {
    let t = tok::theme(dark());
    let open = url.to_owned();
    let tile = el().size(IMAGE_TILE.0, IMAGE_TILE.1).radius(6.0).clip_rounded(6.0).fill(t.bg).border(1.0, t.separator)
        .role("button").focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
        .on_press(move |s: &mut State| s.viewer = Some(open.clone()));
    match s.images.get(url) {
        // Letterboxed, so a tall phone screenshot and a wide desktop one share the tile size.
        Some(Some(image)) => tile.pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Open image"),
        Some(None) => tile.center().label("Image unavailable").child(text("Image unavailable").font(type_::LABEL).color(t.text_secondary)),
        None => tile.label("Loading image"),
    }
}

/// The open picture filling the window, as Teams shows it, with the chat's other pictures a click or arrow key away.
#[cfg(feature = "image")]
pub(crate) fn image_viewer(s: &State) -> Option<El> {
    static BACK: rune::Icon = rune::Icon { name: "viewer-back", view: 24.0, body: r#"<path d="M20 12H5 M11 6l-6 6 6 6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static PREV: rune::Icon = rune::Icon { name: "viewer-prev", view: 24.0, body: r#"<path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static NEXT: rune::Icon = rune::Icon { name: "viewer-next", view: 24.0, body: r#"<path d="M9 5l7 7-7 7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    let open = s.viewer.as_deref()?;
    let page = s.messages.as_ref()?.as_ref().ok()?;
    let all: Vec<(&Msg, &String)> = page.msgs.iter().flat_map(|m| m.images.iter().map(move |u| (m, u))).collect();
    let i = all.iter().position(|(_, u)| *u == open)?;
    let msg = all[i].0;
    let prev = i.checked_sub(1).map(|j| all[j].1.clone());
    let next = all.get(i + 1).map(|(_, u)| (*u).clone());
    let t = tok::theme(dark());
    let icon_button = |icon: &'static rune::Icon, label: &'static str, target: Option<String>| {
        el().size(40.0, 40.0).center().radius(20.0).role("button").label(label).focusable(true)
            .cursor(rune::Cursor::Pointer).hover(t.hover)
            .on_press(move |s: &mut State| s.viewer = target.clone())
            .child(rune::icon(icon, 24.0).color(t.text))
    };
    let arrow = |icon, label, target: Option<String>| match target {
        Some(_) => icon_button(icon, label, target),
        None => el().size(40.0, 40.0),
    };
    let full = images::full(open);
    // The preview stands in until the full size arrives, or if it can't.
    let picture = match s.images.get(&full).flatten().or(s.images.get(open).flatten()) {
        Some(image) => el().grow(1.0).shrink().min_w(0.0).h_full().pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Image"),
        None if s.images.get(&full).is_some() => el().grow(1.0).h_full().center()
            .child(text("Image unavailable").font(type_::BODY).color(t.text_secondary)),
        None => el().grow(1.0).h_full().center().child(loading_messages()),
    };
    let (back, keys) = (prev.clone(), next.clone());
    Some(col().key("image-viewer").w_full().h_full().fill(t.bg).focusable(true).autofocus().label("Image viewer")
        .modal(false, |s: &mut State| s.viewer = None)
        .on_key(move |s: &mut State, e: &rune::KeyEvent, _| {
            s.viewer = match e.key {
                rune::Key::Left if back.is_some() => back.clone(),
                rune::Key::Right if keys.is_some() => keys.clone(),
                _ => return false,
            };
            true
        })
        .child(row().w_full().items_center().gap(12.0).px(12.0).py(8.0).fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(icon_button(&BACK, "Close image", None))
            .child(col().gap(2.0)
                .child(text(msg.sender.as_str()).font(type_::LABEL).color(t.text))
                .child(text(message_time(&msg.time, chrono::Local::now().date_naive())).font(type_::LABEL).color(t.text_secondary))))
        .child(row().grow(1.0).w_full().min_h(0.0).items_center().gap(12.0).padding(16.0)
            .child(arrow(&PREV, "Previous image", prev))
            .child(picture)
            .child(arrow(&NEXT, "Next image", next))))
}

#[cfg(not(feature = "image"))]
pub(crate) fn image_viewer(_: &State) -> Option<El> {
    None
}

#[cfg(not(feature = "image"))]
pub(crate) fn message_image(_: &State, _: &str) -> El {
    text("[image]").font(type_::BODY).color(tok::theme(dark()).text_secondary)
}

/// A shared file as Teams shows it: its name over a preview of its first page, or its icon when
/// there's none (not on SharePoint, or not shared with us). A click opens it in the browser.
pub(crate) fn file_card(s: &State, f: &teams::File) -> El {
    static DOC: rune::Icon = rune::Icon { name: "file-doc", view: 24.0, body: r#"<path d="M6 2.75h8l4.25 4.25v14.25H6z M14 2.75V7h4.25 M9 12h6 M9 15h6 M9 18h4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"# };
    const W: f32 = 360.0;
    const PREVIEW_H: f32 = 200.0;
    let t = tok::theme(dark());
    // Office's colours, so a Word file reads as Word at a glance.
    let tint = match f.kind.as_str() {
        "doc" | "docx" => rune::color::hex("#185abd").unwrap_or(t.text_secondary),
        "xls" | "xlsx" | "csv" => rune::color::hex("#107c41").unwrap_or(t.text_secondary),
        "ppt" | "pptx" => rune::color::hex("#c43e1c").unwrap_or(t.text_secondary),
        "pdf" => rune::color::hex("#d13438").unwrap_or(t.text_secondary),
        "loop" => rune::color::hex("#7b5ce6").unwrap_or(t.text_secondary),
        _ => t.text_secondary,
    };
    #[cfg(feature = "image")]
    let preview = f.preview.as_deref().map_or(Some(None), |u| s.images.get(u));
    #[cfg(not(feature = "image"))]
    let preview: Option<Option<&rune::Image>> = { let _ = s; Some(None) };
    let body = el().w_full().h(PREVIEW_H).clip_rounded(0.0).fill(t.bg);
    let body = match preview {
        // Full width from the top, as Teams crops a page's preview.
        Some(Some(image)) => {
            let [w, h] = image.size().unwrap_or([1, 1]);
            body.fill(rune::Color::WHITE).child(el().w(W).h(W * h as f32 / w.max(1) as f32).pixels_fit(image.pixels(), rune::SurfaceFit::Fit).a11y_hidden())
        }
        Some(None) => body.center().child(rune::icon(&DOC, 40.0).color(tint)),
        None => body,
    };
    let file = f.clone();
    col().w(W).radius(8.0).clip_rounded(8.0).fill(t.card).border(1.0, t.separator)
        .role("link").label(format!("Open {}", f.name)).help(f.url.as_str()).focusable(true)
        .cursor(rune::Cursor::Pointer).hover(t.hover)
        .on_press_tx(move |s: &mut State, tx| { if !file.url.is_empty() { crate::attachments::open_file(s, &file, tx); } })
        .child(row().w_full().items_center().gap(10.0).px(12.0).py(10.0).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(rune::icon(&DOC, 24.0).color(tint))
            .child(text(f.name.as_str()).font(type_::TITLE).color(t.text).truncate().min_w(0.0)))
        .child(body)
}

#[cfg(all(test, feature = "image"))]
mod tests {
    use super::*;

    #[test]
    fn animated_emojis_use_small_tiles_and_viewport_loading() {
        for (width, asset, size) in [(640.0, "100", 58.0), (1200.0, "100", 58.0), (640.0, "20", 58.0), (1200.0, "20", 58.0)] {
            let mut s = init();
            demo(&mut s);
            let emoji = r#"<img itemtype="http://schema.skype.com/Emoji" src="https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/muscle/default/100_f_s3.png" alt="💪🏼">"#;
            let emoji = emoji.replace("100_f", &format!("{asset}_f"));
            let raw = format!("<div>{emoji}{emoji}</div>");
            s.messages.as_mut().unwrap().as_mut().unwrap().msgs = teams::parse_messages(&serde_json::json!({"messages":[{
                "id":"101", "messagetype":"RichText/Html", "content":raw
            }]}), "me");
            let mut rt = rune::Runtime::new(s, view, (width, 800.0), rune::Env::new(true, 1.0));
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            let elements = rt.elements();
            let first = elements.iter().find(|e| e.key.as_deref() == Some("emoji/101/0")).unwrap();
            let second = elements.iter().find(|e| e.key.as_deref() == Some("emoji/101/1")).unwrap();
            assert_eq!(&first.rect[2..], &[size, size]);
            assert!(second.rect[0] >= first.rect[0] + size);
            assert_eq!(first.rect[1], second.rect[1]);
            assert!(second.rect[0] + size <= width);
            assert!(!elements.iter().any(|e| e.text.as_deref() == Some("💪🏼💪🏼")));
            assert!(!elements.iter().any(|e| e.text.as_deref() == Some("💪🏼")), "loading must not flash a static emoji");
            assert_eq!(nearby_previews(&rt.state).count(), 2);
            let rect = first.rect;
            let url = nearby_previews(&rt.state).next().unwrap();
            let image = rune::ImageCache::new(Arc::new(|| {})).rgba(4, 4, vec![255; 64]);
            rt.update(|s, _| s.images.insert(url.clone(), Some(image)));
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            let loaded = rt.elements();
            assert_eq!(loaded.iter().find(|e| e.key.as_deref() == Some("emoji/101/0")).unwrap().rect, rect);
            rt.update(|s, _| s.images.insert(url, None));
            rt.frame(0.0);
            assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("💪🏼") && e.rect[3] >= 40.0), "failed loads keep a large readable fallback");
            rt.state.preview_viewport.set([0.0, 2000.0, width, 800.0]);
            assert!(nearby_previews(&rt.state).next().is_none());
        }
    }

    #[test]
    fn preview_queue_follows_viewport_with_scroll_margin() {
        let mut s = init();
        demo(&mut s);
        let page = s.messages.as_mut().unwrap().as_mut().unwrap();
        for (i, m) in page.msgs.iter_mut().enumerate() {
            m.images = vec![format!("image{i}")];
        }
        page.msgs[1].files.push(teams::File {
            name: "Notes".into(), kind: "docx".into(), url: "file".into(), preview: Some("document".into()),
        });
        let ids: Vec<_> = page.msgs.iter().map(|m| m.id.clone()).collect();
        for (id, y) in ids.iter().zip([-500.0, -100.0, 500.0, 900.0]) {
            s.preview_rows.insert(id.clone(), [0.0, y, 300.0, 100.0]);
        }
        assert!(nearby_previews(&s).next().is_none(), "wait for viewport layout");
        s.preview_viewport.set([0.0, 100.0, 300.0, 400.0]);
        assert_eq!(nearby_previews(&s).collect::<Vec<_>>(), ["image1", "document", "image2"]);
        s.preview_viewport.set([0.0, 800.0, 300.0, 400.0]);
        assert_eq!(nearby_previews(&s).collect::<Vec<_>>(), ["image3"]);
        s.preview_rows.insert(ids[0].clone(), [0.0, -500.0, 300.0, 2000.0]);
        assert_eq!(nearby_previews(&s).collect::<Vec<_>>(), ["image0", "image3"], "tall rows still load");
        s.preview_rows.insert(ids[0].clone(), [500.0, 800.0, 300.0, 100.0]);
        assert_eq!(nearby_previews(&s).collect::<Vec<_>>(), ["image3"], "horizontally clipped rows stay unloaded");
        s.preview_rows.clear();
        assert!(nearby_previews(&s).next().is_none(), "old chat geometry cannot queue new previews");
    }

    #[test]
    fn drawn_media_rows_follow_scrolling() {
        fn view(s: &State, cx: &mut Cx<State>) -> El {
            let Screen::Main { session, chats } = &s.screen else { unreachable!() };
            main_view(s, session, chats, cx)
        }
        let mut s = init();
        demo(&mut s);
        let template = s.messages.as_ref().unwrap().as_ref().unwrap().msgs[0].clone();
        s.messages.as_mut().unwrap().as_mut().unwrap().msgs = (0..20).map(|i| {
            let mut m = template.clone();
            m.id = format!("row{i}");
            m.images = vec![format!("image{i}")];
            m
        }).collect();
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        for _ in 0..5 { rt.frame(1.0 / 60.0); }
        let initial = nearby_previews(&rt.state).collect::<Vec<_>>();
        assert!(!initial.is_empty() && initial.len() < 20, "only nearby rows: {initial:?}");
        rt.wheel_event(WheelEvent::new(0.0, 10000.0).at(950.0, 300.0));
        for _ in 0..180 { rt.frame(1.0 / 60.0); }
        let scrolled = nearby_previews(&rt.state).collect::<Vec<_>>();
        assert!(!scrolled.is_empty() && scrolled.len() < 20);
        assert_ne!(initial, scrolled, "scrolling changes the preview queue");
    }
}
