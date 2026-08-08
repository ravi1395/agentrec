//! Substring/regex search over prompts, turn metadata, and file paths
//! (Phase 3.0 T3, spec `docs/superpowers/specs/2026-08-07-phase-3-design.md`
//! §3.0.2).
//!
//! Scope, precisely: stored prompt text (scrubbed form — the only form
//! persisted, read from CAS via `prompt_ref`), turn metadata (`tool`,
//! `model`), and turn file PATHS. File-SNAPSHOT blobs (`FileEntry::before`/
//! `after` content) are never read by this module — only the recorded path
//! string is searched. A dangling `prompt_ref` (TTL-evicted or corrupt) is
//! handled gracefully: that turn's prompt field is skipped, counted in
//! [`SearchPage::dangling_prompt_refs`], and the turn's other fields are
//! still searched — never a crash, never a silent skip of the whole turn.
//!
//! Paged with the existing [`crate::view::Page`]/[`crate::view::Cursor`]
//! machinery. The generic cursor contract — "the 0-based occurrence of this
//! identity among the records sharing it, in sequence order" — is defined
//! over records sharing an id, not specifically over ledger turn entries, so
//! it applies unchanged to this module's flat hit sequence keyed by each
//! hit's owning turn id. A turn producing more than one hit (e.g. both its
//! `tool` and a file path match) is exactly the same shape the existing
//! machinery already handles for same-id duplicate turn records (P4b).

use crate::record::LogRecord;
use crate::store::{BlobStore, StoreError};
use crate::view::{Cursor, CursorError, Ledger, Page};
use regex::Regex;
use serde::Serialize;
use std::path::Path;

/// Default page size. Not part of the public query (the plan's contract pins
/// `SearchQuery` to exactly `{pattern, regex}`); an internal constant, same
/// role as `stats::DEFAULT_REWORK_WINDOW_DAYS` plays for a knob that has no
/// CLI flag of its own today.
pub const SEARCH_PAGE_SIZE: usize = 50;

/// Search input (spec §3.0.2). Not `Serialize` — an input, mirroring
/// `stats::StatsOptions`'s stance (the produced types are what the wire
/// contract pins).
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub pattern: String,
    pub regex: bool,
}

impl SearchQuery {
    /// Stable identity of this query, mirroring `TurnQuery::fingerprint` /
    /// `DiffQuery::fingerprint`: a cursor minted against one pattern must
    /// never be honored against another.
    fn fingerprint(&self) -> String {
        format!("search:pattern={};regex={}", self.pattern, self.regex)
    }
}

/// Which field of a turn a hit matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedField {
    Prompt,
    Tool,
    Model,
    Path,
}

/// One match: the owning turn, when it ended, which field matched, and a
/// short excerpt around the match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub turn_id: String,
    pub ts: String,
    pub field: MatchedField,
    pub snippet: String,
}

/// A page of search hits. A WRAPPER around the shared [`Page`], not a field
/// added to it (plan decision P2): `Page<T>` is generic and reused by
/// `log`/`recall`/`diff` — widening it would additively change every one of
/// their `--json` wire shapes and goldens. `dangling_prompt_refs` lives
/// beside the page instead.
#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub page: Page<SearchHit>,
    /// Turns whose `prompt_ref` was set but whose blob could not be read
    /// (evicted past TTL, or hash-mismatched) over this WHOLE fold — not
    /// scoped to this one page. Counted once per such turn, regardless of
    /// whether the pattern would have matched its prompt.
    pub dangling_prompt_refs: u64,
}

/// Why a search could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// `--regex` pattern failed to compile. Carries the compiler's own
    /// message; no message is built here.
    BadPattern(String),
    Cursor(CursorError),
}

/// Either matcher this module supports, compiled once per call rather than
/// per haystack.
enum Matcher {
    Substring(String),
    Regex(Regex),
}

impl Matcher {
    fn compile(q: &SearchQuery) -> Result<Self, SearchError> {
        if q.regex {
            Regex::new(&q.pattern)
                .map(Matcher::Regex)
                .map_err(|e| SearchError::BadPattern(e.to_string()))
        } else {
            Ok(Matcher::Substring(q.pattern.clone()))
        }
    }

    /// Byte-offset span of the first match, if any.
    fn find_in(&self, haystack: &str) -> Option<(usize, usize)> {
        match self {
            Matcher::Substring(p) => {
                if p.is_empty() {
                    return None;
                }
                haystack.find(p.as_str()).map(|s| (s, s + p.len()))
            }
            Matcher::Regex(re) => re.find(haystack).map(|m| (m.start(), m.end())),
        }
    }
}

