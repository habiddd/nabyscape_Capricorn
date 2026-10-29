//! Local calendar events and date math (C# `CalendarService`, `CalendarEvent`,
//! `MinimalWidgets.Week` and the month grid behind the island calendar).
//!
//! The `calendar` document is a bare JSON array of events.

use super::links::{validate_url, MAX_MEETING_URL};
use super::new_id;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const DOCUMENT_NAME: &str = "calendar";
pub const MAX_EVENTS: usize = 200;
pub const MAX_TITLE: usize = 120;

/// Offsets are kept exactly as stored (`+07:00` stays `+07:00`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub meeting_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarError {
    /// User-facing validation message.
    Invalid(String),
    /// 200 events already stored.
    Full,
    /// The save callback failed; memory is unchanged.
    Save(String),
}

impl std::fmt::Display for CalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) | Self::Save(m) => f.write_str(m),
            Self::Full => f.write_str(
                "The calendar is full. Remove an event before adding another (200 maximum).",
            ),
        }
    }
}
impl std::error::Error for CalendarError {}

fn create(
    title: &str,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    meeting_url: &str,
    id: Option<&str>,
) -> Result<CalendarEvent, CalendarError> {
    let title = title.trim();
    let length = title.chars().count();
    if !(1..=MAX_TITLE).contains(&length) {
        return Err(CalendarError::Invalid(
            "Enter an event title between 1 and 120 characters.".into(),
        ));
    }
    if end <= start || end - start > Duration::days(31) {
        return Err(CalendarError::Invalid(
            "Choose an end after the start, within 31 days.".into(),
        ));
    }
    let meeting_url = meeting_url.trim();
    let meeting_url = if meeting_url.is_empty() {
        String::new()
    } else {
        validate_url(meeting_url, MAX_MEETING_URL).map_err(|e| CalendarError::Invalid(e.0))?
    };
    let id = match id {
        Some(id) if !id.trim().is_empty() => id.to_string(),
        _ => new_id(),
    };
    Ok(CalendarEvent {
        id,
        title: title.to_string(),
        start,
        end,
        meeting_url,
    })
}

#[derive(Debug, Clone, Default)]
pub struct Calendar {
    events: Vec<CalendarEvent>,
}

impl Calendar {
    /// Loads stored records: nulls, invalid entries and duplicate ids are excluded, titles are
    /// trimmed and cut to 120 characters, and at most 200 are kept. The source is never rewritten.
    pub fn load(stored: Vec<Option<CalendarEvent>>) -> Self {
        let mut events = Vec::new();
        let mut ids = HashSet::new();
        for item in stored.into_iter().flatten() {
            let title: String = item.title.trim().chars().take(MAX_TITLE).collect();
            if let Ok(entry) = create(
                &title,
                item.start,
                item.end,
                &item.meeting_url,
                Some(&item.id),
            ) {
                if ids.insert(entry.id.clone()) {
                    events.push(entry);
                }
                if events.len() == MAX_EVENTS {
                    break;
                }
            }
        }
        Self { events }
    }

    pub fn events(&self) -> &[CalendarEvent] {
        &self.events
    }

    /// Adds an event. `save` receives the would-be list; memory changes only if it succeeds.
    pub fn add_with<E: ToString>(
        &mut self,
        title: &str,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        meeting_url: &str,
        save: impl FnOnce(&[CalendarEvent]) -> Result<(), E>,
    ) -> Result<CalendarEvent, CalendarError> {
        if self.events.len() >= MAX_EVENTS {
            return Err(CalendarError::Full);
        }
        let entry = create(title, start, end, meeting_url, None)?;
        let mut next = self.events.clone();
        next.push(entry.clone());
        save(&next).map_err(|e| CalendarError::Save(e.to_string()))?;
        self.events = next;
        Ok(entry)
    }

    pub fn add(
        &mut self,
        title: &str,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        meeting_url: &str,
    ) -> Result<CalendarEvent, CalendarError> {
        self.add_with(title, start, end, meeting_url, |_| Ok::<(), String>(()))
    }

    /// Removes by id; `Ok(false)` when the id is unknown (nothing saved).
    pub fn remove_with<E: ToString>(
        &mut self,
        id: &str,
        save: impl FnOnce(&[CalendarEvent]) -> Result<(), E>,
    ) -> Result<bool, CalendarError> {
        let next: Vec<CalendarEvent> = self.events.iter().filter(|e| e.id != id).cloned().collect();
        if next.len() == self.events.len() {
            return Ok(false);
        }
        save(&next).map_err(|e| CalendarError::Save(e.to_string()))?;
        self.events = next;
        Ok(true)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.remove_with(id, |_| Ok::<(), String>(()))
            .unwrap_or(false)
    }

