//! Reentrant local timezone helpers safe for multi-threaded Tokio runtimes.

use std::time::{SystemTime, UNIX_EPOCH};

use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDateTimeParts {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub weekday_from_monday: u8,
    pub utc_offset_secs: i32,
}

#[cfg(unix)]
unsafe extern "C" {
    fn tzset();
}

#[cfg(unix)]
fn ensure_tz_initialized() {
    static TZ_INIT: std::sync::Once = std::sync::Once::new();
    TZ_INIT.call_once(|| unsafe {
        tzset();
    });
}

#[cfg(unix)]
pub fn unix_epoch_to_local_parts(epoch_secs: i64) -> Option<LocalDateTimeParts> {
    ensure_tz_initialized();
    let time_val = libc::time_t::try_from(epoch_secs).ok()?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ptr = unsafe { libc::localtime_r(&time_val, &mut tm) };
    if ptr.is_null() {
        return None;
    }
    let weekday_from_monday = if tm.tm_wday == 0 {
        7
    } else if (1..=6).contains(&tm.tm_wday) {
        tm.tm_wday as u8
    } else {
        return None;
    };
    let utc_offset_secs = i32::try_from(tm.tm_gmtoff).ok()?;
    Some(LocalDateTimeParts {
        year: tm.tm_year + 1900,
        month: u8::try_from(tm.tm_mon + 1).ok()?,
        day: u8::try_from(tm.tm_mday).ok()?,
        hour: u8::try_from(tm.tm_hour).ok()?,
        minute: u8::try_from(tm.tm_min).ok()?,
        second: u8::try_from(tm.tm_sec).ok()?,
        weekday_from_monday,
        utc_offset_secs,
    })
}

#[cfg(not(unix))]
pub fn unix_epoch_to_local_parts(epoch_secs: i64) -> Option<LocalDateTimeParts> {
    let utc = OffsetDateTime::from_unix_timestamp(epoch_secs).ok()?;
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let local = utc.to_offset(offset);
    Some(LocalDateTimeParts {
        year: local.year(),
        month: u8::from(local.month()),
        day: local.day(),
        hour: local.hour(),
        minute: local.minute(),
        second: local.second(),
        weekday_from_monday: local.weekday().number_from_monday(),
        utc_offset_secs: offset.whole_seconds(),
    })
}

pub fn now_local_parts() -> Option<LocalDateTimeParts> {
    let epoch_secs =
        i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    unix_epoch_to_local_parts(epoch_secs)
}

#[cfg(unix)]
pub fn now_local_offset_datetime() -> OffsetDateTime {
    let utc = OffsetDateTime::from(SystemTime::now());
    if let Some(parts) = unix_epoch_to_local_parts(utc.unix_timestamp())
        && let Ok(offset) = UtcOffset::from_whole_seconds(parts.utc_offset_secs)
    {
        return utc.to_offset(offset);
    }
    utc
}

#[cfg(not(unix))]
pub fn now_local_offset_datetime() -> OffsetDateTime {
    let utc = OffsetDateTime::from(SystemTime::now());
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    utc.to_offset(offset)
}

#[cfg(unix)]
pub fn local_calendar_to_epoch_secs(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> Option<i64> {
    let month_enum = Month::try_from(month).ok()?;
    let _ = Date::from_calendar_date(year, month_enum, day).ok()?;
    let _ = Time::from_hms(hour, minute, second).ok()?;
    ensure_tz_initialized();
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year.checked_sub(1900)?;
    tm.tm_mon = i32::from(month) - 1;
    tm.tm_mday = i32::from(day);
    tm.tm_hour = i32::from(hour);
    tm.tm_min = i32::from(minute);
    tm.tm_sec = i32::from(second);
    tm.tm_isdst = -1;
    tm.tm_wday = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    if epoch == -1 && tm.tm_wday < 0 {
        return None;
    }
    #[allow(clippy::useless_conversion)]
    Some(i64::from(epoch))
}

#[cfg(not(unix))]
pub fn local_calendar_to_epoch_secs(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> Option<i64> {
    let month_enum = Month::try_from(month).ok()?;
    let date = Date::from_calendar_date(year, month_enum, day).ok()?;
    let clock = Time::from_hms(hour, minute, second).ok()?;
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    Some(date.with_time(clock).assume_offset(offset).unix_timestamp())
}

pub fn format_local_datetime(epoch_secs: i64) -> String {
    let Some(parts) = unix_epoch_to_local_parts(epoch_secs) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_local_calendar_and_epoch() {
        let epoch = local_calendar_to_epoch_secs(2026, 7, 19, 14, 45, 12).unwrap();
        let parts = unix_epoch_to_local_parts(epoch).unwrap();
        assert_eq!(
            (
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second
            ),
            (2026, 7, 19, 14, 45, 12)
        );
        assert_eq!(format_local_datetime(epoch), "2026-07-19 14:45:12");
    }
}
