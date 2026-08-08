//! `annotate`: join `git blame` over a git range against agentrec turns.
//!
//! This is the ONLY layer that knows about git (Phase 3.0 T4 plan constraint —
//! `agentrec-core::annotate` stays git-free). Responsibilities here: spawn
//! `git`, parse `--porcelain`, hand `agentrec_core::annotate::BlameRange`s to
//! `RepositoryView::annotate`, and render. No attribution logic lives here.
//!
//! **Precision.** Every output mode carries the best-effort disclaimer: text
//! and `--md` as prose ([`DISCLAIMER`]), `--json` as the
//! `"line_precision": "best_effort"` key the core result already holds. Spec
//! §3.0.3 makes any wording implying line-exactness a defect.

use agentrec_core::annotate::{AnnotateResult, Attribution, BlameRange, UnattributableReason};
use agentrec_core::view;
use std::path::Path;
use std::process::Command;

/// The prose form of the precision contract. One line, present in text and
/// `--md`; `--json` carries the machine-readable `line_precision` key instead.
pub const DISCLAIMER: &str = "line attribution is best-effort: turns record file-level snapshots, \
not per-line provenance — renames, reformats, and interleaved human+agent edits within one file \
can misattribute lines.";

/// `agentrec annotate <git-range> [--json|--md]`. Read-only: spawns `git`,
/// folds, prints. No write path exists here.
pub fn annotate(root: &Path, range: String, json: bool, md: bool) -> Result<(), String> {
    let paths = changed_paths(root, &range)?;
    let mut ranges: Vec<BlameRange> = Vec::new();
    for path in &paths {
        ranges.extend(blame_ranges(root, &range, path)?);
    }

    let view = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let result = view.annotate(&ranges);

    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print!(
        "{}",
        if md {
            render_md(&range, &result)
        } else {
            render_text(&range, &result)
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Run `git` in `root`, returning stdout or a typed-shaped error message.
///
/// Three distinguishable failures, none of them a panic: git is not
/// installed/executable, git exited non-zero (not a repository, unknown
/// revision, malformed range — git's own stderr is relayed rather than
/// second-guessed), or its output is not UTF-8.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("git exited non-zero with no message");
        return Err(format!("git {}: {detail}", args.join(" ")));
    }
    String::from_utf8(out.stdout).map_err(|_| "git printed non-UTF-8 output".to_string())
}

/// Repository-relative paths changed by the range. Deleted paths are kept out
/// by `--diff-filter=d`: blaming a path that no longer exists is an error, and
/// an error there would take down the whole invocation.
fn changed_paths(root: &Path, range: &str) -> Result<Vec<String>, String> {
    let out = git(root, &["diff", "--name-only", "--diff-filter=d", range])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// `git blame --porcelain <range> -- <path>`, parsed into blame ranges.
fn blame_ranges(root: &Path, range: &str, path: &str) -> Result<Vec<BlameRange>, String> {
    let out = git(root, &["blame", "--porcelain", range, "--", path])?;
    Ok(parse_porcelain(&out, path))
}

/// Parse `git blame --porcelain` output for ONE path.
///
/// The only lines that matter are group headers: `<sha> <orig> <final>
/// <numlines>`. A continuation header inside a group omits `<numlines>` and is
/// therefore skipped — its lines are already covered by the group header that
/// opened the run. Content lines (TAB-prefixed) and metadata lines
/// (`author`, `filename`, …) are ignored; a `^`-prefixed boundary sha is
/// carried through as-is, since "which commit" is git's answer, not ours.
fn parse_porcelain(out: &str, path: &str) -> Vec<BlameRange> {
    let mut ranges = Vec::new();
    for line in out.lines() {
        if line.starts_with('\t') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let sha = fields[0].trim_start_matches('^');
        if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let (Ok(final_line), Ok(num_lines)) = (fields[2].parse::<u32>(), fields[3].parse::<u32>())
        else {
            continue;
        };
        ranges.push(BlameRange {
            path: path.to_string(),
            commit: fields[0].to_string(),
            start_line: final_line,
            line_count: num_lines,
        });
    }
    ranges
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

/// One range's verdict as a single phrase. A one-element `Turns` collapses to
/// the bare turn (core keeps the Vec because turns genuinely overlap; the
/// reader should not pay for that when there is only one).
fn attribution_text(a: &Attribution) -> String {
    match a {
        Attribution::Human => "human".to_string(),
        Attribution::Unattributable { reason } => match reason {
            UnattributableReason::Gap => "unattributable (gap)".to_string(),
            UnattributableReason::BlobEvicted => "unattributable (blob evicted)".to_string(),
        },
        Attribution::Turns { turns } => turns
            .iter()
            .map(|t| {
                let tool = t.tool.as_deref().unwrap_or("unknown-tool");
                let model = t.model.as_deref().unwrap_or("unknown-model");
                let excerpt = t.prompt_excerpt.as_deref().unwrap_or("");
                let ts = &t.ts;
                let id = &t.id;
                if excerpt.is_empty() {
                    format!("{id} ({tool}/{model}, {ts})")
                } else {
                    format!("{id} ({tool}/{model}, {ts}) {excerpt:?}")
                }
            })
            .collect::<Vec<_>>()
            .join(" + "),
    }
}

fn line_span(start: u32, count: u32) -> String {
    if count <= 1 {
        format!("{start}")
    } else {
        format!("{start}-{}", start + count - 1)
    }
}

fn render_text(range: &str, r: &AnnotateResult) -> String {
    let mut out = format!("annotate {range}\n{DISCLAIMER}\n");
    if r.files.is_empty() {
        out.push_str("no blamed lines in this range\n");
    }
    for f in &r.files {
        out.push_str(&format!("{}\n", f.path));
        for ar in &f.ranges {
            out.push_str(&format!(
                "  {span} {commit} {who}\n",
                span = line_span(ar.start_line, ar.line_count),
                commit = ar.commit,
                who = attribution_text(&ar.attribution),
            ));
        }
    }
    out.push_str(&format!("evicted_ranges={}\n", r.evicted_ranges));
    out
}

fn render_md(range: &str, r: &AnnotateResult) -> String {
    let mut out = format!("# annotate `{range}`\n\n_{DISCLAIMER}_\n\n");
    if r.files.is_empty() {
        out.push_str("No blamed lines in this range.\n\n");
    }
    for f in &r.files {
        out.push_str(&format!(
            "## `{}`\n\n| lines | commit | attribution |\n| --- | --- | --- |\n",
            f.path
        ));
        for ar in &f.ranges {
            out.push_str(&format!(
                "| {span} | `{commit}` | {who} |\n",
                span = line_span(ar.start_line, ar.line_count),
                commit = ar.commit,
                who = attribution_text(&ar.attribution),
            ));
        }
        out.push('\n');
    }
    out.push_str(&format!("evicted_ranges: {}\n", r.evicted_ranges));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `git blame --porcelain` shape: a group header carrying
    /// `<sha> <orig> <final> <numlines>`, metadata lines, TAB-prefixed
    /// content, then a continuation header WITHOUT a numlines field — which
    /// must not mint a second range.
    #[test]
    fn parse_porcelain_takes_group_headers_only() {
        let out = "\
1111111111111111111111111111111111111111 1 1 2
author Someone
filename src/x.rs
\tline one
1111111111111111111111111111111111111111 2 2
\tline two
2222222222222222222222222222222222222222 3 3 1
author Other
filename src/x.rs
\tline three
";
        let ranges = parse_porcelain(out, "src/x.rs");
        assert_eq!(ranges.len(), 2, "continuation header must not open a range");
        assert_eq!(ranges[0].start_line, 1);
        assert_eq!(ranges[0].line_count, 2);
        assert_eq!(ranges[1].start_line, 3);
        assert_eq!(ranges[1].line_count, 1);
        assert!(ranges.iter().all(|r| r.path == "src/x.rs"));
    }

    #[test]
    fn line_span_collapses_a_single_line() {
        assert_eq!(line_span(7, 1), "7");
        assert_eq!(line_span(7, 3), "7-9");
    }
}