    /// Events whose local day (in their own offset) is `day`.
    pub fn on_day(&self, day: NaiveDate) -> Vec<&CalendarEvent> {
        self.events
            .iter()
            .filter(|e| e.start.date_naive() == day)
            .collect()
    }
}

fn min_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1, 1, 1).expect("valid date")
}
fn max_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(9999, 12, 31).expect("valid date")
}

/// Monday-first week containing `selected`, clamped to the supported date range.
pub fn week_of(selected: NaiveDate) -> [NaiveDate; 7] {
    let offset = i64::from(selected.weekday().num_days_from_monday());
    let latest_start = max_date() - Duration::days(6);
    let start = (selected - Duration::days(offset)).clamp(min_date(), latest_start);
    std::array::from_fn(|i| start + Duration::days(i as i64))
}

/// Six weeks of day numbers for a month; `first_day` is the leftmost weekday column.
/// Cells outside the month are `None`. Returns all `None` for an invalid month.
pub fn month_grid(year: i32, month: u32, first_day: Weekday) -> [[Option<u32>; 7]; 6] {
    let mut grid = [[None; 7]; 6];
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return grid;
    };
    let lead = (first.weekday().num_days_from_monday() + 7 - first_day.num_days_from_monday()) % 7;
    let days = days_in_month(year, month);
    for day in 1..=days {
        let cell = (lead + day - 1) as usize;
        grid[cell / 7][cell % 7] = Some(day);
    }
    grid
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    match (
        NaiveDate::from_ymd_opt(year, month, 1),
        NaiveDate::from_ymd_opt(ny, nm, 1),
    ) {
        (Some(a), Some(b)) => (b - a).num_days() as u32,
        (Some(a), None) => (max_date() - a).num_days() as u32 + 1,
        _ => 0,
    }
}

