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
                r#"(?i)\b(api[_-]?key|token|secret|password|passwd)\b\s*[=:]\s*['"]?[^\s'"]{8,}"#,
            )
            .unwrap(),
            "credential-assignment",
        ),
    ]
});

const ENTROPY_MIN_LEN: usize = 20;
const ENTROPY_BITS_PER_CHAR: f64 = 4.5;

/// Redact secret shapes and high-entropy tokens; `[redacted:<reason>]` spans.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    for (pattern, reason) in PATTERNS.iter() {
        out = pattern
            .replace_all(&out, format!("[redacted:{reason}]"))
            .into_owned();
    }
    out.split_whitespace()
        .map(|token| {
            if token.len() >= ENTROPY_MIN_LEN
                && !token.starts_with("[redacted:")
                && shannon_bits_per_char(token) > ENTROPY_BITS_PER_CHAR
            {
                "[redacted:high-entropy]".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Excerpt for display: post-scrub, capped at 120 chars (PROTOCOL §5).
pub fn excerpt(text: &str) -> String {
    let clean = scrub(text);
    let flat = clean.replace(['\n', '\r'], " ");
    let mut out = String::new();
    for ch in flat.chars() {
        if out.chars().count() >= 120 {
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
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
        || name.starts_with(".env.")
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
}
