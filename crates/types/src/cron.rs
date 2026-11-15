//! Pure 5-field POSIX cron parser, timezone evaluator, and human-readable formatter.
use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, TimeZone, Timelike, Utc};

/// Parsed 5-field POSIX cron schedule (`minute hour day_of_month month day_of_week`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronSchedule {
    raw: String,
    minutes: u64,
    hours: u32,
    days_of_month: u32,
    months: u16,
    days_of_week: u8,
    dom_restricted: bool,
    dow_restricted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CronTimeZone {
    Utc,
    Local,
    Fixed(FixedOffset),
    Iana(&'static str),
}

impl CronTimeZone {
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Self::Local);
        };
        if s.eq_ignore_ascii_case("local") || s.eq_ignore_ascii_case("system") {
            return Ok(Self::Local);
        }
        if s.eq_ignore_ascii_case("utc")
            || s.eq_ignore_ascii_case("gmt")
            || s.eq_ignore_ascii_case("etc/utc")
            || s.eq_ignore_ascii_case("etc/gmt")
            || s == "Z"
        {
            return Ok(Self::Utc);
        }
        if let Some(offset) = parse_fixed_offset(s) {
            return Ok(Self::Fixed(offset));
        }
        if let Some((canon, _)) = IANA_ZONES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(s))
        {
            return Ok(Self::Iana(canon));
        }
        Err(format!(
            "Unsupported timezone `{s}` (use `Local`, `UTC`, an offset like `+08:00`, or a standard IANA zone such as `Asia/Shanghai` or `America/New_York`)"
        ))
    }

    pub fn canonical_label(&self) -> String {
        match self {
            Self::Local => "Local".into(),
            Self::Utc => "UTC".into(),
            Self::Fixed(off) => off.to_string(),
            Self::Iana(name) => (*name).into(),
        }
    }

    fn datetime_from_utc_ms(&self, utc_ms: u64) -> Option<ZonedMinute> {
        let secs = i64::try_from(utc_ms / 1000).ok()?;
        let utc = Utc.timestamp_opt(secs, 0).single()?;
        match self {
            Self::Utc => Some(ZonedMinute::from_dt(&utc)),
            Self::Local => {
                let local = utc.with_timezone(&Local);
                Some(ZonedMinute::from_dt(&local))
            }
            Self::Fixed(off) => {
                let dt = utc.with_timezone(off);
                Some(ZonedMinute::from_dt(&dt))
            }
            Self::Iana(name) => {
                let off = iana_offset_for_utc(name, &utc)?;
                let dt = utc.with_timezone(&off);
                Some(ZonedMinute::from_dt(&dt))
            }
        }
    }

    /// Formats `utc_ms` as `YYYY-MM-DD` and `YYYY-MM-DD HH:MM` in this timezone.
    pub fn format_timestamp(&self, utc_ms: u64) -> (String, String) {
        if let Some(z) = self.datetime_from_utc_ms(utc_ms) {
            let date = format!("{:04}-{:02}-{:02}", z.year, z.month, z.day);
            let dt = format!("{date} {:02}:{:02}", z.hour, z.minute);
            (date, dt)
        } else {
            ("1970-01-01".into(), "1970-01-01 00:00".into())
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ZonedMinute {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    weekday_sun0: u32,
}

impl ZonedMinute {
    fn from_dt<Tz: TimeZone>(dt: &DateTime<Tz>) -> Self {
        Self {
            year: dt.year(),
            month: dt.month(),
            day: dt.day(),
            hour: dt.hour(),
            minute: dt.minute(),
            weekday_sun0: dt.weekday().num_days_from_sunday(),
        }
    }
}

const IANA_ZONES: &[(&str, ZoneRule)] = &[
    ("Asia/Shanghai", ZoneRule::Fixed(8 * 3600)),
    ("Asia/Chongqing", ZoneRule::Fixed(8 * 3600)),
    ("Asia/Hong_Kong", ZoneRule::Fixed(8 * 3600)),
    ("Asia/Taipei", ZoneRule::Fixed(8 * 3600)),
    ("Asia/Singapore", ZoneRule::Fixed(8 * 3600)),
    ("Asia/Tokyo", ZoneRule::Fixed(9 * 3600)),
    ("Asia/Seoul", ZoneRule::Fixed(9 * 3600)),
    ("Asia/Kolkata", ZoneRule::Fixed(5 * 3600 + 1800)),
    ("Asia/Dubai", ZoneRule::Fixed(4 * 3600)),
    ("Asia/Bangkok", ZoneRule::Fixed(7 * 3600)),
    ("Asia/Jakarta", ZoneRule::Fixed(7 * 3600)),
    ("Europe/London", ZoneRule::Eu { std_offset: 0 }),
    ("Europe/Dublin", ZoneRule::Eu { std_offset: 0 }),
    ("Europe/Lisbon", ZoneRule::Eu { std_offset: 0 }),
    ("Europe/Paris", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Berlin", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Madrid", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Rome", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Amsterdam", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Zurich", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Stockholm", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Warsaw", ZoneRule::Eu { std_offset: 3600 }),
    ("Europe/Athens", ZoneRule::Eu { std_offset: 7200 }),
    ("Europe/Helsinki", ZoneRule::Eu { std_offset: 7200 }),
    ("Europe/Moscow", ZoneRule::Fixed(3 * 3600)),
    (
        "America/New_York",
        ZoneRule::Us {
            std_offset: -5 * 3600,
        },
    ),
    (
        "America/Toronto",
        ZoneRule::Us {
            std_offset: -5 * 3600,
        },
    ),
    (
        "America/Chicago",
        ZoneRule::Us {
            std_offset: -6 * 3600,
        },
    ),
    (
        "America/Denver",
        ZoneRule::Us {
            std_offset: -7 * 3600,
        },
    ),
    ("America/Phoenix", ZoneRule::Fixed(-7 * 3600)),
    (
        "America/Los_Angeles",
        ZoneRule::Us {
            std_offset: -8 * 3600,
        },
    ),
    (
        "America/Vancouver",
        ZoneRule::Us {
            std_offset: -8 * 3600,
        },
    ),
    ("America/Sao_Paulo", ZoneRule::Fixed(-3 * 3600)),
    ("Australia/Perth", ZoneRule::Fixed(8 * 3600)),
    (
        "Australia/Sydney",
        ZoneRule::AuSouth {
            std_offset: 10 * 3600,
        },
    ),
    (
        "Australia/Melbourne",
        ZoneRule::AuSouth {
            std_offset: 10 * 3600,
        },
    ),
    (
        "Pacific/Auckland",
        ZoneRule::Nz {
            std_offset: 12 * 3600,
        },
    ),
];

#[derive(Clone, Copy, Debug)]
enum ZoneRule {
    Fixed(i32),
    Us { std_offset: i32 },
    Eu { std_offset: i32 },
    AuSouth { std_offset: i32 },
    Nz { std_offset: i32 },
}

fn nth_weekday_of_month(year: i32, month: u32, weekday_sun0: u32, nth: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let first_wd = first.weekday().num_days_from_sunday();
    let delta = (7 + weekday_sun0 - first_wd) % 7;
    let day = 1 + delta + (nth - 1) * 7;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn last_sunday_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    let next_month_first = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    let last = next_month_first.pred_opt()?;
    let back = last.weekday().num_days_from_sunday();
    NaiveDate::from_ymd_opt(year, month, last.day() - back)
}

fn iana_offset_for_utc(name: &str, utc: &DateTime<Utc>) -> Option<FixedOffset> {
    let (_, rule) = IANA_ZONES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))?;
    let secs = match *rule {
        ZoneRule::Fixed(off) => off,
        ZoneRule::Us { std_offset } => {
            // US DST: 2nd Sunday in March at 02:00 local standard -> 1st Sunday in November at 02:00 local DST (01:00 standard).
            let year = utc.year();
            let start_date = nth_weekday_of_month(year, 3, 0, 2)?;
            let end_date = nth_weekday_of_month(year, 11, 0, 1)?;
            let start_utc =
                start_date.and_hms_opt(2, 0, 0)?.and_utc().timestamp() - i64::from(std_offset);
            let end_utc =
                end_date.and_hms_opt(2, 0, 0)?.and_utc().timestamp() - i64::from(std_offset + 3600);
            let ts = utc.timestamp();
            if ts >= start_utc && ts < end_utc {
                std_offset + 3600
            } else {
                std_offset
            }
        }
        ZoneRule::Eu { std_offset } => {
            // EU DST: last Sunday in March at 01:00 UTC -> last Sunday in October at 01:00 UTC.
            let year = utc.year();
            let start_utc = last_sunday_of_month(year, 3)?
                .and_hms_opt(1, 0, 0)?
                .and_utc()
                .timestamp();
            let end_utc = last_sunday_of_month(year, 10)?
                .and_hms_opt(1, 0, 0)?
                .and_utc()
                .timestamp();
            let ts = utc.timestamp();
            if ts >= start_utc && ts < end_utc {
                std_offset + 3600
            } else {
                std_offset
            }
        }
        ZoneRule::AuSouth { std_offset } => {
            // Southern Australia DST: 1st Sunday in October at 02:00 std -> 1st Sunday in April at 03:00 DST (02:00 std).
            let year = utc.year();
            let apr_end = nth_weekday_of_month(year, 4, 0, 1)?
                .and_hms_opt(2, 0, 0)?
                .and_utc()
                .timestamp()
                - i64::from(std_offset);
            let oct_start = nth_weekday_of_month(year, 10, 0, 1)?
                .and_hms_opt(2, 0, 0)?
                .and_utc()
                .timestamp()
                - i64::from(std_offset);
            let ts = utc.timestamp();
            if ts < apr_end || ts >= oct_start {
                std_offset + 3600
            } else {
                std_offset
            }
        }
        ZoneRule::Nz { std_offset } => {
            // NZ DST: last Sunday in September at 02:00 std -> 1st Sunday in April at 02:00 std.
            let year = utc.year();
            let apr_end = nth_weekday_of_month(year, 4, 0, 1)?
                .and_hms_opt(2, 0, 0)?
                .and_utc()
                .timestamp()
                - i64::from(std_offset);
            let sep_start = last_sunday_of_month(year, 9)?
                .and_hms_opt(2, 0, 0)?
                .and_utc()
                .timestamp()
                - i64::from(std_offset);
            let ts = utc.timestamp();
            if ts < apr_end || ts >= sep_start {
                std_offset + 3600
            } else {
                std_offset
            }
        }
    };
    FixedOffset::east_opt(secs)
}

fn parse_fixed_offset(raw: &str) -> Option<FixedOffset> {
    let s = raw
        .strip_prefix("UTC")
        .or_else(|| raw.strip_prefix("utc"))
        .or_else(|| raw.strip_prefix("GMT"))
        .or_else(|| raw.strip_prefix("gmt"))
        .unwrap_or(raw)
        .trim();
    let sign = if let Some(rest) = s.strip_prefix('+') {
        (1i32, rest)
    } else {
        let rest = s.strip_prefix('-')?;
        (-1i32, rest)
    };
    let (h_str, m_str) = if let Some((h, m)) = sign.1.split_once(':') {
        (h, m)
    } else if sign.1.len() == 4 {
        (&sign.1[..2], &sign.1[2..])
    } else if (1..=2).contains(&sign.1.len()) {
        (sign.1, "0")
    } else {
        return None;
    };
    let h: i32 = h_str.parse().ok()?;
    let m: i32 = m_str.parse().ok()?;
    if h > 14 || m >= 60 {
        return None;
    }
    FixedOffset::east_opt(sign.0 * (h * 3600 + m * 60))
}

impl CronSchedule {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Cron expression is required".into());
        }
        let expanded = match trimmed.to_ascii_lowercase().as_str() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            _ => trimmed,
        };
        let parts: Vec<&str> = expanded.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(format!(
                "Cron expression `{trimmed}` must have 5 fields (`minute hour day month weekday`), got {}",
                parts.len()
            ));
        }
        let (minutes, _) = parse_field(parts[0], 0, 59, &[], false, "minute")?;
        let (hours, _) = parse_field(parts[1], 0, 23, &[], false, "hour")?;
        let (days_of_month, dom_restricted) =
            parse_field(parts[2], 1, 31, &[], false, "day of month")?;
        let (months, _) = parse_field(parts[3], 1, 12, MONTH_NAMES, false, "month")?;
        let (days_of_week, dow_restricted) =
            parse_field(parts[4], 0, 7, DOW_NAMES, true, "day of week")?;

        Ok(Self {
            raw: trimmed.to_owned(),
            minutes,
            hours: hours as u32,
            days_of_month: days_of_month as u32,
            months: months as u16,
            days_of_week: days_of_week as u8,
            dom_restricted,
            dow_restricted,
        })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    fn matches(&self, z: &ZonedMinute) -> bool {
        if (self.minutes & (1u64 << z.minute)) == 0 {
            return false;
        }
        if (self.hours & (1u32 << z.hour)) == 0 {
            return false;
        }
        if (self.months & (1u16 << z.month)) == 0 {
            return false;
        }
        let dom_ok = (self.days_of_month & (1u32 << z.day)) != 0;
        let dow_ok = (self.days_of_week & (1u8 << z.weekday_sun0)) != 0;
        match (self.dom_restricted, self.dow_restricted) {
            (true, true) => dom_ok || dow_ok,
            (true, false) => dom_ok,
            (false, true) => dow_ok,
            (false, false) => true,
        }
    }

    /// Computes the next UTC timestamp in milliseconds strictly greater than `after_ms`,
    /// aligned to exact minute boundaries (`:00.000`).
    pub fn next_after_ms(&self, after_ms: u64, tz: CronTimeZone) -> Option<u64> {
        // Advance to the next full minute strictly after `after_ms`.
        let mut candidate_min = (after_ms / 60_000).checked_add(1)?;
        // Search up to 5 years ahead (covers Feb 29 leap-year schedules).
        let max_minutes = 5 * 366 * 24 * 60u64;
        let end_min = candidate_min.saturating_add(max_minutes);
        while candidate_min <= end_min {
            let utc_ms = candidate_min.checked_mul(60_000)?;
            let z = tz.datetime_from_utc_ms(utc_ms)?;
            if (self.months & (1u16 << z.month)) == 0 {
                // Skip ahead to next day at 00:00 in rough steps while staying conservative around DST.
                let rem_hours = 23u64.saturating_sub(u64::from(z.hour));
                let rem_mins = 59u64.saturating_sub(u64::from(z.minute));
                candidate_min += (rem_hours * 60 + rem_mins).max(1);
                continue;
            }
            let dom_ok = (self.days_of_month & (1u32 << z.day)) != 0;
            let dow_ok = (self.days_of_week & (1u8 << z.weekday_sun0)) != 0;
            let day_matches = match (self.dom_restricted, self.dow_restricted) {
                (true, true) => dom_ok || dow_ok,
                (true, false) => dom_ok,
                (false, true) => dow_ok,
                (false, false) => true,
            };
            if !day_matches {
                let rem_mins = 59u64.saturating_sub(u64::from(z.minute));
                candidate_min += (rem_mins + 1).max(1);
                continue;
            }
            if (self.hours & (1u32 << z.hour)) == 0 {
                let rem_mins = 59u64.saturating_sub(u64::from(z.minute));
                candidate_min += (rem_mins + 1).max(1);
                continue;
            }
            if self.matches(&z) {
                return Some(utc_ms);
            }
            candidate_min += 1;
        }
        None
    }

    /// Concise English summary for UI display.
    pub fn describe(&self) -> String {
        let lower = self.raw.trim().to_ascii_lowercase();
        match lower.as_str() {
            "@hourly" | "0 * * * *" => return "Every hour".into(),
            "@daily" | "@midnight" => return "Every day at 00:00".into(),
            "@weekly" => return "Every Sunday at 00:00".into(),
            "@monthly" => return "Monthly on day 1 at 00:00".into(),
            "@yearly" | "@annually" => return "Yearly on Jan 1 at 00:00".into(),
            "* * * * *" => return "Every minute".into(),
            _ => {}
        }
        let parts: Vec<&str> = self.raw.split_whitespace().collect();
        if parts.len() == 5
            && let Some(step) = parts[0].strip_prefix("*/")
            && let Ok(n) = step.parse::<u32>()
            && parts[1] == "*"
            && !self.dom_restricted
            && !self.dow_restricted
            && self.months == ALL_MONTHS
        {
            return format!("Every {n} minutes");
        }
        if parts.len() == 5
            && parts[0] == "0"
            && let Some(step) = parts[1].strip_prefix("*/")
            && let Ok(n) = step.parse::<u32>()
            && !self.dom_restricted
            && !self.dow_restricted
            && self.months == ALL_MONTHS
        {
            return format!("Every {n} hours");
        }
        if self.minutes.count_ones() == 1
            && self.hours.count_ones() == 1
            && self.months == ALL_MONTHS
        {
            let m = self.minutes.trailing_zeros();
            let h = self.hours.trailing_zeros();
            let time = format!("{h:02}:{m:02}");
            if !self.dom_restricted && !self.dow_restricted {
                return format!("Every day at {time}");
            }
            if !self.dom_restricted && self.dow_restricted {
                if self.days_of_week == 0b0011_1110 {
                    return format!("Weekdays at {time}");
                }
                if self.days_of_week == 0b0100_0001 {
                    return format!("Weekends at {time}");
                }
                if self.days_of_week.count_ones() == 1 {
                    let wd = self.days_of_week.trailing_zeros() as usize;
                    let name = [
                        "Sunday",
                        "Monday",
                        "Tuesday",
                        "Wednesday",
                        "Thursday",
                        "Friday",
                        "Saturday",
                    ][wd];
                    return format!("Every {name} at {time}");
                }
            }
            if self.dom_restricted && !self.dow_restricted && self.days_of_month.count_ones() == 1 {
                let d = self.days_of_month.trailing_zeros();
                return format!("Monthly on day {d} at {time}");
            }
        }
        format!("Cron `{}`", self.raw)
    }
}

