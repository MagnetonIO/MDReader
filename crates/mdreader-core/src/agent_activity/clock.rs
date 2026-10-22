//! UTC timestamp parsing/formatting for agent logs without a date dependency.

const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn number(text: &str) -> Option<i64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff…](Z|±HH:MM)` into Unix milliseconds.
/// Anything else is `None`; callers fall back to file modification time.
pub fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.len() < 20 || text.len() > 40 || !text.is_ascii() {
        return None;
    }
    let (date, rest) = text.split_at(10);
    let mut parts = date.split('-');
    let (year, month, day) = (
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    );
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let (clock, zone) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => return None,
    };
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut hms = hms.split(':');
    let (hour, minute, second) = (
        number(hms.next()?)?,
        number(hms.next()?)?,
        number(hms.next()?)?,
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || (!fraction.is_empty() && number(fraction).is_none())
    {
        return None;
    }
    let millis = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0i64, |n, b| n * 10 + i64::from(b - b'0'));
    let offset = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (h, m) = zone.get(1..)?.split_once(':')?;
            sign * (number(h)? * 60 + number(m)?) * 60_000
        }
    };
    Some(
        days_from_civil(year, month, day) * DAY_MS
            + ((hour * 60 + minute) * 60 + second) * 1000
            + millis
            - offset,
    )
}

/// `YYYY-MM-DD` (UTC) for display fallbacks.
pub fn utc_date(ms: i64) -> String {
    let (year, month, day) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_log_timestamps_and_formats_dates() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-10-21T12:34:56.789Z"),
            Some(1_792_586_096_789)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-10-21T14:34:56.789+02:00"),
            Some(1_792_586_096_789)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-10-21T12:34:56.7891234Z"),
            Some(1_792_586_096_789)
        );
        assert_eq!(parse_rfc3339_ms("2026-13-21T12:34:56Z"), None);
        assert_eq!(parse_rfc3339_ms("yesterday"), None);
        assert_eq!(utc_date(1_792_586_096_789), "2026-10-21");
        assert_eq!(utc_date(0), "1970-01-01");
    }
}
