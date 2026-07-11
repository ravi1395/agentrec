//! Secret scrubbing (PROTOCOL §9, D11/D31): secret-shape regexes + entropy
//! detection on prompt text, and the secret-file denylist for snapshot
//! withholding. Scrub runs before persistence; there is no bypass switch.

use regex::Regex;
use std::sync::LazyLock;

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(), "aws-key"),
        (
            Regex::new(r"gh[pousr]_[A-Za-z0-9]{20,}").unwrap(),
            "github-token",
        ),
        (
            Regex::new(r"xox[baprs]-[A-Za-z0-9-]{10,}").unwrap(),
            "slack-token",
        ),
        (
            Regex::new(r"sk_(live|test)_[A-Za-z0-9]{16,}").unwrap(),
            "stripe-key",
        ),
        (
            Regex::new(
                r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            )
            .unwrap(),
            "private-key",
        ),
        (
            Regex::new(
                r#"(?i)\b(api[_-]?key|token|secret|password|passwd)\b\s*[=:]\s*(?:'[^']{4,}'|"[^"]{4,}"|[^\s'"]{8,})"#,
            )
            .unwrap(),
            "credential-assignment",
        ),
        // Bare hex-shaped tokens (git SHAs, raw key material) — a dedicated
        // shape rule because narrow-alphabet tokens (hex maxes out at 4
        // bits/char) can dodge the entropy-ratio check below on adversarial
        // (non-random-looking) fixtures.
        (
            Regex::new(r"(?i)\b[0-9a-f]{40,}\b").unwrap(),
            "hex-token",
        ),
    ]
});

const ENTROPY_MIN_LEN: usize = 20;
// Fraction of the inferred alphabet's max entropy a token must hit to be
// flagged. 0.75 * log2(62) ~= 4.47, close to the old flat 4.5 bits/char
// threshold for ordinary mixed alnum tokens (preserves prior behavior there)
// while giving narrow alphabets (hex/decimal) their own reachable ceiling —
// the old flat threshold was mathematically unreachable for hex (max 4.0
// bits/char) and decimal (max 3.32 bits/char) tokens.
const ENTROPY_RATIO: f64 = 0.75;

/// Redact secret shapes and high-entropy tokens; `[redacted:<reason>]` spans.
/// Operates on the whole text for the regex passes, then rebuilds the
/// entropy pass token-by-token using the *original* separators (spaces,
/// tabs, newlines) so callers that persist the scrubbed text (e.g. the
/// prompt blob) keep the source's whitespace structure. `excerpt()` is the
/// only place that intentionally flattens.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    for (pattern, reason) in PATTERNS.iter() {
        out = pattern
            .replace_all(&out, format!("[redacted:{reason}]"))
            .into_owned();
    }
    redact_entropy_preserving_whitespace(&out)
}

/// Excerpt for display: post-scrub, capped at 120 chars (PROTOCOL §5).
pub fn excerpt(text: &str) -> String {
    let clean = scrub(text);
    let flat = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in flat.chars() {
        if out.chars().count() >= 120 {
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

/// Walk `text`, redacting whitespace-delimited tokens that look
/// high-entropy, while pushing every whitespace char through unchanged —
/// preserves newlines/indentation/blank lines exactly.
fn redact_entropy_preserving_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut token_start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_token(&mut result, &text[start..idx]);
            }
            result.push(ch);
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        push_token(&mut result, &text[start..]);
    }
    result
}

fn push_token(result: &mut String, token: &str) {
    if !token.starts_with("[redacted:") && is_high_entropy_token(token) {
        result.push_str("[redacted:high-entropy]");
    } else {
        result.push_str(token);
    }
}

fn is_high_entropy_token(token: &str) -> bool {
    if token.len() < ENTROPY_MIN_LEN {
        return false;
    }
    let threshold = ENTROPY_RATIO * alphabet_size(token).log2();
    shannon_bits_per_char(token) > threshold
}

/// Infer the symbol-set a token is drawn from, most-restrictive first
/// (decimal digits are also valid hex digits, so digit-only must be
/// checked before hex-only). Determines the max-possible-entropy ceiling
/// used to normalize the ratio check — narrow alphabets (hex/decimal) get
/// a reachable ceiling instead of the old one-size-fits-all bits/char cutoff.
fn alphabet_size(token: &str) -> f64 {
    if token.chars().all(|c| c.is_ascii_digit()) {
        10.0
    } else if token.chars().all(|c| c.is_ascii_hexdigit()) {
        16.0
    } else if token.chars().all(|c| c.is_ascii_alphanumeric()) {
        62.0
    } else {
        95.0
    }
}

