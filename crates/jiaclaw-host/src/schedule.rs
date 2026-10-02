// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Persistable schedules with bounded, strictly-future UTC calculation.
//!
//! Croner parses five-field cron and finds civil calendar candidates. Chrono-TZ
//! resolves those candidates: nonexistent local times are skipped; repeated
//! local times run only at their first UTC occurrence, for every cron pattern.
//! This deliberately avoids Croner's separate fixed/wildcard DST policies.

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Datelike, LocalResult, Months, TimeZone, Utc};
use chrono_tz::Tz;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

const MAX_INTERVAL_SECONDS: u64 = 31_536_000;
const SEARCH_YEARS: u32 = 8;
const MAX_CANDIDATES: usize = 4096;
// Deterministic validation anchor, including the Gregorian leap-century year.
const VALIDATION_ANCHOR_MS: i64 = 946_684_800_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleSpec {
    Cron {
        expression: String,
        timezone: String,
    },
    Interval {
        seconds: u64,
    },
}

impl ScheduleSpec {
    /// Validate syntax, policy and the existence of a bounded future occurrence.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Interval { seconds } => validate_interval(*seconds),
            Self::Cron { .. } => self.next_after(VALIDATION_ANCHOR_MS).map(|_| ()),
        }
    }

    /// Return a UTC millisecond timestamp strictly greater than `after_ms`.
    ///
    /// Cron searches at most eight calendar years and 4096 civil candidates.
    /// Intervals preserve the millisecond phase of the supplied timestamp and
    /// use checked arithmetic. Callers own missed-run and clock-change policy.
    pub fn next_after(&self, after_ms: i64) -> Result<i64> {
        match self {
            Self::Interval { seconds } => {
                validate_interval(*seconds)?;
                let increment = i64::try_from(*seconds)?
                    .checked_mul(1000)
                    .context("interval duration overflow")?;
                after_ms
                    .checked_add(increment)
                    .context("next interval timestamp overflow")
            }
            Self::Cron {
                expression,
                timezone,
            } => next_cron(expression, timezone, after_ms),
        }
    }
}

fn validate_interval(seconds: u64) -> Result<()> {
    ensure!(
        (1..=MAX_INTERVAL_SECONDS).contains(&seconds),
        "interval seconds must be in 1..=31536000"
    );
    Ok(())
}

