static PROFILE_CLEARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// Appended only to the temporary profiling copy by profile.py.
#[derive(Default)]
struct ProfileDriver {
    started: Option<std::time::Instant>,
    sampled_second: Option<u64>,
    next_action: u64,
    step: usize,
    chats: Vec<String>,
    cleanup: u8,
}

fn profile_tick(s: &mut State, tx: &mut Tx) -> bool {
    use std::time::{Duration, Instant};
    thread_local! {
        static DRIVER: std::cell::RefCell<ProfileDriver> = Default::default();
    }
    tx.wake_after(Duration::from_millis(250));
    DRIVER.with(|driver| {
        let mut d = driver.borrow_mut();
        let elapsed = d.started.get_or_insert_with(Instant::now).elapsed();
        let second = elapsed.as_secs();
        if second > 330 {
            eprintln!("PROFILE_ERROR timed out waiting for sign-in or message requests");
            std::process::exit(2);
        }
        if d.sampled_second == Some(second) { return false; }
        d.sampled_second = Some(second);
        let page = s.messages.as_ref().and_then(|r| r.as_ref().ok());
        let text_bytes: usize = page.into_iter().flat_map(|p| &p.msgs)
            .map(|m| m.text.len() + m.raw.len()).sum();
        let read = |name| std::fs::read_to_string(format!("/proc/self/{name}"))
            .expect("could not read process profiling counters");
        let (previews, full_size, failed) = s.images.profile_counts();
        eprintln!("PROFILE {}", serde_json::json!({
            "elapsed": elapsed.as_secs_f64(), "step": d.step,
            "phase": match d.cleanup { 0 => "loaded", 1 => "images_cleared", 2 => "caches_cleared", _ => "trimmed" },
            "allocations": profile_allocations(),
            "messages": page.map_or(0, |p| p.msgs.len()),
            "message_text_bytes": text_bytes,
            "image_previews": previews, "image_full_size": full_size, "image_failed": failed,
            "image_bytes": s.images.profile_bytes(),
            "avatar_bytes": s.avatars.profile_bytes(),
            "smaps": read("smaps_rollup"), "stat": read("stat"), "io": read("io"),
        }));
        if let Some(Err(error)) = &s.messages {
            eprintln!("PROFILE_ERROR messages: {error}");
            std::process::exit(2);
        }
        if d.chats.is_empty() {
            match &s.screen {
                Screen::Main { chats, .. } => {
                    d.chats = chats.iter().filter(|c| !c.unread).take(6)
                        .map(|c| c.id.clone()).collect();
                    if d.chats.is_empty() {
                        eprintln!("PROFILE_ERROR no already-read chats available");
                        std::process::exit(2);
                    }
                    eprintln!("PROFILE_READY chats={}", d.chats.len());
                    // Leave one settled baseline sample before opening the first chat.
                    d.next_action = second + 3;
                }
                Screen::Login(_) | Screen::Code { .. } => {
                    eprintln!("PROFILE_ERROR sign in with the normal app first");
                    std::process::exit(2);
                }
                Screen::Loading => {}
            }
            return false;
        }
        if second < d.next_action || s.loader.is_pending() || s.older_loader.is_pending() {
            return false;
        }
        let actions = d.chats.len() * 4 * 2;
        if d.step >= actions {
            match d.cleanup {
                0 => {
                    PROFILE_CLEARED.store(true, SeqCst);
                    s.images = Default::default();
                    eprintln!("PROFILE_PHASE images_cleared");
                }
                1 => {
                    s.avatars = Default::default();
                    s.messages = None;
                    s.selected = None;
                    s.viewer = None;
                    s.loader = Default::default();
                    s.older_loader = Default::default();
                    eprintln!("PROFILE_PHASE caches_cleared");
                }
                2 => {
                    let released = unsafe { malloc_trim(0) };
                    eprintln!("PROFILE_PHASE trimmed released={released}");
                }
                _ => {
                    eprintln!("PROFILE_DONE");
                    std::process::exit(0);
                }
            }
            d.cleanup += 1;
            d.next_action = second + 30;
            return true;
        }
        let step = d.step;
        d.step += 1;
        d.next_action = second + if d.step == actions { 15 } else { 3 };
        if step % 4 == 0 {
            let index = (step / 4) % d.chats.len();
            eprintln!("PROFILE_ACTION open chat_index={} pass={}", index, step / (d.chats.len() * 4));
            open_chat(s, d.chats[index].clone());
        } else {
            eprintln!("PROFILE_ACTION scroll_top");
            tx.scroll_to_y("messages", 0.0);
            load_older(s);
        }
        true
    })
}
