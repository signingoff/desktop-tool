use crate::{
    market::{Calendar, Session},
    quote::{self, Quotes},
};
use chrono::Utc;
use std::{
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

const QUOTE_INTERVAL: Duration = Duration::from_secs(2);
pub(super) type Update = (Session, Result<Quotes, String>);

pub(super) fn start(calendar: Arc<Calendar>) -> (mpsc::Receiver<Update>, mpsc::SyncSender<()>) {
    let (tx, rx) = mpsc::channel();
    let (refresh, wake) = mpsc::sync_channel(1);
    let worker_calendar = calendar;
    std::thread::spawn(move || {
        let agent = quote::agent();
        loop {
            let started = Instant::now();
            if let Some(session) = worker_calendar.active(Utc::now()) {
                let result = agent
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|a| quote::fetch(a, session));
                if tx.send((session, result)).is_err() {
                    break;
                }
            }
            if matches!(
                wake.recv_timeout(QUOTE_INTERVAL.saturating_sub(started.elapsed())),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ) {
                break;
            }
        }
    });
    (rx, refresh)
}

pub(super) fn apply_current(
    current: Option<Session>,
    quotes: &mut Quotes,
    error: &mut Option<String>,
    (session, update): Update,
) -> bool {
    if Some(session) != current {
        return false;
    }
    apply(quotes, error, update)
}

/// Preserve the last successful batch on transport errors; identical updates
/// do not force a repaint. A successful batch clears the stale marker.
fn apply(quotes: &mut Quotes, error: &mut Option<String>, update: Result<Quotes, String>) -> bool {
    match update {
        Ok(next) => {
            let changed = *quotes != next || error.is_some();
            *quotes = next;
            *error = None;
            changed
        }
        Err(next) => {
            let changed = error.as_ref() != Some(&next);
            *error = Some(next);
            changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quote::Quote;

    #[test]
    fn transport_failure_preserves_prices_and_recovery_clears_stale_state() {
        let mut quotes = vec![Ok(Quote {
            price: 3000.0,
            percent: -1.25,
            time: chrono::NaiveDate::from_ymd_opt(2026, 10, 16)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        })];
        let original = quotes.clone();
        let mut error = None;
        assert!(apply(&mut quotes, &mut error, Err("offline".into())));
        assert_eq!(quotes, original);
        assert!(!apply(&mut quotes, &mut error, Err("offline".into())));
        assert!(apply(&mut quotes, &mut error, Ok(original.clone())));
        assert!(error.is_none());
        assert!(!apply(&mut quotes, &mut error, Ok(original)));
    }
    #[test]
    fn stale_session_response_cannot_replace_current_quotes_or_error() {
        let current = Session {
            market: crate::market::Market::China,
            date: chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
        };
        let previous = Session {
            date: current.date.pred_opt().unwrap(),
            ..current
        };
        let mut quotes = vec![Err("current quote".into())];
        let original = quotes.clone();
        let mut error = Some("current error".into());
        for active in [Some(current), None] {
            assert!(!apply_current(
                active,
                &mut quotes,
                &mut error,
                (previous, Ok(vec![]))
            ));
            assert!(!apply_current(
                active,
                &mut quotes,
                &mut error,
                (previous, Err("old error".into()))
            ));
            assert_eq!(quotes, original);
            assert_eq!(error.as_deref(), Some("current error"));
        }
    }
}
