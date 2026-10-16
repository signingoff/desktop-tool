use crate::market::{Market, Session};
use chrono::NaiveDateTime;
use std::{sync::Arc, time::Duration};
#[derive(Debug, Clone, Copy)]
pub struct Index {
    pub symbol: &'static str,
    pub name: &'static str,
}
pub const CHINA: &[Index] = &[
    Index {
        symbol: "sh000001",
        name: "上证",
    },
    Index {
        symbol: "sz399006",
        name: "创业板",
    },
    Index {
        symbol: "sz399001",
        name: "深成指",
    },
    Index {
        symbol: "sh000688",
        name: "科创50",
    },
    Index {
        symbol: "bj899050",
        name: "北证50",
    },
];
pub const US: &[Index] = &[
    Index {
        symbol: "us.IXIC",
        name: "纳指",
    },
    Index {
        symbol: "us.INX",
        name: "标普500",
    },
    Index {
        symbol: "us.DJI",
        name: "道指",
    },
];
pub fn indices(market: Market) -> &'static [Index] {
    match market {
        Market::China => CHINA,
        Market::Us => US,
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub price: f64,
    pub percent: f64,
    pub time: NaiveDateTime,
}
pub fn parse(line: &str, index: Index, session: Session) -> Result<Quote, String> {
    let prefix = format!("v_{}=\"", index.symbol);
    let payload = line
        .trim()
        .strip_prefix(&prefix)
        .and_then(|s| s.strip_suffix("\";"))
        .ok_or("行情响应格式错误")?;
    let fields: Vec<_> = payload.split('~').collect();
    if fields.len() < 33 || fields[2] != &index.symbol[2..] {
        return Err("行情字段缺失或代码不匹配".into());
    }
    let number = |i: usize| -> Result<f64, String> {
        let n: f64 = fields[i].parse().map_err(|_| "行情数字无效")?;
        if !n.is_finite() {
            return Err("行情数字无效".into());
        }
        Ok(n)
    };
    let price = number(3)?;
    let previous = number(4)?;
    if price <= 0.0 || previous <= 0.0 {
        return Err("行情价格无效".into());
    }
    let format = if session.market == Market::China {
        "%Y%m%d%H%M%S"
    } else {
        "%Y-%m-%d %H:%M:%S"
    };
    let time = NaiveDateTime::parse_from_str(fields[30], format).map_err(|_| "行情时间无效")?;
    if time.date() != session.date {
        return Err("等待当天行情".into());
    }
    let percent = (price / previous - 1.0) * 100.0;
    if !percent.is_finite() {
        return Err("行情涨跌幅无效".into());
    }
    Ok(Quote {
        price,
        percent,
        time,
    })
}
pub type Quotes = Vec<Result<Quote, String>>;
pub fn fetch(agent: &ureq::Agent, session: Session) -> Result<Quotes, String> {
    use std::io::Read;
    let list = indices(session.market);
    let url = format!(
        "https://qt.gtimg.cn/q={}",
        list.iter().map(|i| i.symbol).collect::<Vec<_>>().join(",")
    );
    let response = agent
        .get(&url)
        .set("User-Agent", "sse-taskbar/0.1")
        .call()
        .map_err(|_| "行情连接失败")?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(64 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|_| "行情读取失败")?;
    // Only consume ASCII fields; provider names are GBK encoded.
    let raw = String::from_utf8_lossy(&bytes);
    Ok(list
        .iter()
        .map(|index| {
            let prefix = format!("v_{}=", index.symbol);
            raw.lines()
                .find(|line| line.starts_with(&prefix))
                .ok_or_else(|| "缺少行情".to_string())
                .and_then(|line| parse(line, *index, session))
        })
        .collect())
}
pub fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|_| "无法初始化安全连接")?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(8))
        .build())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(symbol: &str, price: &str, previous: &str, time: &str) -> String {
        let mut f = vec![""; 33];
        f[2] = &symbol[2..];
        f[3] = price;
        f[4] = previous;
        f[30] = time;
        format!("v_{symbol}=\"{}\";", f.join("~"))
    }
    fn session(market: Market) -> Session {
        Session {
            market,
            date: chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
        }
    }
    #[test]
    fn both_markets_and_direction() {
        let q = parse(
            &fixture("sh000001", "3742.66", "3707.48", "20261016120500"),
            CHINA[0],
            session(Market::China),
        )
        .unwrap();
        assert_eq!(q.price, 3742.66);
        assert!((q.percent - 0.94889).abs() < 0.0001);
        let q = parse(
            &fixture("us.IXIC", "99", "100", "2026-10-16 09:35:00"),
            US[0],
            session(Market::Us),
        )
        .unwrap();
        assert!((q.percent + 1.0).abs() < 1e-10);
    }
    #[test]
    fn rejects_bad_prices_previous_day_and_bad_date() {
        for p in ["NaN", "inf", "0", "-1", "oops"] {
            assert!(parse(
                &fixture("sh000001", p, "100", "20261016120500"),
                CHINA[0],
                session(Market::China)
            )
            .is_err());
        }
        for time in ["20261015120500", "20261316120500", "garbage"] {
            assert!(parse(
                &fixture("sh000001", "100", "99", time),
                CHINA[0],
                session(Market::China)
            )
            .is_err());
        }
        assert!(parse("v_sh000001=\"1~2\";", CHINA[0], session(Market::China)).is_err());
    }
    #[test]
    fn recorded_provider_response_for_all_eight_indices() {
        let raw = include_str!("../tests/fixtures/quotes-2026-10-16.txt");
        for market in [Market::China, Market::Us] {
            let date = chrono::NaiveDate::from_ymd_opt(
                2026,
                10,
                if market == Market::China { 16 } else { 15 },
            )
            .unwrap();
            for index in indices(market) {
                let prefix = format!("v_{}=", index.symbol);
                let line = raw.lines().find(|line| line.starts_with(&prefix)).unwrap();
                let quote = parse(line, *index, Session { market, date }).unwrap();
                assert!(quote.price > 0.0);
                assert_eq!(quote.time.date(), date);
            }
        }
    }
    #[test]
    #[ignore = "Requires network and a Chinese trading day with current-day quotes"]
    fn live_china_quotes() {
        let date = chrono::Utc::now()
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive();
        let quotes = fetch(
            &agent().unwrap(),
            Session {
                market: Market::China,
                date,
            },
        )
        .unwrap();
        assert_eq!(quotes.len(), 5);
        for quote in quotes {
            assert!(quote.is_ok(), "{quote:?}");
        }
    }
}
