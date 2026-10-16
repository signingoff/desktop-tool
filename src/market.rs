use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::{America::New_York, Asia::Shanghai};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Market {
    China,
    Us,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub market: Market,
    pub date: NaiveDate,
}
#[derive(Deserialize)]
pub struct Calendar {
    cn_years: Vec<i32>,
    us_years: Vec<i32>,
    cn_closed: Vec<NaiveDate>,
    us_closed: Vec<NaiveDate>,
    us_early: Vec<NaiveDate>,
}
impl Calendar {
    pub fn load() -> Result<Self, String> {
        let path = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("calendar.json");
        let text = if path.exists() {
            std::fs::read_to_string(path).map_err(|e| e.to_string())?
        } else {
            include_str!("../calendar.json").to_owned()
        };
        serde_json::from_str(&text).map_err(|e| format!("交易日历无效：{e}"))
    }
    fn trading_day(&self, market: Market, date: NaiveDate) -> bool {
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return false;
        }
        let (years, closed) = match market {
            Market::China => (&self.cn_years, &self.cn_closed),
            Market::Us => (&self.us_years, &self.us_closed),
        };
        years.contains(&date.year()) && !closed.contains(&date)
    }
    pub fn active(&self, now: DateTime<Utc>) -> Option<Session> {
        let us = now.with_timezone(&New_York);
        let minutes = us.hour() * 60 + us.minute();
        let close = if self.us_early.contains(&us.date_naive()) {
            13 * 60
        } else {
            16 * 60
        };
        if self.trading_day(Market::Us, us.date_naive()) && (570..close).contains(&minutes) {
            return Some(Session {
                market: Market::Us,
                date: us.date_naive(),
            });
        }
        let cn = now.with_timezone(&Shanghai);
        // Retain Chinese close until the US opens, or midnight on US holidays.
        if self.trading_day(Market::China, cn.date_naive()) && cn.hour() * 60 + cn.minute() >= 570 {
            return Some(Session {
                market: Market::China,
                date: cn.date_naive(),
            });
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn active(s: &str) -> Option<Session> {
        let c: Calendar = serde_json::from_str(include_str!("../calendar.json")).unwrap();
        c.active(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }
    #[test]
    fn china_lunch_close_and_holidays() {
        for t in ["2026-10-16T12:00:00+08:00", "2026-10-16T19:00:00+08:00"] {
            assert_eq!(active(t).unwrap().market, Market::China);
        }
        assert!(active("2026-10-01T10:00:00+08:00").is_none());
        assert!(active("2026-10-10T10:00:00+08:00").is_none());
        assert!(active("2026-10-16T09:29:59+08:00").is_none());
    }
    #[test]
    fn us_dst_cross_midnight_and_early_close() {
        assert_eq!(
            active("2026-10-16T21:30:00+08:00").unwrap().market,
            Market::Us
        );
        let s = active("2026-10-17T01:00:00+08:00").unwrap();
        assert_eq!(s.market, Market::Us);
        assert_eq!(s.date.to_string(), "2026-10-16");
        assert!(active("2026-10-17T04:00:00+08:00").is_none());
        assert_eq!(
            active("2026-11-02T22:29:00+08:00").unwrap().market,
            Market::China
        );
        assert_eq!(
            active("2026-11-02T22:30:00+08:00").unwrap().market,
            Market::Us
        );
        assert!(active("2026-11-28T02:00:00+08:00").is_none());
        assert_eq!(
            active("2026-11-26T23:00:00+08:00").unwrap().market,
            Market::China
        );
    }
    #[test]
    fn unknown_calendar_fails_closed() {
        assert!(active("2029-10-16T10:00:00+08:00").is_none());
    }
}
