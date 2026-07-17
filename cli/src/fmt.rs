//! Shared accessibility rendering helpers (D42/D43): relative timestamps,
//! terminal color gating, and the `--explain` glossary. Pure functions —
//! callers inject the system clock / TTY / env state so these stay
//! unit-testable without touching real I/O.
//!
//! Also the single home for turn-header rendering (D-PD6): `cmds::format_turn`
//! (used by `log`) and `readcmds::render_turn` (used by `show`/`blame`) used
//! to be two independently-maintained functions whose separators drifted —
//! `log` used double-space, fixed-width columns while `show`/`blame` used
//! ` · ` — and each hand-rolled its own copy of the turn-id truncation.
//! [`turn_list_line`] and [`turn_detail_header`] are now the only two turn
//! renderers, both built on the shared [`SEP`] and [`short_id`].

use agentrec_core::record::TurnRecord;

/// The one separator every turn-rendering call site (`log`, `show`,
/// `blame`) uses between fields. Centralized so a third caller can't
/// reintroduce the drift this module was created to close.
pub const SEP: &str = " · ";

/// `t_<ULID>` → `t_<first4>…<last4>` for display — the id is unambiguous per
/// K+ (prefix-match resolves it against the full stored id), so this is
/// purely a readability truncation. The single canonical definition for
/// `TurnRecord` ids (memory-record ids use their own, differently-shaped
/// `memorycmds::short_id` — not a turn id, deliberately not unified here).
pub fn short_id(id: &str) -> String {
    let body = id.strip_prefix("t_").unwrap_or(id);
    if body.len() <= 8 {
        return id.to_string();
    }
    format!("t_{}…{}", &body[..4], &body[body.len() - 4..])
}

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

/// Strip terminal control characters — C0 controls `0x00`-`0x1F`, DEL
/// `0x7F`, and C1 controls `U+0080`-`U+009F` — from text about to be
/// printed to a real terminal (E7): a prompt excerpt is user-authored text
/// that reaches stdout verbatim, so an embedded escape sequence (e.g. an
/// OSC "set terminal title" or a cursor move) must never survive to the
/// terminal. C1 is included because some terminals honor its single-byte
/// forms as escape introducers in their own right — CSI (U+009B) and OSC
/// (U+009D) chief among them — not just the ESC-prefixed 7-bit equivalents
/// C0 already covers. Printable text — including non-ASCII UTF-8 outside
/// the C1 range — passes through unchanged; this is display-only and never
/// touches what's persisted (the scrub/excerpt pipeline in
/// `agentrec_core::scrub` already ran before this text ever reaches here).
pub fn sanitize_terminal(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let cp = *c as u32;
            cp >= 0x20 && cp != 0x7f && !(0x80..=0x9f).contains(&cp)
        })
        .collect()
}

/// `log`'s compact multi-row turn line:
/// `<id> · <grade> · <tool> · <when> · <files>[ · "<prompt excerpt>"][ · (truncated)]`.
///
/// The grade (`rich`/`bare`) is always rendered as its own literal field —
/// never folded into a "bare turn" phrase like [`turn_detail_header`] is —
/// because `log --explain`'s glossary scan (D43, `glossary_for`) matches on
/// the literal word "rich"/"bare" appearing in this invocation's rendered
/// output; folding it away would silently break that AC. `when` and `files`
/// are caller-computed (relative-vs-UTC time (D43), file count) so this stays
/// a pure formatter. `id_color` gates ANSI on the id only (D42) — `log` is
/// the only caller that has ever colorized turn output.
pub fn turn_list_line(t: &TurnRecord, when: &str, files: &str, id_color: bool) -> String {
    let id = paint(&short_id(&t.id), "36", id_color);
    let tool = t.tool.as_deref().unwrap_or("—");
    let mut line = format!("{id}{SEP}{}{SEP}{tool}{SEP}{when}{SEP}{files}", t.grade);
    if let Some(excerpt) = t.prompt_excerpt.as_deref() {
        line.push_str(&format!("{SEP}\"{}\"", sanitize_terminal(excerpt)));
    }
    if t.truncated {
        line.push_str(&format!("{SEP}(truncated)"));
    }
    line
}