const ALL_MONTHS: u16 = 0b0001_1111_1111_1110;

const MONTH_NAMES: &[(&str, u8)] = &[
    ("JAN", 1),
    ("FEB", 2),
    ("MAR", 3),
    ("APR", 4),
    ("MAY", 5),
    ("JUN", 6),
    ("JUL", 7),
    ("AUG", 8),
    ("SEP", 9),
    ("OCT", 10),
    ("NOV", 11),
    ("DEC", 12),
];

const DOW_NAMES: &[(&str, u8)] = &[
    ("SUN", 0),
    ("MON", 1),
    ("TUE", 2),
    ("WED", 3),
    ("THU", 4),
    ("FRI", 5),
    ("SAT", 6),
];

fn parse_atom(
    raw: &str,
    min: u8,
    max: u8,
    names: &[(&str, u8)],
    field: &str,
) -> Result<u8, String> {
    let upper = raw.to_ascii_uppercase();
    if let Some((_, val)) = names.iter().find(|(k, _)| *k == upper) {
        return Ok(*val);
    }
    let val: u8 = raw
        .parse()
        .map_err(|_| format!("Invalid {field} value `{raw}`"))?;
    if val < min || val > max {
        return Err(format!(
            "{field} value `{val}` is out of range ({min}..={max})"
        ));
    }
    Ok(val)
}

