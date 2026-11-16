//! Session-local history and reading positions; foreground requests use their own worker.
use crate::*;
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct History(VecDeque<(String, Page, Option<ScrollPos>)>);

impl History {
    pub fn get(&mut self, id: &str) -> Option<Page> {
        let index = self.0.iter().position(|(key, ..)| key == id)?;
        let entry = self.0.remove(index)?;
        let page = entry.1.clone();
        self.0.push_back(entry);
        Some(page)
    }

    pub fn position(&self, id: &str) -> Option<ScrollPos> {
        self.0.iter().find(|(key, ..)| key == id).and_then(|(_, _, position)| *position)
    }

    pub fn remember(&mut self, id: String, page: Page, at: ScrollPos) {
        self.insert(id.clone(), page);
        let index = self.0.iter().position(|(key, ..)| key == &id).unwrap();
        let mut entry = self.0.remove(index).unwrap();
        entry.2 = Some(at);
        self.0.push_back(entry);
    }

    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page> {
        self.0.iter_mut().find(|(key, ..)| key == id).map(|(_, page, _)| page)
    }

    pub fn remove(&mut self, id: &str) {
        self.0.retain(|(key, ..)| key != id);
    }

    pub fn insert(&mut self, id: String, page: Page) {
        if let Some((_, cached, _)) = self.0.iter_mut().find(|(key, ..)| key == &id) {
            *cached = page;
            return;
        }
        self.0.push_back((id, page, None));
        if self.0.len() > 10 { self.0.pop_front(); }
    }
}

pub(crate) fn remember_selected(s: &mut State) {
    if let (Some(id), Some(Ok(page))) = (&s.selected, &s.messages) {
        if !cacheable(s, id) { return; }
        let at = s.restore_scroll.unwrap_or_else(|| s.msg_bar.position());
        s.history.remember(id.clone(), page.clone(), at);
    }
}

fn cacheable(s: &State, id: &str) -> bool {
    !notifications::conversation(s, id).is_some_and(|c| c.unread) || notifications::chat_open(s, id)
}

pub(crate) fn cache_page(s: &mut State, id: String, page: Page) {
    if cacheable(s, &id) { s.history.insert(id, page); }
    else { invalidate(s, &id); }
}

pub(crate) fn stop_prefetch(s: &mut State, id: &str) {
    if s.prefetch_id.as_deref() == Some(id) {
        s.prefetch.cancel();
        s.prefetch_id = None;
    }
}

pub(crate) fn invalidate(s: &mut State, id: &str) {
    s.history.remove(id);
    stop_prefetch(s, id);
    s.prefetch_queue.retain(|queued| queued != id);
}

pub(crate) fn poll(s: &mut State) {
    if let Some((id, result)) = s.prefetch.take() {
        s.prefetch_id = None;
        if let Ok(page) = result { cache_page(s, id, page); }
    }
    if s.waker.is_none() || s.prefetch.is_pending() || s.loader.is_pending() || s.sender.is_pending() { return; }
    let Screen::Main { session, .. } = &s.screen else { return };
    while let Some(id) = s.prefetch_queue.pop_front() {
        if !cacheable(s, &id) || s.selected.as_ref() == Some(&id) || s.history.page_mut(&id).is_some() { continue; }
        let session = session.clone();
        s.prefetch_id = Some(id.clone());
        s.prefetch.request(id, move |id| {
            let result = teams::messages(&session, &id);
            (id, result)
        });
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_evicts_least_recently_used_and_replaces_pages() {
        let mut cache = History::default();
        let page = || Page { msgs: vec![], older: Some("cursor".into()) };
        for i in 0..10 { cache.insert(i.to_string(), page()); }
        assert_eq!(cache.get("0").unwrap().older.as_deref(), Some("cursor"));
        cache.insert("1".into(), page()); // Background refresh is not a user access.
        cache.insert("10".into(), page());
        assert!(cache.get("1").is_none());
        assert!(cache.get("0").is_some());
        cache.insert("0".into(), Page { msgs: vec![], older: None });
        assert!(cache.get("0").unwrap().older.is_none());
        assert_eq!(cache.0.len(), 10);
    }

    #[test]
    fn refreshed_pages_keep_positions_and_eviction_discards_them() {
        let mut cache = History::default();
        let page = || Page { msgs: vec![], older: None };
        let mut at = ScrollPos::default();
        at.offset[1] = 300.0;
        at.max[1] = 900.0;
        cache.remember("a".into(), page(), at);
        cache.insert("a".into(), page());
        assert_eq!(cache.position("a").unwrap().offset[1], 300.0);
        for i in 0..10 { cache.insert(i.to_string(), page()); }
        assert!(cache.position("a").is_none());
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn invalidation_discards_cached_and_queued_history() {
        let mut s = init();
        s.history.insert("chat".into(), Page { msgs: vec![], older: None });
        s.prefetch_queue.push_back("chat".into());
        s.prefetch_id = Some("chat".into());
        invalidate(&mut s, "chat");
        assert!(s.history.get("chat").is_none());
        assert!(s.prefetch_id.is_none());
        assert!(s.prefetch_queue.is_empty());
    }
}
