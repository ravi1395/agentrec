//! Shared accessibility rendering helpers (D42/D43): relative timestamps,
//! terminal color gating, and the `--explain` glossary. Pure functions —
//! callers inject the system clock / TTY / env state so these stay
//! unit-testable without touching real I/O.

/// Render `then_rfc3339` (a `TurnRecord.started`/`.ended` RFC 3339 UTC
/// timestamp) relative to `now_unix_ms`: "just now" (<60s), "Nm ago" (<1h),
/// "Nh ago" (<24h), "yesterday" (24h–48h), "Nd ago" (2d–7d), and an absolute
/// `YYYY-MM-DD` date beyond that. `now` is injected rather than read from the
/// system clock here, so this is golden-testable. Falls back to the raw
/// string on an unparseable timestamp rather than panicking.
pub fn relative_time(then_rfc3339: &str, now_unix_ms: u64) -> String {
    let Some(then_ms) = parse_rfc3339_ms(then_rfc3339) else {
        return then_rfc3339.to_string();
    };
    if then_ms >= now_unix_ms {
        return "just now".to_string(); // clock skew / future timestamp guard
    }
    let secs = (now_unix_ms - then_ms) / 1000;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else if secs < 2 * 86_400 {
        "yesterday".to_string()
    } else if secs < 7 * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else {
        then_rfc3339
            .split('T')
            .next()
            .unwrap_or(then_rfc3339)
            .to_string()
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS(.fff)?Z` into unix milliseconds. Tolerates a
/// missing fractional-seconds component (test fixtures often omit it).
/// `None` on any malformed input — the caller falls back to the raw string.
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;

    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let mut tp = hms.splitn(3, ':');
    let h: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next()?.parse().ok()?;
    let se: i64 = tp.next()?.parse().ok()?;

    let millis: u64 = match frac {
        Some(f) => {
            let mut digits: String = f.chars().take(3).collect();
            while digits.len() < 3 {
                digits.push('0');
            }
            digits.parse().ok()?
        }
        None => 0,
    };

    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + se;
    if secs < 0 {
        return None;
    }
    Some(secs as u64 * 1000 + millis)
}

/// Inverse of the `civil_from_days` algorithm in `agentrec_core::time`
/// (Howard Hinnant's days-from-civil); same epoch constants (719468, 146097)
/// so the two stay consistent by construction.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Whether color escape codes should be emitted at all: only on a real TTY,
/// and never under `NO_COLOR` (https://no-color.org). Pure so it's
/// unit-testable independent of the real terminal/env — callers pass
/// `std::io::stdout().is_terminal()` and `std::env::var_os("NO_COLOR").is_some()`.
pub fn should_color(stdout_is_tty: bool, no_color_env: bool) -> bool {
    stdout_is_tty && !no_color_env
}

/// Wrap `text` in an ANSI SGR color code when `enabled`; returns `text`
/// unchanged (no escape bytes at all) when not — piped/NO_COLOR output must
/// never contain `\x1b[`.
pub fn paint(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// One glossary entry: (term, one-line definition). Order is the canonical
/// scan order — `glossary_for` preserves it in its output.
const GLOSSARY: &[(&str, &str)] = &[
    (
        "rich",
        "rich: a turn attributed to a specific tool/model/prompt via an agent hook signal",
    ),
    (
        "bare",
        "bare: an unattributed activity window inferred from file changes alone — never assumed to be agent activity",
    ),
    (
        "git",
        "git turn: a mutation burst coinciding with a git operation (checkout, merge, ...); hidden from `log` unless --all",
    ),
    (
        "truncated",
        "truncated: the turn's start signal had no matching stop — closed at the last observed mutation",
    ),
    (
        "human-edited since",
        "human-edited since: the file changed after this turn ended, outside any rich turn's coverage",
    ),
];

/// Scan `rendered` (the text this invocation of `log` actually printed) for
/// the domain terms in [`GLOSSARY`], returning only the entries that appear —
/// D43: a listing with no bare turns must not explain "bare", and vice versa.
/// Single words are matched whole (so "digit" doesn't trigger "git"); the one
/// multi-word phrase is matched as a literal substring.
pub(crate) fn glossary_for(rendered: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (term, definition) in GLOSSARY {
        let hit = if term.contains(' ') {
            rendered.contains(term)
        } else {
            contains_word(rendered, term)
        };
        if hit {
            out.push(*definition);
        }
    }
    out
}

/// True when `word` appears in `text` as a standalone alphanumeric token
/// (split on any non-alphanumeric byte), case-insensitively.
fn contains_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|tok| tok.eq_ignore_ascii_case(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC-Z+2 golden tests: exact strings for each relative-time bucket.
    #[test]
    fn relative_time_golden_buckets() {
        let now: u64 = 2_000_000_000_000; // arbitrary fixed "now"
        let then = |delta_ms: u64| {
            let ms = now - delta_ms;
            agentrec_core::time::rfc3339(ms)
        };
        assert_eq!(relative_time(&then(10_000), now), "just now");
        assert_eq!(relative_time(&then(3 * 60_000), now), "3m ago");
        assert_eq!(relative_time(&then(2 * 3_600_000), now), "2h ago");
        assert_eq!(relative_time(&then(26 * 3_600_000), now), "yesterday");
        assert_eq!(relative_time(&then(3 * 86_400_000), now), "3d ago");
        let ten_days = then(10 * 86_400_000);
        let expected_date = ten_days.split('T').next().unwrap().to_string();
        assert_eq!(relative_time(&ten_days, now), expected_date);
    }

    #[test]
    fn relative_time_falls_back_on_garbage() {
        assert_eq!(relative_time("not-a-date", 12345), "not-a-date");
    }

    // AC-Z+3 truth table.
    #[test]
    fn should_color_truth_table() {
        assert!(should_color(true, false));
        assert!(!should_color(true, true));
        assert!(!should_color(false, false));
        assert!(!should_color(false, true));
    }

    #[test]
    fn paint_no_escape_when_disabled() {
        assert_eq!(paint("t_ABCD", "36", false), "t_ABCD");
        assert!(!paint("t_ABCD", "36", false).contains('\x1b'));
    }

    #[test]
    fn paint_wraps_when_enabled() {
        let out = paint("t_ABCD", "36", true);
        assert!(out.contains('\x1b'));
        assert!(out.contains("t_ABCD"));
    }

    // AC-Z+4: rich-only listing never explains "bare"; a bare turn present
    // must explain it. Also checks "git" isn't matched inside "digit".
    #[test]
    fn glossary_rich_only_excludes_bare() {
        let rendered = "t_AAAA  rich   claude   3m ago  1 file\n";
        let entries = glossary_for(rendered);
        assert!(entries.iter().any(|d| d.starts_with("rich:")));
        assert!(!entries.iter().any(|d| d.starts_with("bare:")));
    }

    #[test]
    fn glossary_includes_bare_when_present() {
        let rendered = "t_AAAA  bare   —   3m ago  1 file\n";
        let entries = glossary_for(rendered);
        assert!(entries.iter().any(|d| d.starts_with("bare:")));
    }

    #[test]
    fn glossary_git_word_boundary_ignores_substring() {
        assert!(glossary_for("a digit changed").is_empty());
        assert!(!glossary_for("tool git 1 file").is_empty());
    }
}