/// `show`'s bare header and `blame`'s per-file/per-line result line:
/// `<id> · <tool> · "<prompt excerpt>" · <when>` for a rich turn, or
/// `<id> · bare turn · <when>` for a bare one — a bare turn never fabricates
/// a tool or prompt (AC G3). Callers append further clauses (e.g. `blame`'s
/// " · deleted this file" / " · human-edited since") using the same [`SEP`].
pub fn turn_detail_header(t: &TurnRecord, when: &str) -> String {
    let id = short_id(&t.id);
    if t.grade == "rich" {
        let tool = t.tool.as_deref().unwrap_or("—");
        let prompt = t
            .prompt_excerpt
            .as_deref()
            .map(sanitize_terminal)
            .unwrap_or_else(|| "—".to_string());
        format!("{id}{SEP}{tool}{SEP}\"{prompt}\"{SEP}{when}")
    } else {
        format!("{id}{SEP}bare turn{SEP}{when}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // C1 controls (U+0080-U+009F) — CSI (U+009B) and OSC (U+009D) among
    // them — must never survive sanitize_terminal either: some terminals
    // honor them as escape introducers just like their C0/ESC-prefixed
    // equivalents. RED before the fix: the pre-fix filter only excluded C0
    // (0x00-0x1F) + DEL (0x7F), so both codepoints pass through unchanged.
    #[test]
    fn sanitize_terminal_strips_c1_controls_keeps_text() {
        let evil = "hello \u{9b}31m \u{9d}evil\u{9c} world";
        let clean = sanitize_terminal(evil);
        assert!(!clean.contains('\u{9b}'));
        assert!(!clean.contains('\u{9d}'));
        assert!(!clean.contains('\u{9c}'));
        assert_eq!(clean, "hello 31m evil world");
    }

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

    // E7: an ESC/BEL-laden prompt (e.g. an OSC "set terminal title" payload)
    // must never survive sanitize_terminal — the plain text around it does.
    #[test]
    fn sanitize_terminal_strips_c0_and_del_keeps_text() {
        let evil = "hello \x1b]0;evil\x07 world\x7f!";
        let clean = sanitize_terminal(evil);
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\x07'));
        assert!(!clean.contains('\x7f'));
        assert_eq!(clean, "hello ]0;evil world!");
    }

    #[test]
    fn sanitize_terminal_noop_on_plain_text() {
        assert_eq!(sanitize_terminal("write g.rs — done"), "write g.rs — done");
    }

    fn turn(id: &str, grade: &str, tool: Option<&str>, excerpt: Option<&str>) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: grade.to_string(),
            truncated: false,
            started: "2026-01-01T00:00:00.000Z".into(),
            ended: "2026-01-01T00:00:01.000Z".into(),
            tool: tool.map(String::from),
            model: None,
            session: None,
            root: "/repo".into(),
            prompt_ref: None,
            prompt_excerpt: excerpt.map(String::from),
            merges: vec![],
            files: vec![],
        }
    }

    // D-PD6: pins `turn_list_line`'s exact shape — the single separator, the
    // literal grade word (needed for `--explain`'s glossary scan), and the
    // optional excerpt/truncated suffixes.
    #[test]
    fn turn_list_line_rich_with_excerpt_and_truncated() {
        let mut t = turn(
            "t_ABCD00000000000000EFGH",
            "rich",
            Some("claude"),
            Some("do the thing"),
        );
        t.truncated = true;
        let line = turn_list_line(&t, "3m ago", "2 files", false);
        assert_eq!(
            line,
            "t_ABCD…EFGH · rich · claude · 3m ago · 2 files · \"do the thing\" · (truncated)"
        );
    }

    #[test]
    fn turn_list_line_bare_no_tool_no_excerpt() {
        let t = turn("t_ABCD00000000000000EFGH", "bare", None, None);
        let line = turn_list_line(&t, "just now", "1 file", false);
        assert_eq!(line, "t_ABCD…EFGH · bare · — · just now · 1 file");
    }

    #[test]
    fn turn_list_line_colorizes_only_the_id() {
        let t = turn("t_ABCD00000000000000EFGH", "rich", Some("claude"), None);
        let line = turn_list_line(&t, "3m ago", "1 file", true);
        assert!(line.starts_with("\x1b[36mt_ABCD…EFGH\x1b[0m"));
        assert!(!line[line.find("rich").unwrap()..].contains('\x1b'));
    }

    // D-PD6: pins `turn_detail_header`'s exact shape — same separator as
    // `turn_list_line`, but grade folded into "bare turn" for bare turns
    // (AC G3: never fabricate a tool or quoted prompt for one) instead of a
    // literal grade field.
    #[test]
    fn turn_detail_header_rich_quotes_excerpt() {
        let t = turn(
            "t_ABCD00000000000000EFGH",
            "rich",
            Some("claude"),
            Some("do the thing"),
        );
        let line = turn_detail_header(&t, "14:03");
        assert_eq!(line, "t_ABCD…EFGH · claude · \"do the thing\" · 14:03");
    }

    #[test]
    fn turn_detail_header_bare_never_fabricates() {
        let t = turn("t_ABCD00000000000000EFGH", "bare", None, None);
        let line = turn_detail_header(&t, "14:03");
        assert_eq!(line, "t_ABCD…EFGH · bare turn · 14:03");
        assert!(!line.contains('"'));
    }

    // Both renderers must agree on the id truncation and the separator for
    // the same turn — the RED/GREEN contract this module exists to enforce
    // (D-PD6), mirrored at the unit level alongside the integration-level
    // `log_and_show_render_turn_header_with_identical_formatting`.
    #[test]
    fn list_and_detail_renderers_share_id_format_and_separator() {
        let t = turn("t_ABCD00000000000000EFGH", "rich", Some("claude"), None);
        let list = turn_list_line(&t, "3m ago", "1 file", false);
        let detail = turn_detail_header(&t, "14:03");
        assert!(list.starts_with("t_ABCD…EFGH · "));
        assert!(detail.starts_with("t_ABCD…EFGH · "));
    }
}