fn parse_field(
    expr: &str,
    min: u8,
    max: u8,
    names: &[(&str, u8)],
    wrap_sun7: bool,
    field: &str,
) -> Result<(u64, bool), String> {
    if expr == "*" {
        let mut mask = 0u64;
        let effective_max = if wrap_sun7 { 6 } else { max };
        for v in min..=effective_max {
            mask |= 1u64 << v;
        }
        return Ok((mask, false));
    }
    let mut mask = 0u64;
    for part in expr.split(',') {
        if part.is_empty() {
            return Err(format!("Empty item in {field} `{expr}`"));
        }
        let (range_part, step) = if let Some((lhs, rhs)) = part.split_once('/') {
            let s: u8 = rhs
                .parse()
                .map_err(|_| format!("Invalid step `{rhs}` in {field} `{expr}`"))?;
            if s == 0 {
                return Err(format!("Step cannot be 0 in {field} `{expr}`"));
            }
            (lhs, s)
        } else {
            (part, 1u8)
        };

        let (start, end) = if range_part == "*" {
            (min, if wrap_sun7 { 6 } else { max })
        } else if let Some((a, b)) = range_part.split_once('-') {
            let s = parse_atom(a, min, max, names, field)?;
            let e = parse_atom(b, min, max, names, field)?;
            if s > e {
                return Err(format!("Invalid range `{a}-{b}` in {field} `{expr}`"));
            }
            (s, e)
        } else {
            let v = parse_atom(range_part, min, max, names, field)?;
            if part.contains('/') {
                (v, if wrap_sun7 { 6 } else { max })
            } else {
                (v, v)
            }
        };

        let mut cur = start;
        while cur <= end {
            let bit = if wrap_sun7 && cur == 7 { 0 } else { cur };
            mask |= 1u64 << bit;
            match cur.checked_add(step) {
                Some(next) => cur = next,
                None => break,
            }
        }
    }
    if mask == 0 {
        return Err(format!("{field} `{expr}` matched no values"));
    }
    Ok((mask, true))
}

