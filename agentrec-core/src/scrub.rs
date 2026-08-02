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
        // F6 (red team round 2): the key-name side used to be `\b(...)\b`,
        // which does NOT fire inside an underscore-joined name — `_` is a
        // word character, so `\bsecret\b` never matches within
        // `aws_secret_access_key`. Measured before changing it, not assumed:
        // `scrub("aws_secret_access_key = <40 chars>")` came back byte-for-byte
        // unredacted, as did the `AWS_SECRET_ACCESS_KEY=<40 chars>` env form.
        // The word boundaries are therefore replaced by bounded runs of
        // identifier punctuation on either side, so the credential word may sit
        // anywhere inside the key name. `passphrase`/`credential` added at the
        // same time; the pre-existing words are unchanged.
        //
        // Accepted over-redaction, stated because it is a real widening: a
        // prose colon form whose value is 8+ non-space chars — `the token:
        // abcdefghij` — now redacts. This function only ever runs on prompt
        // text and memory facts, so the cost is display fidelity, never
        // recovery; the safe direction.
        (
            Regex::new(
                r#"(?i)[A-Za-z0-9_.\-]{0,40}(api[_-]?key|token|secret|password|passwd|passphrase|credential)[A-Za-z0-9_.\-]{0,40}\s*[=:]\s*(?:'[^']{4,}'|"[^"]{4,}"|[^\s'"]{8,})"#,
            )
            .unwrap(),
            "credential-assignment",
        ),
        // F6: credentials embedded in a URL authority —
        // `scheme://user:password@host/...`. Both a `:` and an `@` must appear
        // between `://` and the first `/` or whitespace, which is what keeps a
        // password-less `postgres://db.internal:5432/prod` (host:port, no `@`)
        // and a user-only `postgres://admin@db.internal/prod` (no `:` before
        // the `@`) out. The whole URL is replaced, not just the password: the
        // host and account name are part of what leaks.
        (
            Regex::new(r"(?i)\b[a-z][a-z0-9+.\-]*://[^\s:/@]+:[^\s/@]+@[^\s]*").unwrap(),
            "url-credential",
        ),
        // F6: OpenAI-style keys — `sk-` plus 20+ token chars, covering both the
        // legacy flat form and the long `sk-proj-` form. Distinct from the
        // Stripe rule above, which keys on `sk_` with an underscore. The `\b`
        // is load-bearing: without it this fires inside ordinary kebab-case
        // prose such as `risk-management-framework-review`, where the `sk-` is
        // preceded by a word character and the boundary correctly fails.
        (
            Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}").unwrap(),
            "openai-key",
        ),
        // F6: `Authorization: Bearer <token>` / `Basic <base64>` headers. The
        // 20-char floor is what separates a credential from prose — `Bearer
        // token`, `Bearer <token>` and `basic authentication` all fall under
        // it. A 20+ char word directly after the scheme name is still
        // redactable prose in principle; accepted for the same reason as
        // above (prompt text only, safe direction).
        (
            Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{20,}").unwrap(),
            "bearer-token",
        ),
        // Bare hex-shaped tokens (git SHAs, raw key material) — a dedicated
        // shape rule because narrow-alphabet tokens (hex maxes out at 4
        // bits/char) can dodge the entropy-ratio check below on adversarial
        // (non-random-looking) fixtures.
        (
            Regex::new(r"(?i)\b[0-9a-f]{40,}\b").unwrap(),
            "hex-token",
        ),
        // NOT EXHAUSTIVE — one more shape rule lives outside this table:
        // `is_aws_secret_shape`, the bare 40-char AWS *secret access key*
        // (`[redacted:aws-secret-key]`), is applied per whitespace-delimited
        // token in `push_token` rather than as a regex here. It cannot be a
        // regex over the whole text without lookaround: the character class an
        // AWS secret needs (`[A-Za-z0-9/+=]`, slashes included) matches runs
        // *inside* ordinary paths — `Users/dev/projects/agentrec/agentrec-core`
        // is a 41-char run of exactly those characters — so an unanchored
        // `{40}` rule would redact file paths. Tokenizing first gives the
        // anchoring for free.
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
    if token.starts_with("[redacted:") {
        result.push_str(token);
        return;
    }
    // F6: bare AWS secret access key, checked before the entropy fallback
    // because entropy demonstrably does not catch it (see
    // `is_aws_secret_shape`). Surrounding punctuation is split off and
    // re-emitted so a secret at the end of a sentence still loses only the
    // secret.
    let (lead, core, tail) = split_edge_punctuation(token);
    if is_aws_secret_shape(core) {
        result.push_str(lead);
        result.push_str("[redacted:aws-secret-key]");
        result.push_str(tail);
        return;
    }
    if is_high_entropy_token(token) {
        result.push_str("[redacted:high-entropy]");
    } else {
        result.push_str(token);
    }
}