/// Previous/next month, `None` past year 1 or 9999 (the toolbar arrows disable there).
pub fn shift_month(year: i32, month: u32, forward: bool) -> Option<(i32, u32)> {
    let index = year * 12 + month as i32 - 1 + if forward { 1 } else { -1 };
    let (y, m) = (index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
    (1..=9999).contains(&y).then_some((y, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-10-14T12:00:00+07:00").unwrap()
    }
    fn h(n: i64) -> Duration {
        Duration::hours(n)
    }

    #[test]
    fn saves_reloads_preserves_offsets_and_removes() {
        let mut cal = Calendar::default();
        let item = cal
            .add(
                "  Planning  ",
                now(),
                now() + h(1),
                " https://example.com/?utm_source=preserved ",
            )
            .unwrap();
        assert_eq!(item.title, "Planning");
        let json = serde_json::to_string(cal.events()).unwrap();
        assert!(json.contains("+07:00") && json.contains("\"MeetingUrl\""));
        let stored: Vec<Option<CalendarEvent>> = serde_json::from_str(&json).unwrap();
        let mut restored = Calendar::load(stored);
        let saved = &restored.events()[0];
        assert!(
            saved.id == item.id && saved.start == now() && saved.start.offset() == now().offset()
        );
        assert_eq!(saved.end, now() + h(1));
        assert_eq!(
            saved.meeting_url,
            "https://example.com/?utm_source=preserved"
        );
        assert!(!restored.remove("missing"));
        assert!(restored.remove(&item.id));
        assert!(restored.events().is_empty());
    }

    #[test]
    fn validates_title_range_url_and_limit() {
        let mut cal = Calendar::default();
        for title in [" ".to_string(), "a".repeat(121)] {
            assert!(matches!(
                cal.add(&title, now(), now() + h(1), ""),
                Err(CalendarError::Invalid(_))
            ));
        }
        for end in [
            now(),
            now() - Duration::minutes(1),
            now() + Duration::days(31) + Duration::nanoseconds(1),
        ] {
            assert!(matches!(
                cal.add("a", now(), end, ""),
                Err(CalendarError::Invalid(_))
            ));
        }
        for url in [
            "relative",
            "file:///C:/test",
            "https://user@example.com",
            &format!("https://example.com/{}", "a".repeat(2048)),
        ] {
            assert!(matches!(
                cal.add("a", now(), now() + h(1), url),
                Err(CalendarError::Invalid(_))
            ));
        }
        assert!(cal.events().is_empty());
        cal.add(&"t".repeat(120), now(), now() + Duration::days(31), "")
            .unwrap();
        for i in 1..200 {
            cal.add(&format!("Event {i}"), now(), now() + h(1), "")
                .unwrap();
        }
        assert_eq!(
            cal.add("overflow", now(), now() + h(1), ""),
            Err(CalendarError::Full)
        );
        assert_eq!(cal.events().len(), 200);
    }

    #[test]
    fn save_failure_leaves_memory_untouched() {
        let mut cal = Calendar::default();
        let item = cal.add("existing", now(), now() + h(1), "").unwrap();
        let r = cal.add_with("new", now(), now() + h(1), "", |_| Err::<(), _>("disk"));
        assert_eq!(r, Err(CalendarError::Save("disk".into())));
        let r = cal.remove_with(&item.id, |_| Err::<(), _>("disk"));
        assert_eq!(r, Err(CalendarError::Save("disk".into())));
        assert_eq!(cal.events().len(), 1);
        assert_eq!(cal.events()[0].id, item.id);
    }

    #[test]
    fn load_bounds_and_normalizes_stored_records() {
        let ev = |id: &str, title: &str, end: DateTime<FixedOffset>, url: &str| CalendarEvent {
            id: id.into(),
            title: title.into(),
            start: now(),
            end,
            meeting_url: url.into(),
        };
        let mut stored = vec![
            None,
            Some(ev("keep", "  Existing  ", now() + h(1), "")),
            Some(ev("keep", "duplicate", now() + h(1), "")),
            Some(ev("", "bad dates", now(), "")),
            Some(ev("", "bad URL", now() + h(1), "file:///C:/bad")),
            Some(ev("long", &"t".repeat(200), now() + h(1), "")),
        ];
        stored.extend((0..220).map(|i| Some(ev(&format!("id{i}"), "event", now() + h(1), ""))));
        let cal = Calendar::load(stored);
        assert_eq!(cal.events().len(), 200);
        assert_eq!(cal.events()[0].id, "keep");
        assert_eq!(cal.events()[0].title, "Existing");
        assert_eq!(cal.events()[1].title.len(), 120);
        let ids: HashSet<_> = cal.events().iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), 200);
    }

    #[test]
    fn stored_null_entries_and_missing_fields_do_not_crash() {
        let stored: Vec<Option<CalendarEvent>> = serde_json::from_str(r#"[null,{"Title":"no dates"},{"Id":"a","Title":"ok","Start":"2026-10-14T12:00:00+07:00","End":"2026-10-14T13:00:00+07:00"}]"#).unwrap();
        let cal = Calendar::load(stored);
        assert_eq!(cal.events().len(), 1);
        assert_eq!(cal.events()[0].id, "a");
    }

    #[test]
    fn on_day_uses_event_offset() {
        let mut cal = Calendar::default();
        cal.add("late", now() + h(13), now() + h(14), "").unwrap();
        assert_eq!(
            cal.on_day(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap())
                .len(),
            0
        );
        assert_eq!(
            cal.on_day(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
                .len(),
            1
        );
    }

    #[test]
    fn week_is_monday_first_and_clamped() {
        let wed = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        let week = week_of(wed);
        assert_eq!(week[0], NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
        assert_eq!(week[6], NaiveDate::from_ymd_opt(2026, 10, 18).unwrap());
        let sunday = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(week_of(sunday)[0], week[0]);
        assert_eq!(week_of(max_date())[6], max_date());
        assert_eq!(week_of(min_date())[0], min_date());
    }

    #[test]
    fn month_grid_layouts() {
        // October 2026 starts on a Thursday.
        let monday_first = month_grid(2026, 10, Weekday::Mon);
        assert_eq!(
            monday_first[0],
            [None, None, None, Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(monday_first[4][5], Some(31));
        assert_eq!(monday_first[5], [None; 7]);
        let sunday_first = month_grid(2026, 10, Weekday::Sun);
        assert_eq!(
            sunday_first[0],
            [None, None, None, None, Some(1), Some(2), Some(3)]
        );
        // February 2026 (28 days) starting on Sunday fills exactly four Monday-first weeks + 1.
        let feb = month_grid(2026, 2, Weekday::Mon);
        assert_eq!(feb[0][6], Some(1));
        assert_eq!(feb[4][5], Some(28));
        // A 31-day month starting on the last column needs all six rows.
        let aug = month_grid(2026, 8, Weekday::Mon);
        assert_eq!(aug[0][5], Some(1));
        assert_eq!(aug[5][0], Some(31));
        assert_eq!(month_grid(2026, 13, Weekday::Mon), [[None; 7]; 6]);
        let leap = month_grid(2028, 2, Weekday::Mon)
            .iter()
            .flatten()
            .flatten()
            .count();
        assert_eq!(leap, 29);
    }

    #[test]
    fn month_navigation_bounds() {
        assert_eq!(shift_month(2026, 12, true), Some((2027, 1)));
        assert_eq!(shift_month(2026, 1, false), Some((2025, 12)));
        assert_eq!(shift_month(9999, 12, true), None);
        assert_eq!(shift_month(1, 1, false), None);
        assert_eq!(days_in_month(9999, 12), 31);
    }
}
