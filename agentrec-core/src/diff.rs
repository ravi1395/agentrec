//! Text diffing for the `diff` verb (PROTOCOL-adjacent CLI concern, AC F1–F4).
//! Binary detection gates whether a unified diff is attempted at all.

use similar::{ChangeTag, TextDiff};

/// True when `bytes` look binary: a NUL in the first 8 KiB, or invalid UTF-8.
/// Either condition makes a line-oriented unified diff meaningless.
pub fn is_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        return true;
    }
    std::str::from_utf8(bytes).is_err()
}

/// Unified diff of `old` → `new`, headered as `--- a/<path>` / `+++ b/<path>`.
/// Empty `old` or `new` naturally renders as a full add/remove.
pub fn unified(old: &str, new: &str, path: &str) -> String {
    let a = format!("a/{path}");
    let b = format!("b/{path}");
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&a, &b)
        .to_string()
}

/// New-side lines whose change is an `Insert` — covers both pure additions
/// and the new side of a replaced line. Used by `blame` (AC G5) to decide
/// which turn introduced a given line of text; trailing `\n` is trimmed per
/// line so callers can compare directly against `str::lines()` output.
pub fn added_or_changed_lines(old: &str, new: &str) -> Vec<String> {
    TextDiff::from_lines(old, new)
        .iter_all_changes()
        .filter(|change| change.tag() == ChangeTag::Insert)
        .map(|change| change.value().trim_end_matches('\n').to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modify_shows_add_and_remove_lines() {
        let out = unified("a\nb\nc\n", "a\nB\nc\n", "src/x.rs");
        assert!(out.contains("--- a/src/x.rs"));
        assert!(out.contains("+++ b/src/x.rs"));
        assert!(out.lines().any(|l| l == "-b"));
        assert!(out.lines().any(|l| l == "+B"));
    }

    #[test]
    fn identical_input_yields_no_hunks() {
        let out = unified("same\ntext\n", "same\ntext\n", "src/x.rs");
        assert!(!out.contains("@@"));
    }

    #[test]
    fn full_add_from_empty_old() {
        let out = unified("", "line one\nline two\n", "src/new.rs");
        assert!(out.lines().any(|l| l == "+line one"));
        assert!(out.lines().any(|l| l == "+line two"));
    }

    #[test]
    fn is_binary_true_for_nul_byte() {
        assert!(is_binary(b"abc\0def"));
    }

    #[test]
    fn is_binary_true_for_invalid_utf8() {
        // No NUL byte here — this must trip the UTF-8 check specifically.
        assert!(is_binary(&[0xff, 0xfe, 0xfd]));
    }

    #[test]
    fn is_binary_false_for_ordinary_text() {
        assert!(!is_binary(b"fn main() {\n    println!(\"hi\");\n}\n"));
    }

    #[test]
    fn added_or_changed_lines_covers_pure_add_and_replace() {
        let lines = added_or_changed_lines("a\nb\n", "a\nB2\nc\n");
        // "b" -> "B2" is a replace (Delete "b" + Insert "B2"); "c" is a pure add.
        assert_eq!(lines, vec!["B2".to_string(), "c".to_string()]);
    }

    #[test]
    fn added_or_changed_lines_empty_when_unchanged() {
        assert!(added_or_changed_lines("same\ntext\n", "same\ntext\n").is_empty());
    }
}