/// Secret-file denylist (D31): matched files are never snapshotted.
pub fn is_secret_path(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase();
    // Config/data files whose name signals credentials — e.g.
    // `service-account-key.json`, `my-secrets.yaml`, `prod-credentials.toml`.
    // Gated on a structured extension so ordinary source (`keyboard.ts`,
    // `env.rs`) is never withheld.
    let config_ext = [
        ".json", ".yaml", ".yml", ".toml", ".txt", ".ini", ".cfg", ".conf",
    ]
    .iter()
    .any(|e| name.ends_with(e));
    let credential_word =
        name.contains("secret") || name.contains("credential") || name.contains("key");

    name == ".env"
        || name == ".envrc"
        || name.starts_with(".env.")
        || name.ends_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".key")
        || name.ends_with(".keystore")
        || name.starts_with("credentials")
        || name.contains("id_rsa")
        || name.contains("id_ed25519")
        || (config_ext && credential_word)
}

fn shannon_bits_per_char(token: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut len = 0f64;
    for ch in token.chars() {
        *counts.entry(ch).or_insert(0f64) += 1.0;
        len += 1.0;
    }
    if len == 0.0 {
        return 0.0;
    }
    counts
        .values()
        .map(|c| {
            let p = c / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_secret_shapes_redacted() {
        assert!(scrub("key AKIAABCDEFGHIJKLMNOP end").contains("[redacted:aws-key]"));
        assert!(scrub("ghp_abcdefghij0123456789ABCD").contains("[redacted:github-token]"));
        assert!(scrub("api_key = supersecretvalue123").contains("[redacted:credential-assignment]"));
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----";
        assert!(scrub(pem).contains("[redacted:private-key]"));
    }

    #[test]
    fn entropy_catches_random_token_not_prose_or_identifiers() {
        let scrubbed = scrub("deploy with Kj8fPq2xLmNv9rTw4yZsAb7dEu3g now");
        assert!(scrubbed.contains("[redacted:high-entropy]"));
        let code = scrub("call resolve_restore_plan_for_turn please");
        assert!(!code.contains("redacted"));
        let prose = scrub("add rate limiting to the login endpoint");
        assert!(!prose.contains("redacted"));
    }

    #[test]
    fn excerpt_flattens_scrubs_and_caps() {
        let long = format!("fix the bug\nin auth {}", "x".repeat(300));
        let e = excerpt(&long);
        assert!(e.chars().count() <= 120);
        assert!(!e.contains('\n'));
        assert!(e.starts_with("fix the bug in auth"));
    }

    #[test]
    fn secret_paths_matched() {
        for p in [
            ".env",
            "config/.env.production",
            "certs/server.pem",
            "creds/credentials.json",
            "/home/u/.ssh/id_rsa",
            "secrets.yaml",
            "gcp/service-account-key.json",
            "config/prod-credentials.toml",
            "app/my-secret.yml",
        ] {
            assert!(is_secret_path(p), "{p} should be withheld");
        }
        // `key`/`secret` as substrings of ordinary source must NOT withhold.
        for p in [
            "src/main.rs",
            "README.md",
            "env.rs",
            "keyboard.ts",
            "src/keymap.tsx",
            "secret_santa.py",
        ] {
            assert!(!is_secret_path(p), "{p} should not be withheld");
        }
    }

    // B1: entropy pass was blind to hex/decimal — old flat 4.5 bits/char
    // threshold is mathematically unreachable for a hex alphabet (max 4.0)
    // or decimal alphabet (max 3.32), and unreachable for ANY alphabet at
    // token lengths 20-22 (max possible entropy given N unique chars is
    // log2(N), which is <4.5 for N<=22).
    #[test]
    fn entropy_catches_hex_and_decimal_tokens() {
        let sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(
            scrub(&format!("blob {sha256} ok")).contains("[redacted:hex-token]")
                || scrub(&format!("blob {sha256} ok")).contains("[redacted:high-entropy]"),
            "64-char hex token must be redacted"
        );
        let sha1 = "356a192b7913b04c54574d18c28d46e6395428ab";
        assert!(
            scrub(&format!("commit {sha1} done")).contains("[redacted:hex-token]")
                || scrub(&format!("commit {sha1} done")).contains("[redacted:high-entropy]"),
            "40-char hex token must be redacted"
        );
        // Dead-range regression: a structured (non-random-looking) 40-hex
        // token must still be caught by the dedicated shape rule even
        // though its entropy is far below any statistical ratio check.
        let structured_hex = "0123456789abcdef0123456789abcdef01234567";
        assert!(
            scrub(&format!("ref {structured_hex} end")).contains("[redacted:hex-token]"),
            "structured 40-char hex token must be caught by the shape rule regardless of entropy"
        );
        // Decimal-only tokens (finding B1 explicitly calls these out: max
        // entropy for a decimal alphabet is log2(10)=3.32 bits/char, so the
        // old flat 4.5 threshold could never fire). Deliberate tradeoff:
        // this makes any 20+ digit run redactable — innocuous long numbers
        // (unix-ms timestamps <=13 digits, unix-ns <=19, card numbers 16)
        // fall under the ENTROPY_MIN_LEN=20 gate and are unaffected.
        let dec = "48213097561208476531";
        assert!(
            scrub(&format!("id {dec} end")).contains("[redacted:high-entropy]"),
            "high-entropy 20-digit decimal token must be redacted"
        );
    }

    #[test]
    fn entropy_does_not_flag_long_prose_words_paths_or_urls() {
        let path = scrub("open /Users/dev/projects/agentrec/agentrec-core/src/scrub.rs now");
        assert!(
            !path.contains("redacted"),
            "file path must not be redacted: {path}"
        );
        let word = scrub("this uses internationalization and containerization heavily");
        assert!(
            !word.contains("redacted"),
            "long English words must not be redacted: {word}"
        );
        let url = scrub("see https://example.com/docs/getting-started/installation for setup");
        assert!(!url.contains("redacted"), "URL must not be redacted: {url}");
        let identifier =
            scrub("the function resolve_restore_plan_for_turn_and_check_baseline handles this");
        assert!(
            !identifier.contains("redacted"),
            "long identifier must not be redacted: {identifier}"
        );
    }

    // B2: quoted / multi-word secret values used to bypass the
    // credential-assignment regex entirely (unquoted-shape stopped at the
    // first space), leaking every word after the first.
    #[test]
    fn quoted_multiword_secret_fully_redacted() {
        let s = scrub(r#"password = "correct horse battery staple""#);
        assert!(s.contains("[redacted:credential-assignment]"));
        assert!(!s.contains("correct"));
        assert!(!s.contains("horse"));
        assert!(!s.contains("battery"));
        assert!(!s.contains("staple"));
    }

    #[test]
    fn quoted_secret_with_short_leading_word_no_tail_leak() {
        // Old regex's unquoted-shape fallback required 8+ non-space chars,
        // so a quoted value with a short first word plus more text after a
        // space could leak the tail unredacted.
        let s = scrub(r#"token: "ab cdefghijkl""#);
        assert!(s.contains("[redacted:credential-assignment]"));
        assert!(
            !s.contains("cdefghijkl"),
            "tail after the space must not leak: {s}"
        );
    }

    #[test]
    fn single_quoted_secret_also_fully_redacted() {
        let s = scrub("secret = 'top secret phrase here'");
        assert!(s.contains("[redacted:credential-assignment]"));
        assert!(!s.contains("phrase"));
    }

    // B3: scrub() must preserve the original whitespace structure
    // (newlines, indentation, blank lines) of the persisted prompt text;
    // only excerpt() flattens for display.
    #[test]
    fn scrub_preserves_multiline_whitespace_structure() {
        let input = "line one\n  line two indented\n\nblank line above\n\tline four tabbed";
        let scrubbed = scrub(input);
        assert_eq!(
            scrubbed, input,
            "scrub must not alter whitespace when nothing is redacted"
        );
        assert!(scrubbed.contains("\n  line two indented"));
        assert!(scrubbed.contains("\n\nblank line above"));
        assert!(scrubbed.contains("\n\tline four tabbed"));
    }

    #[test]
    fn scrub_redacts_secret_in_multiline_prompt_and_keeps_structure() {
        let input = "please deploy this\napi_key = supersecretvalue123\nthanks for the help";
        let scrubbed = scrub(input);
        assert!(scrubbed.contains("[redacted:credential-assignment]"));
        assert!(!scrubbed.contains("supersecretvalue123"));
        // Structure preserved: 3 lines in, 3 lines out.
        assert_eq!(scrubbed.lines().count(), 3);
        assert!(scrubbed.starts_with("please deploy this\n"));
        assert!(scrubbed.ends_with("\nthanks for the help"));
    }

    #[test]
    fn excerpt_still_flattens_after_scrub_preserves_whitespace() {
        // Regression guard: now that scrub() itself preserves whitespace,
        // excerpt() must independently flatten (its documented job), not
        // rely on scrub() to have already done it.
        let input = "line one\n  line two\n\tline three";
        let e = excerpt(input);
        assert!(!e.contains('\n'));
        assert!(!e.contains('\t'));
        assert_eq!(e, "line one line two line three");
    }

    // B4: `.envrc` and `*.env` (e.g. `local.env`, `dev.env`) were missed by
    // the secret-file denylist.
    #[test]
    fn secret_paths_envrc_and_star_env_matched() {
        for p in [".envrc", "local.env", "dev.env", "config/prod.env"] {
            assert!(is_secret_path(p), "{p} should be withheld");
        }
        // Must not over-match: names that merely contain "env" as a
        // substring or share a suffix fragment are not `.env` files.
        for p in ["environment.md", "src/env.rs", "envision.py"] {
            assert!(!is_secret_path(p), "{p} should not be withheld");
        }
    }
}
