//! Quiet hours: a daily period when a target holds its deliveries back until the period ends, or
//! drops them. Times are wall-clock times in the target's time zone, so they follow daylight saving.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuietHours {
    pub enabled: bool,
    /// `HH:MM`. A period whose end isn't after its start runs past midnight; equal times are 24 hours.
    pub start: String,
    pub end: String,
    /// The days a period starts on, 0 (Sunday) to 6 (Saturday).
    pub days: Vec<u8>,
    /// An IANA name such as `Europe/Stockholm`. Empty uses the server's time zone (`TZ`).
    pub timezone: String,
    pub action: QuietAction,
    /// Notifications with at least this priority go through anyway. None lets nothing through.
    pub min_priority: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuietAction {
    /// Deliver once quiet hours end.
    #[default]
    Hold,
    /// Don't deliver at all.
    Drop,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            start: "22:00".into(),
            end: "07:00".into(),
            days: (0..7).collect(),
            timezone: String::new(),
            action: QuietAction::Hold,
            min_priority: Some(5),
        }
    }
}

/// What quiet hours do to a delivery created now.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Send,
    /// Hold it until this time (Unix milliseconds).
    Hold(i64),
    Drop,
}

impl QuietHours {
    /// Every problem with the settings, for `Graph::validate`. Only checked when enabled.
    pub fn check(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut errors = Vec::new();
        for (what, t) in [("start", &self.start), ("end", &self.end)] {
            if parse_time(t).is_none() {
                errors.push(format!(
                    "quiet hours {what} \"{t}\" isn't a time like 22:00"
                ));
            }
        }
        if self.days.is_empty() {
            errors.push("quiet hours need at least one day".into());
        }
        if self.days.iter().any(|d| *d > 6) {
            errors.push("quiet hours days are 0 (Sunday) to 6 (Saturday)".into());
        }
        if let Err(e) = self.time_zone() {
            errors.push(e);
        }
        errors
    }

    /// What to do with a delivery of a notification with this priority, created at `now_ms`.
    pub fn verdict(&self, priority: i64, now_ms: i64) -> Result<Verdict, String> {
        if !self.enabled || self.min_priority.is_some_and(|p| priority >= p) {
            return Ok(Verdict::Send);
        }
        let Some(until) = self.quiet_until(now_ms)? else {
            return Ok(Verdict::Send);
        };
        Ok(match self.action {
            QuietAction::Hold => Verdict::Hold(until),
            QuietAction::Drop => Verdict::Drop,
        })
    }

    /// When the quiet period containing `now_ms` ends, following periods that run into each other.
    pub fn quiet_until(&self, now_ms: i64) -> Result<Option<i64>, String> {
        let mut until = None;
        let mut at = now_ms;
        // A week of back-to-back 24-hour periods is the most there can be.
        for _ in 0..8 {
            match self.period_end(at)? {
                Some(end) if end > at => {
                    until = Some(end);
                    at = end;
                }
                _ => break,
            }
        }
        Ok(until)
    }

    /// The end of a period that contains `at_ms`, if one does.
    fn period_end(&self, at_ms: i64) -> Result<Option<i64>, String> {
        let tz = self.time_zone()?;
        let (start, end) = match (parse_time(&self.start), parse_time(&self.end)) {
            (Some(s), Some(e)) => (s, e),
            _ => return Err("quiet hours have a bad start or end time".into()),
        };
        let err = |e: jiff::Error| e.to_string();
        let at = Timestamp::from_millisecond(at_ms).map_err(err)?;
        let today = at.to_zoned_datetime(tz.clone()).date();
        // A period that started yesterday may still be running.
        for day in [today.yesterday().map_err(err)?, today] {
            let weekday = day.weekday().to_sunday_zero_offset() as u8;
            if !self.days.contains(&weekday) {
                continue;
            }
            let from = instant(day, start, &tz).map_err(err)?;
            let end_day = if end <= start {
                day.tomorrow().map_err(err)?
            } else {
                day
            };
            let to = instant(end_day, end, &tz).map_err(err)?;
            if from <= at_ms && at_ms < to {
                return Ok(Some(to));
            }
        }
        Ok(None)
    }

