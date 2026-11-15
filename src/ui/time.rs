//! Timestamps as the chat list and messages show them.

use crate::*;

pub(crate) fn local_time(iso: &str, format: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|time| time.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_else(|_| iso.to_owned())
}

pub(crate) fn message_date(iso: &str, today: chrono::NaiveDate) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let date = time.with_timezone(&chrono::Local).date_naive();
    match (today - date).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=7 => date.format("%A").to_string(),
        _ => date.format("%A, %B %-d").to_string(),
    }
}

pub(crate) fn day(iso: &str) -> String {
    local_time(iso, "%Y-%m-%d")
}

pub(crate) fn chat_time(iso: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let time = time.with_timezone(&chrono::Local);
    let format = if time.date_naive() == now.date_naive() {
        "%-I:%M %p"
    } else {
        "%-m/%-d"
    };
    time.format(format).to_string()
}

pub(crate) fn message_time(iso: &str, today: chrono::NaiveDate) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let time = time.with_timezone(&chrono::Local);
    let date = if (0..=7).contains(&(today - time.date_naive()).num_days()) {
        message_date(iso, today)
    } else {
        time.format("%-m/%-d").to_string()
    };
    format!("{date} {}", time.format("%-I:%M %p"))
}

pub(crate) fn same_message_group(previous: &Msg, current: &Msg) -> bool {
    previous.sender == current.sender && previous.mine == current.mine
        && day(&previous.time) == day(&current.time)
        && chrono::DateTime::parse_from_rfc3339(&previous.time).ok()
            .zip(chrono::DateTime::parse_from_rfc3339(&current.time).ok())
            .is_some_and(|(before, after)| {
                let gap = after - before;
                gap >= chrono::Duration::zero() && gap < chrono::Duration::minutes(5)
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_times_show_time_only_on_the_same_local_day() {
        use chrono::{Local, TimeZone};
        let now = Local.with_ymd_and_hms(2026, 11, 15, 0, 47, 0).earliest().unwrap();
        for (age, expected) in [
            (chrono::Duration::zero(), "12:47 AM"),
            (chrono::Duration::minutes(47), "12:00 AM"),
            (chrono::Duration::minutes(47) + chrono::Duration::seconds(1), "11/14"),
            (chrono::Duration::hours(1), "11/14"),
            (chrono::Duration::hours(24) - chrono::Duration::seconds(1), "11/14"),
            (chrono::Duration::hours(24), "11/14"),
            (chrono::Duration::days(8), "11/7"),
        ] {
            let iso = (now - age).with_timezone(&chrono::Utc).to_rfc3339();
            assert_eq!(chat_time(&iso, now), expected);
        }
        assert_eq!(chat_time("", now), "");
        assert_eq!(chat_time("invalid", now), "invalid");
    }

    #[test]
    fn message_dates_use_relative_days_then_full_dates() {
        use chrono::{Local, TimeZone};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        for (days, expected) in [
            (0, "Today"), (1, "Yesterday"), (2, "Friday"),
            (6, "Monday"), (7, "Sunday"), (8, "Saturday, November 7"),
            (15, "Saturday, October 31"), (-1, "Monday, November 16"),
        ] {
            let date = today - chrono::Duration::days(days);
            let local = Local.from_local_datetime(&date.and_hms_opt(0, 5, 0).unwrap()).earliest().unwrap();
            assert_eq!(message_date(&local.with_timezone(&chrono::Utc).to_rfc3339(), today), expected);
        }
        assert_eq!(message_date("", today), "");
        assert_eq!(message_date("invalid", today), "invalid");
    }

    #[test]
    fn sender_times_use_relative_dates_then_numeric_dates() {
        use chrono::{Local, TimeZone};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        for (days, expected) in [
            (0, "Today 12:04 AM"), (1, "Yesterday 12:04 AM"),
            (2, "Friday 12:04 AM"), (7, "Sunday 12:04 AM"),
            (8, "11/7 12:04 AM"),
        ] {
            let date = today - chrono::Duration::days(days);
            let local = Local.from_local_datetime(&date.and_hms_opt(0, 4, 0).unwrap()).earliest().unwrap();
            assert_eq!(message_time(&local.with_timezone(&chrono::Utc).to_rfc3339(), today), expected);
        }
        assert_eq!(message_time("", today), "");
        assert_eq!(message_time("invalid", today), "invalid");
    }

    #[test]
    fn message_groups_require_same_sender_day_and_short_gap() {
        let mut s = init();
        demo(&mut s);
        let original = &s.messages.as_ref().unwrap().as_ref().unwrap().msgs[0];
        let mut previous = original.clone();
        let mut current = original.clone();
        previous.time = "2026-11-14T10:00:00Z".into();
        current.time = "2026-11-14T10:04:59Z".into();
        assert!(same_message_group(&previous, &current));
        current.time = "2026-11-14T10:05:00Z".into();
        assert!(!same_message_group(&previous, &current));
        current.time = previous.time.clone();
        current.sender = "Someone else".into();
        assert!(!same_message_group(&previous, &current));
        current.sender = previous.sender.clone();
        current.mine = !previous.mine;
        assert!(!same_message_group(&previous, &current));
        current.mine = previous.mine;
        current.time = "invalid".into();
        assert!(!same_message_group(&previous, &current));
        current.time = "2026-11-14T09:59:59Z".into();
        assert!(!same_message_group(&previous, &current));
        use chrono::{Local, TimeZone};
        let midnight = Local.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).earliest().unwrap();
        previous.time = (midnight - chrono::Duration::seconds(1)).to_rfc3339();
        current.time = midnight.to_rfc3339();
        assert!(!same_message_group(&previous, &current));
    }

    #[test]
    fn timestamps_use_local_dates_and_twelve_hour_time() {
        use chrono::{Local, TimeZone};
        for (hour, label) in [(0, "12:05 AM"), (12, "12:05 PM"), (23, "11:05 PM")] {
            let local = Local.with_ymd_and_hms(2026, 1, 15, hour, 5, 0).earliest().unwrap();
            let iso = local.with_timezone(&chrono::Utc).to_rfc3339();
            assert_eq!(local_time(&iso, "%I:%M %p"), label);
            assert_eq!(day(&iso), "2026-01-15");
        }
        assert_eq!(local_time("", "%I:%M %p"), "");
        assert_eq!(local_time("invalid", "%I:%M %p"), "invalid");
    }
}
