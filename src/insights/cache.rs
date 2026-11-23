//! Persistent counts for completed local calendar days. Message data stays out of the cache.

use super::{Counts, Dates, DaysByChat};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde_json::{Map, Value, json};
use std::{fs, path::PathBuf, sync::Mutex};

pub(super) struct ChatCache {
    file: PathBuf,
    prefix: String,
    through: NaiveDate,
    history: CachedHistory,
}

#[derive(Default)]
struct CachedHistory {
    ranges: Vec<Dates>,
    days: DaysByChat,
}

impl ChatCache {
    pub(super) fn open(account: &str, chat: &str, today: NaiveDate) -> Option<Self> {
        if !crate::avatars::valid_id(account) {
            return None;
        }
        let dirs = rune::dirs::AppDirs::new("ms-teams-agent")?;
        let file = dirs
            .cache()
            .join("insights-counts-v1")
            .join(account)
            .join(format!("{}.json", URL_SAFE_NO_PAD.encode(chat)));
        Self::at(file, chat, today)
    }

    pub(super) fn at(file: PathBuf, chat: &str, today: NaiveDate) -> Option<Self> {
        let through = today.pred_opt()?;
        let prefix = format!("chat:{chat}:");
        let mut cache = Self {
            file,
            prefix,
            through,
            history: CachedHistory::default(),
        };
        cache.history = cache.read().unwrap_or_default();
        Some(cache)
    }

    pub(super) fn scan_from(&self, requested: NaiveDate) -> NaiveDate {
        self.history
            .ranges
            .iter()
            .find(|range| (range.from..=range.to).contains(&requested))
            .and_then(|range| range.to.succ_opt())
            .unwrap_or(requested)
    }

    pub(super) fn covers(&self, dates: Dates) -> bool {
        self.history
            .ranges
            .iter()
            .any(|range| range.from <= dates.from && range.to >= dates.to)
    }

    pub(super) fn merge_counts(&self, days: &mut DaysByChat) {
        days.retain(|date, _| !self.history.covers(*date));
        days.extend(&self.history.days);
    }

    pub(super) fn remember(&self, from: NaiveDate, days: &DaysByChat) {
        if from > self.through {
            return;
        }
        static WRITE: Mutex<()> = Mutex::new(());
        let Ok(_write) = WRITE.lock() else { return };
        let mut history = self.read().unwrap_or_default();
        let fresh: Vec<_> = days
            .iter()
            .filter(|(date, _)| (from..=self.through).contains(*date) && !history.covers(**date))
            .map(|(&date, &counts)| (date, counts))
            .collect();
        history.days.extend(fresh);
        history.include(Dates {
            from,
            to: self.through,
        });
        let Some(value) = self.encode(&history) else {
            return;
        };
        let Some(parent) = self.file.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let temporary = self
            .file
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        if fs::write(&temporary, value.to_string())
            .and_then(|()| fs::rename(&temporary, &self.file))
            .is_err()
        {
            let _ = fs::remove_file(temporary);
        }
    }

    fn read(&self) -> Option<CachedHistory> {
        let value: Value = serde_json::from_slice(&fs::read(&self.file).ok()?).ok()?;
        let mut history = CachedHistory::default();
        for span in value["coverage"].as_array()? {
            let from = if span["from"].is_null() {
                NaiveDate::MIN
            } else {
                span["from"].as_str()?.parse().ok()?
            };
            let to = span["to"].as_str()?.parse().ok()?;
            if from > to || to > self.through || coverage(Dates { from, to })? != *span {
                return None;
            }
            history.include(Dates { from, to });
        }
        for (key, value) in value["counts"].as_object()? {
            let stamps = key.strip_prefix(&self.prefix)?;
            let separator = stamps.get(1..)?.find('-')? + 1;
            let start = stamps[..separator].parse().ok()?;
            let date = DateTime::from_timestamp(start, 0)?
                .with_timezone(&Local)
                .date_naive();
            let sent = value["sent_total"].as_u64()?;
            let total = value["total"].as_u64()?;
            if !history.covers(date) || key != &self.key(date)? || sent > total {
                return None;
            }
            history.days.insert(date, Counts { sent, total });
        }
        Some(history)
    }

    fn encode(&self, history: &CachedHistory) -> Option<Value> {
        let ranges: Option<Vec<_>> = history.ranges.iter().copied().map(coverage).collect();
        let counts: Option<Map<String, Value>> = history
            .days
            .iter()
            .map(|(&date, count)| {
                Some((
                    self.key(date)?,
                    json!({"sent_total": count.sent, "total": count.total}),
                ))
            })
            .collect();
        Some(json!({"coverage": ranges?, "counts": counts?}))
    }

    fn key(&self, day: NaiveDate) -> Option<String> {
        let start = boundary(day)?;
        let end = boundary(day.succ_opt()?)?;
        Some(format!("{}{start}-{end}", self.prefix))
    }
}

impl CachedHistory {
    fn covers(&self, date: NaiveDate) -> bool {
        self.ranges
            .iter()
            .any(|range| (range.from..=range.to).contains(&date))
    }

    fn include(&mut self, range: Dates) {
        self.ranges.push(range);
        self.ranges.sort_unstable_by_key(|range| range.from);
        let mut merged: Vec<Dates> = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.to.succ_opt().is_some_and(|next| next >= range.from)
            {
                last.to = last.to.max(range.to);
            } else {
                merged.push(range);
            }
        }
        self.ranges = merged;
    }
}

fn boundary(day: NaiveDate) -> Option<i64> {
    Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|time| time.timestamp())
}

fn coverage(range: Dates) -> Option<Value> {
    let (from, start) = if range.from == NaiveDate::MIN {
        (None, None)
    } else {
        (Some(range.from.to_string()), Some(boundary(range.from)?))
    };
    Some(json!({
        "from": from, "to": range.to.to_string(),
        "start_unix": start, "end_unix": boundary(range.to.succ_opt()?)?
    }))
}
