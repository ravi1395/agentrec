//! Unix-ms ↔ RFC 3339 (UTC) conversion without a chrono dependency.
//! Uses the standard days-from-civil / civil-from-days algorithms.

/// Format unix milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn rfc3339(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    let (h, mi, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        // 2026-07-05T00:00:00Z = 1783209600 secs
        assert_eq!(rfc3339(1_783_209_600_000), "2026-07-05T00:00:00.000Z");
        // millisecond component survives
        assert_eq!(rfc3339(1_500), "1970-01-01T00:00:01.500Z");
    }

    #[test]
    fn leap_day() {
        // 2024-02-29T12:00:00Z = 1709208000 secs
        assert_eq!(rfc3339(1_709_208_000_000), "2024-02-29T12:00:00.000Z");
    }
}