/// Validates a cron expression and optional timezone, returning `(schedule, timezone)`.
pub fn parse_schedule(
    cron: &str,
    timezone: Option<&str>,
) -> Result<(CronSchedule, CronTimeZone), String> {
    let sched = CronSchedule::parse(cron)?;
    let tz = CronTimeZone::parse(timezone)?;
    Ok((sched, tz))
}

/// Human-readable description of a cron expression (falls back to raw string if invalid).
pub fn describe_cron(cron: &str) -> String {
    CronSchedule::parse(cron)
        .map(|s| s.describe())
        .unwrap_or_else(|_| cron.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_macros_steps_lists_and_names() {
        let daily = CronSchedule::parse("0 2 * * *").unwrap();
        assert_eq!(daily.describe(), "Every day at 02:00");

        let weekdays = CronSchedule::parse("30 9 * * MON-FRI").unwrap();
        assert_eq!(weekdays.describe(), "Weekdays at 09:30");

        let step = CronSchedule::parse("*/15 * * * *").unwrap();
        assert_eq!(step.describe(), "Every 15 minutes");

        let sun7 = CronSchedule::parse("0 8 * * 7").unwrap();
        assert_eq!(sun7.describe(), "Every Sunday at 08:00");

        assert!(CronSchedule::parse("60 * * * *").is_err());
        assert!(CronSchedule::parse("* 24 * * *").is_err());
        assert!(CronSchedule::parse("* * 0 * *").is_err());
        assert!(CronSchedule::parse("* * * 13 *").is_err());
        assert!(CronSchedule::parse("* * * * 8").is_err());
        assert!(CronSchedule::parse("0 2 * *").is_err());
    }

    #[test]
    fn evaluates_next_tick_in_utc_and_shanghai_timezone() {
        // 2026-11-15T00:00:00Z
        let base_utc = Utc
            .with_ymd_and_hms(2026, 11, 15, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;

        let sched = CronSchedule::parse("0 2 * * *").unwrap();
        let next_utc = sched.next_after_ms(base_utc, CronTimeZone::Utc).unwrap();
        let expected_utc = Utc
            .with_ymd_and_hms(2026, 11, 15, 2, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(next_utc, expected_utc);

        // In Asia/Shanghai (+08:00), 2026-11-15T00:00:00Z is already 08:00 local,
        // so 02:00 Asia/Shanghai is the NEXT day (2026-11-15T18:00:00Z).
        let shanghai = CronTimeZone::parse(Some("Asia/Shanghai")).unwrap();
        let next_sh = sched.next_after_ms(base_utc, shanghai).unwrap();
        let expected_sh = Utc
            .with_ymd_and_hms(2026, 11, 15, 18, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(next_sh, expected_sh);
    }

    #[test]
    fn posix_or_semantics_when_both_dom_and_dow_are_restricted() {
        // 15th of month OR Monday at 00:00 UTC.
        // 2026-11-15 is a Sunday (matches DOM=15), and 2026-11-16 is Monday (matches DOW=1).
        let sched = CronSchedule::parse("0 0 15 * 1").unwrap();
        let before = Utc
            .with_ymd_and_hms(2026, 11, 14, 23, 59, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let first = sched.next_after_ms(before, CronTimeZone::Utc).unwrap();
        let sun_15 = Utc
            .with_ymd_and_hms(2026, 11, 15, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(first, sun_15);

        let second = sched.next_after_ms(first, CronTimeZone::Utc).unwrap();
        let mon_16 = Utc
            .with_ymd_and_hms(2026, 11, 16, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(second, mon_16);
    }
}
