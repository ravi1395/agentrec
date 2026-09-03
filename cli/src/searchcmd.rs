//! `search`: substring/regex search over stored prompt text, turn metadata,
//! and file paths. Thin adapter over `agentrec_core::search`/
//! `RepositoryView::search` (Phase 3.0 T3) — no matching logic here, no
//! CAS/ledger reads outside the view call.
//!
//! `--content` (file-snapshot content search) is a reserved, NOT-implemented
//! slot (spec §3.0.2 non-goal): it errors rather than silently degrading to
//! the metadata search this command actually performs.

use agentrec_core::search::{MatchedField, SearchHit, SearchPage, SearchQuery};
use agentrec_core::view;
use std::path::Path;

/// `agentrec search <pattern> [--regex] [--content] [--json]`. Read-only:
/// opens the view, folds, prints. No write path exists here.
pub fn search(
    root: &Path,
    pattern: String,
    regex: bool,
    content: bool,
    json: bool,
) -> Result<(), String> {
    if content {
        return Err("content search not implemented".to_string());
    }
    let view = view::RepositoryView::open(root).map_err(|e| e.to_string())?;
    let q = SearchQuery { pattern, regex };
    let page = view.search(&q, None).map_err(|e| search_error_text(&e))?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&page).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print!("{}", render_search(&page));
    Ok(())
}

/// Prose for a [`agentrec_core::search::SearchError`]. All message text is
/// built here — the core type carries data, never prose (same discipline as
/// `readcmds::diff_error_text`).
fn search_error_text(e: &agentrec_core::search::SearchError) -> String {
    match e {
        agentrec_core::search::SearchError::BadPattern(msg) => {
            format!("invalid --regex pattern: {msg}")
        }
        agentrec_core::search::SearchError::Cursor(c) => match c {
            view::CursorError::Stale => {
                "search results changed since the last page — restart the search".to_string()
            }
            view::CursorError::QueryMismatch => {
                "internal error: search cursor does not match this query".to_string()
            }
            view::CursorError::ZeroLimit => "a limit of 0 has no honest page".to_string(),
        },
    }
}

fn field_label(f: MatchedField) -> &'static str {
    match f {
        MatchedField::Prompt => "prompt",
        MatchedField::Tool => "tool",
        MatchedField::Model => "model",
        MatchedField::Path => "path",
    }
}

/// Gate round-2 A1: `dangling_prompt_refs` renders EVERY time, including
/// zero and including the no-matches branch — the zeros-always-render
/// precedent `statscmd::render_rework` already set for every one of
/// `ReworkRate`'s disclosure fields.
fn render_search(page: &SearchPage) -> String {
    if page.page.items.is_empty() {
        return format!(
            "search: no matches\ndangling_prompt_refs={}\n",
            page.dangling_prompt_refs
        );
    }
    let mut out = String::new();
    for hit in &page.page.items {
        out.push_str(&render_hit(hit));
    }
    out.push_str(&format!(
        "dangling_prompt_refs={}\n",
        page.dangling_prompt_refs
    ));
    if page.page.next.is_some() {
        // Gate round-2: the CLI has no `--limit`/page-2 flag today (plan
        // decision, recorded at commit 3108e9e — `SearchQuery` carries no
        // page-size field, so this verb has no way to request a further
        // page). "more results available (paged)" was a dead end; this
        // names the one actionable option a human has today and the one a
        // machine caller has via `--json`'s `page.next` cursor.
        out.push_str(
            "more results available — narrow the pattern, or consume page.next via --json\n",
        );
    }
    out
}

fn render_hit(h: &SearchHit) -> String {
    format!(
        "{id} {ts} [{field}] {snippet}\n",
        id = h.turn_id,
        ts = h.ts,
        field = field_label(h.field),
        snippet = h.snippet,
    )
}