    fn time_zone(&self) -> Result<TimeZone, String> {
        let name = self.timezone.trim();
        if name.is_empty() {
            return Ok(TimeZone::system());
        }
        TimeZone::get(name).map_err(|_| format!("\"{name}\" isn't a known time zone"))
    }
}

/// `HH:MM` as (hour, minute).
fn parse_time(s: &str) -> Option<(i8, i8)> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i8, i8) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

/// A wall-clock time on a day, as Unix milliseconds. A time skipped by daylight saving moves forward.
fn instant(day: Date, (h, m): (i8, i8), tz: &TimeZone) -> Result<i64, jiff::Error> {
    Ok(day
        .at(h, m, 0, 0)
        .to_zoned_datetime(tz.clone())?
        .timestamp()
        .as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> i64 {
        s.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn night() -> QuietHours {
        QuietHours {
            enabled: true,
            timezone: "Europe/Stockholm".into(),
            ..Default::default()
        }
    }

    #[test]
    fn holds_across_midnight() {
        let q = night();
        assert!(q.check().is_empty());
        // 23:30 in Stockholm (UTC+1 in winter) is held until 07:00 the next morning.
        assert_eq!(
            q.verdict(3, ms("2026-11-24T22:30:00Z")),
            Ok(Verdict::Hold(ms("2026-11-25T06:00:00Z")))
        );
        // 03:00 is still inside the period that started the evening before.
        assert_eq!(
            q.verdict(3, ms("2026-11-25T02:00:00Z")),
            Ok(Verdict::Hold(ms("2026-11-25T06:00:00Z")))
        );
        assert_eq!(q.verdict(3, ms("2026-11-25T12:00:00Z")), Ok(Verdict::Send));
        assert_eq!(q.verdict(5, ms("2026-11-24T22:30:00Z")), Ok(Verdict::Send));
        let drop = QuietHours {
            action: QuietAction::Drop,
            min_priority: None,
            ..night()
        };
        assert_eq!(
            drop.verdict(5, ms("2026-11-24T22:30:00Z")),
            Ok(Verdict::Drop)
        );
    }

    #[test]
    fn respects_days_and_daylight_saving() {
        // Weekends only: periods starting Friday (5) and Saturday (6) evening.
        let q = QuietHours {
            days: vec![5, 6],
            ..night()
        };
        // Sunday 2026-11-29 03:00 is in Saturday's period; Monday 03:00 isn't (Sunday's isn't on).
        assert!(matches!(
            q.verdict(3, ms("2026-11-29T02:00:00Z")),
            Ok(Verdict::Hold(_))
        ));
        assert_eq!(q.verdict(3, ms("2026-11-30T02:00:00Z")), Ok(Verdict::Send));
        // In summer Stockholm is UTC+2, so 07:00 local is 05:00 UTC.
        assert_eq!(
            night().quiet_until(ms("2026-07-01T23:00:00Z")),
            Ok(Some(ms("2026-07-02T05:00:00Z")))
        );
    }

    #[test]
    fn joins_back_to_back_periods() {
        // 24 hours on Monday (1) and Tuesday (2): Monday 09:00 is held until Wednesday 09:00.
        let q = QuietHours {
            start: "09:00".into(),
            end: "09:00".into(),
            days: vec![1, 2],
            timezone: "UTC".into(),
            ..night()
        };
        assert_eq!(
            q.quiet_until(ms("2026-11-23T09:00:00Z")),
            Ok(Some(ms("2026-11-25T09:00:00Z")))
        );
    }

    #[test]
    fn rejects_bad_settings() {
        let q = QuietHours {
            start: "25:00".into(),
            days: vec![],
            timezone: "Mars/Olympus".into(),
            ..night()
        };
        assert_eq!(q.check().len(), 3);
    }
}