/// Context (bytes) kept on each side of a match in the rendered snippet.
const SNIPPET_CONTEXT: usize = 30;

/// An excerpt around `[start, end)`, char-boundary-safe: a byte-index window
/// naively sliced can land mid multi-byte codepoint and panic (the same
/// class of bug `statscmd::SinceArg`'s parser was fixed for at the T2 gate).
fn snippet(haystack: &str, start: usize, end: usize) -> String {
    let lo_target = start.saturating_sub(SNIPPET_CONTEXT);
    let hi_target = (end + SNIPPET_CONTEXT).min(haystack.len());
    let lo = (0..=lo_target)
        .rev()
        .find(|&i| haystack.is_char_boundary(i))
        .unwrap_or(0);
    let hi = (hi_target..=haystack.len())
        .find(|&i| haystack.is_char_boundary(i))
        .unwrap_or(haystack.len());
    haystack[lo..hi].to_string()
}

/// The one fold behind [`crate::view::RepositoryView::search`]. Takes an
/// already-read ledger and an explicit page size (crate-internal — the
/// public contract's `page_size`-less signature is `RepositoryView::search`
/// itself; tests exercise pagination by calling this directly with a small
/// size, the same pattern `stats::compute_stats` uses for its own knobs).
pub(crate) fn compute_search(
    ledger: &Ledger,
    objects_dir: &Path,
    q: &SearchQuery,
    cursor: Option<Cursor>,
    page_size: usize,
) -> Result<SearchPage, SearchError> {
    let matcher = Matcher::compile(q)?;
    let store = BlobStore::new(objects_dir);

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut dangling_prompt_refs: u64 = 0;

    for record in &ledger.records {
        let LogRecord::Turn(t) = record else {
            continue;
        };

        // Prompt field — reads CAS. A dangling ref is disclosed, never a
        // crash, and never prevents the turn's other fields from matching.
        if let Some(prompt_ref) = &t.prompt_ref {
            match store.get(prompt_ref) {
                Ok(bytes) => {
                    // Non-UTF8 prompt content: unreadable as searchable
                    // text. The blob IS present, so this is not dangling.
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some((s, e)) = matcher.find_in(&text) {
                            hits.push(SearchHit {
                                turn_id: t.id.clone(),
                                ts: t.ended.clone(),
                                field: MatchedField::Prompt,
                                snippet: snippet(&text, s, e),
                            });
                        }
                    }
                }
                Err(StoreError::Missing(_)) | Err(StoreError::Corrupt(_)) => {
                    dangling_prompt_refs += 1;
                }
            }
        }

        if let Some(tool) = &t.tool {
            if let Some((s, e)) = matcher.find_in(tool) {
                hits.push(SearchHit {
                    turn_id: t.id.clone(),
                    ts: t.ended.clone(),
                    field: MatchedField::Tool,
                    snippet: snippet(tool, s, e),
                });
            }
        }

        if let Some(model) = &t.model {
            if let Some((s, e)) = matcher.find_in(model) {
                hits.push(SearchHit {
                    turn_id: t.id.clone(),
                    ts: t.ended.clone(),
                    field: MatchedField::Model,
                    snippet: snippet(model, s, e),
                });
            }
        }

        // Paths only — never a file-snapshot blob read (spec §3.0.2
        // non-goal; `--content` is a reserved, explicitly-erroring slot,
        // handled by the CLI adapter, not here).
        for f in &t.files {
            if let Some((s, e)) = matcher.find_in(&f.path) {
                hits.push(SearchHit {
                    turn_id: t.id.clone(),
                    ts: t.ended.clone(),
                    field: MatchedField::Path,
                    snippet: snippet(&f.path, s, e),
                });
            }
        }
    }

    // Pagination mirrors `select_turns`/`RepositoryView::diff`'s cursor walk
    // exactly, over `hits` instead of the ledger's turn vec.
    let start = match &cursor {
        None => 0,
        Some(c) => {
            if c.query != q.fingerprint() {
                return Err(SearchError::Cursor(CursorError::QueryMismatch));
            }
            let Some(idx) = hits
                .iter()
                .enumerate()
                .filter(|(_, h)| h.turn_id == c.after_id)
                .map(|(i, _)| i)
                .nth(c.after_occurrence)
            else {
                return Err(SearchError::Cursor(CursorError::Stale));
            };
            idx + 1
        }
    };
    let start = start.min(hits.len());
    let end = start.saturating_add(page_size).min(hits.len());
    let items = hits[start..end].to_vec();
    let next = if end < hits.len() {
        let last_idx = end.saturating_sub(1);
        let last = &hits[last_idx];
        Some(Cursor {
            after_id: last.turn_id.clone(),
            after_occurrence: hits[..last_idx]
                .iter()
                .filter(|h| h.turn_id == last.turn_id)
                .count(),
            query: q.fingerprint(),
        })
    } else {
        None
    };

    Ok(SearchPage {
        page: Page { items, next },
        dangling_prompt_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{FileEntry, TurnRecord};

    fn fe(path: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            before: None,
            after: None,
            op: "modify".to_string(),
            skipped: false,
            withheld: false,
            baseline_unknown: false,
            skipped_reason: None,
            after_synthesized: None,
            link_kind: None,
            attribution: None,
        }
    }

    fn turn(
        id: &str,
        tool: Option<&str>,
        model: Option<&str>,
        files: Vec<FileEntry>,
    ) -> TurnRecord {
        TurnRecord {
            v: 1,
            id: id.to_string(),
            grade: "rich".to_string(),
            truncated: false,
            started: "2020-06-01T00:00:00.000Z".to_string(),
            ended: "2020-06-01T00:00:05.000Z".to_string(),
            tool: tool.map(str::to_string),
            model: model.map(str::to_string),
            session: None,
            root: "/repo".to_string(),
            prompt_ref: None,
            prompt_excerpt: None,
            merges: vec![],
            imported: None,
            files_complete: None,
            origin: None,
            files,
        }
    }

    fn ledger(records: Vec<LogRecord>) -> Ledger {
        Ledger {
            records,
            unknown_type_lines: 0,
            unparsed_lines: 0,
        }
    }

    fn q(pattern: &str, regex: bool) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_string(),
            regex,
        }
    }

    // ---- substring match per field, one fixture each ---------------------

    #[test]
    fn substring_matches_the_prompt_field_via_cas() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        let hash = store.put(b"please fix the flaky bisect test").unwrap();
        let mut t = turn("t_1", Some("claude"), None, vec![]);
        t.prompt_ref = Some(hash);
        let l = ledger(vec![LogRecord::Turn(t)]);

        let page =
            compute_search(&l, &objects, &q("flaky", false), None, SEARCH_PAGE_SIZE).unwrap();
        assert_eq!(page.page.items.len(), 1);
        assert_eq!(page.page.items[0].field, MatchedField::Prompt);
        assert_eq!(page.page.items[0].turn_id, "t_1");
        assert!(page.page.items[0].snippet.contains("flaky"));
        assert_eq!(page.dangling_prompt_refs, 0);
    }

    #[test]
    fn substring_matches_the_tool_field() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_1",
            Some("codex"),
            None,
            vec![],
        ))]);
        let page =
            compute_search(&l, &objects, &q("codex", false), None, SEARCH_PAGE_SIZE).unwrap();
        assert_eq!(page.page.items.len(), 1);
        assert_eq!(page.page.items[0].field, MatchedField::Tool);
    }

    #[test]
    fn substring_matches_the_model_field() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_1",
            Some("claude"),
            Some("opus-9"),
            vec![],
        ))]);
        let page =
            compute_search(&l, &objects, &q("opus-9", false), None, SEARCH_PAGE_SIZE).unwrap();
        assert_eq!(page.page.items.len(), 1);
        assert_eq!(page.page.items[0].field, MatchedField::Model);
    }

    #[test]
    fn substring_matches_a_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![LogRecord::Turn(turn(
            "t_1",
            Some("claude"),
            None,
            vec![fe("agentrec-core/src/bisect.rs")],
        ))]);
        let page =
            compute_search(&l, &objects, &q("bisect.rs", false), None, SEARCH_PAGE_SIZE).unwrap();
        assert_eq!(page.page.items.len(), 1);
        assert_eq!(page.page.items[0].field, MatchedField::Path);
        assert_eq!(page.page.items[0].snippet, "agentrec-core/src/bisect.rs");
    }

    // ---- --regex invalid pattern -------------------------------------------

    #[test]
    fn invalid_regex_pattern_is_a_typed_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![]);
        let err = compute_search(&l, &objects, &q("(unclosed", true), None, SEARCH_PAGE_SIZE)
            .unwrap_err();
        assert!(matches!(err, SearchError::BadPattern(_)));
    }

    // ---- dangling prompt_ref -------------------------------------------

    #[test]
    fn dangling_prompt_ref_skips_prompt_hit_counts_and_other_fields_still_match() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        std::fs::create_dir_all(&objects).unwrap();
        let mut t = turn("t_1", Some("marker-tool"), None, vec![]);
        // A well-formed-looking ref to a blob that was never written —
        // simulates a TTL-evicted prompt.
        t.prompt_ref = Some(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );
        let l = ledger(vec![LogRecord::Turn(t)]);

        let page = compute_search(
            &l,
            &objects,
            &q("marker-tool", false),
            None,
            SEARCH_PAGE_SIZE,
        )
        .unwrap();
        assert_eq!(page.dangling_prompt_refs, 1);
        assert_eq!(page.page.items.len(), 1, "tool field still matched");
        assert_eq!(page.page.items[0].field, MatchedField::Tool);
        // Never a crash even when the pattern would ALSO have matched the
        // (unreadable) prompt content.
        let page2 = compute_search(&l, &objects, &q("t_1", false), None, SEARCH_PAGE_SIZE);
        assert!(page2.is_ok());
    }

    // ---- same-id duplicate turns: both occurrences returned, cursor -------

    #[test]
    fn duplicate_turn_id_both_occurrences_hit_and_cursor_resumes_without_redelivery() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        // Two DISTINCT records sharing id "t_DUP" (P4b shape: a pre-fix
        // daemon's orphan-recovery double-emit / a resumed import
        // `sessionId`), both carrying the search term in their tool field so
        // each mints its own hit.
        let l = ledger(vec![
            LogRecord::Turn(turn("t_DUP", Some("needle-tool"), None, vec![])),
            LogRecord::Turn(turn("t_other", Some("unrelated"), None, vec![])),
            LogRecord::Turn(turn("t_DUP", Some("needle-tool"), None, vec![])),
        ]);

        // page_size 1 forces the cursor boundary to land INSIDE the
        // duplicate id's two hits.
        let page1 = compute_search(&l, &objects, &q("needle-tool", false), None, 1).unwrap();
        assert_eq!(page1.page.items.len(), 1);
        assert_eq!(page1.page.items[0].turn_id, "t_DUP");
        let cursor = page1.page.next.clone().expect("more hits remain");
        assert_eq!(cursor.after_occurrence, 0, "sits after the FIRST t_DUP hit");

        let page2 = compute_search(
            &l,
            &objects,
            &q("needle-tool", false),
            Some(cursor),
            SEARCH_PAGE_SIZE,
        )
        .unwrap();
        assert_eq!(
            page2.page.items.len(),
            1,
            "the second t_DUP hit, not re-delivered, not skipped"
        );
        assert_eq!(page2.page.items[0].turn_id, "t_DUP");
        assert!(page2.page.next.is_none());
    }

    // ---- file-snapshot blobs never read -------------------------------

    #[test]
    fn file_snapshot_content_is_never_read_even_when_only_it_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let store = BlobStore::new(&objects);
        // The search term lives ONLY inside a file blob's content — never in
        // the turn's prompt, tool, model, or any path string.
        let after_hash = store
            .put(b"the secret marker UNIQUE_NEEDLE_XYZ lives only in file content")
            .unwrap();
        let t = turn(
            "t_1",
            Some("claude"),
            None,
            vec![FileEntry {
                after: Some(after_hash),
                ..fe("src/lib.rs")
            }],
        );
        let l = ledger(vec![LogRecord::Turn(t)]);

        let page = compute_search(
            &l,
            &objects,
            &q("UNIQUE_NEEDLE_XYZ", false),
            None,
            SEARCH_PAGE_SIZE,
        )
        .unwrap();
        assert!(
            page.page.items.is_empty(),
            "file-snapshot content must never be searched: {:?}",
            page.page.items
        );
    }

    #[test]
    fn empty_ledger_yields_no_hits_and_no_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let objects = tmp.path().join("objects");
        let l = ledger(vec![]);
        let page =
            compute_search(&l, &objects, &q("anything", false), None, SEARCH_PAGE_SIZE).unwrap();
        assert!(page.page.items.is_empty());
        assert!(page.page.next.is_none());
        assert_eq!(page.dangling_prompt_refs, 0);
    }
}