fn next_cron(expression: &str, timezone: &str, after_ms: i64) -> Result<i64> {
    ensure!(
        !expression.is_empty()
            && expression.len() <= 128
            && expression.split_whitespace().count() == 5
            && !expression.contains(['+', '@']),
        "cron requires five fields, at most 128 bytes, without nicknames or AND modifiers"
    );
    ensure!(timezone.len() <= 128, "timezone name exceeds 128 bytes");
    let timezone: Tz = timezone.parse().context("invalid IANA timezone")?;
    let after = DateTime::<Utc>::from_timestamp_millis(after_ms)
        .context("cron timestamp is outside the supported calendar")?;
    let horizon = after
        .checked_add_months(Months::new(SEARCH_YEARS * 12))
        .context("cron search horizon overflow")?;
    let mut cursor = after.with_timezone(&timezone).naive_local();
    let year = cursor.year();
    let search_years = i32::try_from(SEARCH_YEARS)?;
    ensure!(
        (croner::YEAR_LOWER_LIMIT..=croner::YEAR_UPPER_LIMIT - search_years).contains(&year),
        "cron timestamp is outside the supported calendar"
    );
    // A library-parsed year range bounds its internal search too: checking only
    // the returned date would still allow an impossible expression to scan millennia.
    let bounded = format!("0 {expression} {year}-{}", year + search_years);
    let cron = CronParser::builder()
        .seconds(Seconds::Required)
        .year(Year::Required)
        .dom_and_dow(false)
        .build()
        .parse(&bounded)
        .context("invalid cron expression")?;
    for _ in 0..MAX_CANDIDATES {
        let candidate = cron
            .find_next_occurrence(&cursor, false)
            .context("cron has no occurrence within the eight-year search window")?;
        ensure!(candidate > cursor, "cron candidate did not advance");
        cursor = candidate;
        let resolved = match timezone.from_local_datetime(&candidate) {
            LocalResult::None => continue,
            LocalResult::Single(value) => value,
            LocalResult::Ambiguous(first, second) => first.min(second),
        };
        let next = resolved.timestamp_millis();
        if next <= after_ms {
            // During the second pass through a folded local hour, its first
            // occurrence is already in the past and must never be replayed.
            continue;
        }
        ensure!(
            next <= horizon.timestamp_millis(),
            "cron has no occurrence within the eight-year search window"
        );
        return Ok(next);
    }
    bail!("cron exceeded the bounded civil-candidate search limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn millis(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    }

    fn cron(expression: &str, timezone: &str) -> ScheduleSpec {
        ScheduleSpec::Cron {
            expression: expression.into(),
            timezone: timezone.into(),
        }
    }

    fn assert_next(spec: &ScheduleSpec, after: &str, expected: &str) {
        assert_eq!(spec.next_after(millis(after)).unwrap(), millis(expected));
    }

    #[test]
    fn tagged_policy_roundtrips_and_rejects_unknown_fields() {
        let value =
            json!({"kind":"cron", "expression":"0 9 * * MON-FRI", "timezone":"Asia/Shanghai"});
        let spec: ScheduleSpec = serde_json::from_value(value.clone()).unwrap();
        spec.validate().unwrap();
        assert_eq!(serde_json::to_value(spec).unwrap(), value);
        for value in [
            json!({"kind":"cron", "expression":"* * * * *", "timezone":"UTC", "seconds":60}),
            json!({"kind":"interval", "seconds":60, "timezone":"UTC"}),
            json!({"kind":"interval", "seconds":-1}),
            json!({"kind":"interval", "seconds":1.5}),
            json!({"seconds":60}),
        ] {
            assert!(serde_json::from_value::<ScheduleSpec>(value).is_err());
        }
    }

    #[test]
    fn utc_minutes_are_strictly_future_including_subseconds() {
        let spec = cron("*/5 * * * *", "UTC");
        for after in ["2026-10-02T12:00:00Z", "2026-10-02T12:00:00.500Z"] {
            assert_next(&spec, after, "2026-10-02T12:05:00Z");
        }
        assert_next(&spec, "2026-10-02T11:59:59.999Z", "2026-10-02T12:00:00Z");
        // Moving the supplied clock backwards still returns strictly after that clock.
        assert_next(&spec, "2026-10-02T11:58:00Z", "2026-10-02T12:00:00Z");
    }

    #[test]
    fn named_timezone_and_day_of_month_or_week_semantics() {
        assert_next(
            &cron("0 9 * * MON-FRI", "Asia/Shanghai"),
            "2026-10-02T01:00:00Z",
            "2026-10-05T01:00:00Z",
        );
        let spec = cron("0 9 1 * MON", "UTC");
        assert_next(&spec, "2026-09-30T09:00:00Z", "2026-10-01T09:00:00Z");
        assert_next(&spec, "2026-10-01T09:00:00Z", "2026-10-05T09:00:00Z");
    }

    #[test]
    fn dst_gap_and_skipped_civil_day_are_not_compensated() {
        assert_next(
            &cron("30 2 * * *", "America/New_York"),
            "2026-03-07T07:30:00Z",
            "2026-03-09T06:30:00Z",
        );
        assert_next(
            &cron("0 12 * * *", "Pacific/Apia"),
            "2011-12-29T22:00:00Z",
            "2011-12-30T22:00:00Z",
        );
    }

    #[test]
    fn folded_fixed_time_runs_only_at_earliest_utc_occurrence() {
        let spec = cron("30 1 * * *", "America/New_York");
        assert_next(&spec, "2026-11-01T05:00:00Z", "2026-11-01T05:30:00Z");
        assert_next(&spec, "2026-11-01T05:30:00Z", "2026-11-02T06:30:00Z");
        // 06:15 UTC is the second 01:15. The first 01:30 is already past.
        assert_next(&spec, "2026-11-01T06:15:00Z", "2026-11-02T06:30:00Z");
    }

    #[test]
    fn folded_wildcard_minutes_use_the_same_once_only_policy() {
        let spec = cron("* * * * *", "America/New_York");
        assert_next(&spec, "2026-11-01T05:58:00Z", "2026-11-01T05:59:00Z");
        assert_next(&spec, "2026-11-01T05:59:00Z", "2026-11-01T07:00:00Z");
        assert_next(&spec, "2026-11-01T06:15:00Z", "2026-11-01T07:00:00Z");
    }

    #[test]
    fn leap_century_and_impossible_calendar_are_bounded() {
        let leap = cron("0 0 29 2 *", "UTC");
        leap.validate().unwrap();
        assert_next(&leap, "2096-02-29T00:00:00Z", "2104-02-29T00:00:00Z");
        let start = std::time::Instant::now();
        assert!(cron("0 0 31 2 *", "UTC").validate().is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn malformed_policy_and_out_of_range_calendar_are_rejected() {
        for expression in [
            "",
            "* * * *",
            "0 * * * * *",
            "0 0 * * * 2026",
            "@hourly",
            "0 0 1 * +MON",
            "61 * * * *",
            "*/0 * * * *",
            "0 25 * * *",
            "0 0 * 13 *",
        ] {
            assert!(cron(expression, "UTC").validate().is_err(), "{expression}");
        }
        assert!(cron(&"* ".repeat(65), "UTC").validate().is_err());
        assert!(cron("* * * * *", "invalid/timezone").validate().is_err());
        assert!(cron("* * * * *", "UTC").next_after(i64::MAX).is_err());
        assert!(cron("* * * * *", "UTC").next_after(i64::MIN).is_err());
    }

    #[test]
    fn intervals_preserve_phase_validate_bounds_and_check_overflow() {
        let minute = ScheduleSpec::Interval { seconds: 60 };
        assert_eq!(minute.next_after(0).unwrap(), 60_000);
        assert_eq!(minute.next_after(-501).unwrap(), 59_499);
        assert_eq!(minute.next_after(1234).unwrap(), 61_234);
        for seconds in [1, MAX_INTERVAL_SECONDS] {
            let spec = ScheduleSpec::Interval { seconds };
            spec.validate().unwrap();
            assert!(spec.next_after(i64::MAX).is_err());
            assert!(spec.next_after(i64::MIN).unwrap() > i64::MIN);
        }
        for seconds in [0, MAX_INTERVAL_SECONDS + 1, u64::MAX] {
            let spec = ScheduleSpec::Interval { seconds };
            assert!(spec.validate().is_err());
            assert!(spec.next_after(0).is_err());
        }
    }
}