/// Split a token into (leading punctuation, core, trailing punctuation) so a
/// shape rule can be applied to the core alone. Only the characters that
/// realistically wrap a pasted credential in prose or code are peeled; `/`,
/// `+` and `=` are deliberately NOT in the set because they are part of the
/// AWS secret alphabet itself.
fn split_edge_punctuation(token: &str) -> (&str, &str, &str) {
    const EDGE: &[char] = &[
        '.', ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '\'', '"',
    ];
    let core = token.trim_matches(EDGE);
    let start = token.len() - token.trim_start_matches(EDGE).len();
    (&token[..start], core, &token[start + core.len()..])
}

/// A bare AWS secret access key: exactly 40 characters from AWS's
/// `[A-Za-z0-9/+=]` secret alphabet, carrying at least one upper, one lower
/// and one digit.
///
/// This exists because the entropy fallback provably does not cover it, and
/// the reason is distribution, not length. Measured on this file's own
/// helpers before the rule was written, at the same 40-char length:
///   * an all-distinct 40-char token scores 5.322 bits/char against a 4.466
///     threshold -> `is_high_entropy_token` = true;
///   * a 40-char token with a realistic repeat distribution scores 2.546 ->
///     false;
///   * the same with a `/` in it moves the alphabet ceiling to 95 and the
///     threshold to 4.927 while scoring 2.732 -> false.
///
/// Real credentials sit in the second and third rows, so the entropy gate
/// catches only the near-maximal-entropy tail. The exact-40 length is what
/// makes the check safe to apply to bare tokens: file paths, base64 blobs and
/// prose words are essentially never exactly 40 characters of that alphabet.
/// A git SHA is 40 chars but lowercase-only (no upper, no mixed case) and is
/// already redacted by the `hex-token` regex before tokenization.
fn is_aws_secret_shape(token: &str) -> bool {
    const AWS_SECRET_LEN: usize = 40;
    if token.len() != AWS_SECRET_LEN || token.chars().count() != AWS_SECRET_LEN {
        return false;
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '+' || c == '=')
    {
        return false;
    }
    token.chars().any(|c| c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_lowercase())
        && token.chars().any(|c| c.is_ascii_digit())
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

/// Directory components that make every file beneath them a secret (F7). The
/// pre-F7 matcher looked at the basename only, so `secrets/prod.yaml` — where
/// the credential word is in the *directory* and the basename `prod.yaml`
/// matches nothing — was snapshotted in full.
///
/// Deliberately NOT in this list, because withholding is not free (a withheld
/// file is never snapshotted, therefore never undoable — see the D-register's
/// `withheld` semantics):
///   * `credentials` / `.credentials` — a repo with `src/credentials/` holding
///     *auth code* would have that source silently un-snapshotted. It stays a
///     basename prefix rule only, which is where it already was.
///   * `key` / `keys` — collides with `src/keys/keymap.ts`, i18n key tables.
///   * `config` / `.config` — far too broad to be a credential signal.
const SECRET_DIR_COMPONENTS: &[&str] = &[
    ".aws", ".ssh", ".gnupg", ".kube", ".docker", "secrets", ".secrets",
];

/// Exact basenames that are credentials regardless of extension (F7). Every
/// entry here was reported by the red team as accepted (i.e. snapshotted in
/// full) by the pre-F7 matcher.
const SECRET_BASENAMES: &[&str] = &[
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".pgpass",
    ".npmrc",
    ".pypirc",
    ".htpasswd",
    ".dockercfg",
    "kubeconfig",
    "terraform.tfstate",
    "terraform.tfstate.backup",
];

/// Secret-file denylist (D31): matched files are never snapshotted.
///
/// Matches on the basename AND on the intermediate path components — callers
/// pass either a root-relative path (`daemon.rs`, `memory.rs`) or an absolute
/// one (`importcmd.rs` hands over the transcript's raw `filePath`), and both
/// forms carry their directories, so component matching works on both.
pub fn is_secret_path(path: &str) -> bool {
    let components: Vec<String> = path
        .split(['/', '\\'])
        .map(|c| c.to_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    let name = components.last().cloned().unwrap_or_default();
    if components
        .iter()
        .rev()
        .skip(1)
        .any(|c| SECRET_DIR_COMPONENTS.contains(&c.as_str()))
    {
        return true;
    }
    if SECRET_BASENAMES.contains(&name.as_str()) {
        return true;
    }
    // Config/data files whose name signals credentials — e.g.
    // `service-account-key.json`, `my-secrets.yaml`, `prod-credentials.toml`.
    // Gated on a structured extension so ordinary source (`keyboard.ts`,
    // `env.rs`) is never withheld.
    let config_ext = [
        ".json", ".yaml", ".yml", ".toml", ".txt", ".ini", ".cfg", ".conf",
    ]
    .iter()
    .any(|e| name.ends_with(e));
    // F7 additions: `password`/`passwd`, plus `auth` matched as an exact stem
    // rather than a substring. `authors.json` is a real filename in real repos
    // and holds no credential, so `name.contains("auth")` would withhold — and
    // therefore make un-undoable — an ordinary data file. `token` was
    // CONSIDERED AND REJECTED for the same class of reason: `tokens.json` is
    // the conventional design-token filename, and withholding a design system's
    // token file is a worse outcome than the leak it would prevent.
    let stem = name.split('.').next().unwrap_or(&name);
    let credential_word = name.contains("secret")
        || name.contains("credential")
        || name.contains("key")
        || name.contains("password")
        || name.contains("passwd")
        || stem == "auth";

    name == ".env"
        || name == ".envrc"
        || name.starts_with(".env.")
        || name.ends_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".key")
        || name.ends_with(".keystore")
        // F7: `AuthKey_ABC123.p8` (Apple), `keystore.jks` (Java), `.ppk`
        // (PuTTY) — all private key material the pre-F7 suffix list missed.
        || name.ends_with(".p8")
        || name.ends_with(".jks")
        || name.ends_with(".ppk")
        || name.starts_with("credentials")
        || name.contains("id_rsa")
        || name.contains("id_ed25519")
        // F7: only `id_rsa`/`id_ed25519` were listed; the other two OpenSSH
        // private-key names were snapshotted in full. `contains` (not equality)
        // mirrors the two existing entries, so the `_sk` FIDO variants and the
        // `.pub` companions are covered the same way they already were.
        || name.contains("id_dsa")
        || name.contains("id_ecdsa")
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

    // ---- F6 (red team round 2): secret shapes the table used to pass ------
    //
    // FIXTURE DISCIPLINE, load-bearing: every literal below matches the
    // REGEX SHAPE of a credential and nothing else. Secret material is always
    // a run of `x` padded to the rule's minimum length, so no string here is
    // or resembles a live key, and GitHub push protection (which rejected an
    // earlier commit of the red-team doc for carrying real-shaped OpenAI
    // literals, GH013) has nothing to detect. Do not "improve" these into
    // realistic-looking tokens.
    //
    // Each positive assertion pins the SPECIFIC reason label, never a bare
    // `contains("redacted")` — the entropy fallback can also fire on some of
    // these inputs, and a reason-agnostic assertion would stay green with the
    // regex row deleted.

    #[test]
    fn f6_aws_secret_access_key_redacted() {
        // Bare 40-char secret (the `aws-secret-key` token rule).
        let fake = "AWSxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx01";
        assert_eq!(fake.len(), 40, "fixture must be exactly AWS secret length");
        let s = scrub(&format!("use {fake} for the upload"));
        assert!(
            s.contains("[redacted:aws-secret-key]"),
            "bare AWS secret access key must be redacted: {s}"
        );
        assert!(!s.contains("xxxxxxxx"), "secret body must not survive: {s}");
        // Trailing sentence punctuation must not defeat the shape check.
        let punct = scrub(&format!("the key is {fake}."));
        assert!(
            punct.contains("[redacted:aws-secret-key]") && punct.ends_with('.'),
            "trailing punctuation split off, secret still redacted: {punct}"
        );
        // Underscore-joined key names: the `\b` defect this fix removes.
        for form in [
            format!("aws_secret_access_key = {fake}"),
            format!("AWS_SECRET_ACCESS_KEY={fake}"),
        ] {
            let s = scrub(&form);
            assert!(
                s.contains("[redacted:credential-assignment]"),
                "underscore-joined credential name must redact: {s}"
            );
        }
    }

    #[test]
    fn f6_aws_secret_shape_does_not_eat_paths_or_shas() {
        // A path is a long run of the same character class; the exact-40 gate
        // is what keeps it out. This is the reason the rule is not a regex.
        let path = scrub("open /Users/dev/projects/agentrec/agentrec-core/src/scrub.rs now");
        assert!(!path.contains("aws-secret-key"), "path untouched: {path}");
        // A 40-char git SHA is lowercase-only: no uppercase, so the AWS shape
        // rejects it, and the pre-existing hex rule claims it instead.
        let sha = scrub("commit 356a192b7913b04c54574d18c28d46e6395428ab done");
        assert!(sha.contains("[redacted:hex-token]"), "sha stays hex: {sha}");
        assert!(!sha.contains("aws-secret-key"));
        // A 39- and a 41-char token of the same alphabet are not AWS secrets.
        for n in [39usize, 41] {
            let t = format!("Ab1{}", "x".repeat(n - 3));
            let s = scrub(&format!("value {t} end"));
            assert!(
                !s.contains("aws-secret-key"),
                "length {n} must not match the exact-40 shape: {s}"
            );
        }
    }

    #[test]
    fn f6_url_embedded_credentials_redacted() {
        for url in [
            "postgres://admin:xxxxxxxxxxxx@db.internal:5432/prod",
            "mongodb+srv://root:xxxxxxxxxxxx@cluster0.example.net/admin",
            "https://ci:xxxxxxxxxxxx@git.example.com/org/repo.git",
        ] {
            let s = scrub(&format!("connect to {url} please"));
            assert!(
                s.contains("[redacted:url-credential]"),
                "URL credential must be redacted: {s}"
            );
            assert!(
                !s.contains("xxxxxxxxxxxx"),
                "password must not survive: {s}"
            );
        }
    }

    #[test]
    fn f6_urls_without_credentials_are_not_redacted() {
        // The negative half of the URL rule — over-redacting these would make
        // ordinary connection strings and doc links unreadable in every prompt.
        for url in [
            "postgres://db.internal:5432/prod",
            "postgres://admin@db.internal:5432/prod",
            "https://example.com/docs/getting-started/installation",
            "redis://cache.internal:6379/0",
            "git@github.com:user/repo.git",
        ] {
            let s = scrub(&format!("see {url} for details"));
            assert!(
                !s.contains("redacted"),
                "credential-free URL must survive verbatim: {s}"
            );
        }
    }

    #[test]
    fn f6_openai_style_key_redacted() {
        for key in [
            format!("sk-proj-{}", "x".repeat(48)),
            format!("sk-{}", "x".repeat(48)),
        ] {
            let s = scrub(&format!("export OPENAI_KEY {key} now"));
            assert!(
                s.contains("[redacted:openai-key]"),
                "OpenAI-shaped key must be redacted: {s}"
            );
        }
    }

    #[test]
    fn f6_openai_rule_does_not_fire_on_kebab_prose() {
        // `\b` is what stops `risk-`/`task-` style prose from matching.
        for text in [
            "risk-management-framework-review is scheduled",
            "the task-oriented-development-workflow doc",
            "rename sk-1 to sk-2",
        ] {
            let s = scrub(text);
            assert!(!s.contains("openai-key"), "prose must survive: {s}");
        }
    }

    #[test]
    fn f6_bearer_and_basic_auth_headers_redacted() {
        for header in [
            format!("Authorization: Bearer {}", "x".repeat(40)),
            format!("authorization: basic {}", "x".repeat(24)),
        ] {
            let s = scrub(&format!("send {header} with the request"));
            assert!(
                s.contains("[redacted:bearer-token]"),
                "auth header token must be redacted: {s}"
            );
            assert!(!s.contains("xxxxxxxxxxxxxxxxxxxxxxxx"));
        }
    }

    #[test]
    fn f6_bearer_in_prose_is_not_redacted() {
        for text in [
            "send a Bearer token with the request",
            "Authorization: Bearer <token>",
            "we use basic authentication here",
        ] {
            let s = scrub(text);
            assert!(!s.contains("redacted"), "prose must survive: {s}");
        }
    }

    #[test]
    fn f6_credential_assignment_still_ignores_plain_prose() {
        // The key-name widening removed `\b`; it must not have turned every
        // sentence containing a credential word into a redaction. The rule
        // still requires a `=`/`:` AND an 8+ char value.
        for text in [
            "update the token handling in auth",
            "rotate the password next week",
            "the api_key argument is optional",
        ] {
            let s = scrub(text);
            assert!(!s.contains("redacted"), "prose must survive: {s}");
        }
    }

    // ---- F7: secret-FILE denylist misses ---------------------------------

    #[test]
    fn f7_credential_filenames_withheld() {
        for p in [
            ".netrc",
            "_netrc",
            ".git-credentials",
            ".pgpass",
            ".npmrc",
            ".pypirc",
            ".htpasswd",
            ".dockercfg",
            "kubeconfig",
            "deploy/kubeconfig",
            "infra/terraform.tfstate",
            "infra/terraform.tfstate.backup",
            "/home/u/.ssh/id_dsa",
            "/home/u/.ssh/id_ecdsa",
            "keys/id_ecdsa_sk",
            "certs/AuthKey_ABC123.p8",
            "certs/keystore.jks",
            "certs/session.ppk",
            "config/auth.json",
            "config/passwords.yaml",
        ] {
            assert!(is_secret_path(p), "{p} should be withheld");
        }
    }

    #[test]
    fn f7_directory_components_are_consulted() {
        // The sharpest pre-F7 miss: the credential word lives in the
        // DIRECTORY and the basename alone matches nothing.
        for p in [
            "secrets/prod.yaml",
            "app/secrets/database.yml",
            ".secrets/token",
            "/home/u/.aws/config",
            "/home/u/.ssh/known_hosts",
            "/home/u/.kube/config",
            "/home/u/.docker/config.json",
            "/home/u/.gnupg/gpg.conf",
            // Windows-style separators reach this function too.
            "app\\secrets\\database.yml",
        ] {
            assert!(is_secret_path(p), "{p} should be withheld (directory)");
        }
    }

    #[test]
    fn f7_directory_matching_does_not_over_withhold() {
        // Withheld == never snapshotted == never undoable, so the component
        // set is deliberately narrow. These must all still be recorded.
        for p in [
            "src/credentials/oauth_client.rs",
            "src/keys/keymap.ts",
            "src/config/routes.rs",
            ".config/nvim/init.lua",
            "docs/secrets-management.md",
            "tokens.json",
            "design/tokens.json",
            "authors.json",
            "migrations/0001_init.sql",
            "certs/server.crt",
        ] {
            assert!(!is_secret_path(p), "{p} should NOT be withheld");
        }
    }
}
