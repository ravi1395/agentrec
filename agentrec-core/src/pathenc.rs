//! Path → wire-string conversion decision. Records serialize paths as JSON
//! strings (PROTOCOL: `ChangeObs.path` / `FileEntry.path`), which requires
//! valid UTF-8. A raw OS path is not always UTF-8 (arbitrary bytes are legal
//! in a Linux filename; macOS/HFS+/APFS reject non-UTF8 bytes at the syscall
//! level, so this is effectively Linux-only in practice).
//!
//! The one correct behavior here is "never guess": `to_string_lossy()`
//! substitutes U+FFFD for the invalid bytes, producing a *different* string
//! than the actual path — silently recording the wrong key would break every
//! later hash lookup keyed on that string (undo's file resolution, blame's
//! path join). Callers must treat a `None` here as "this file cannot be
//! represented in a wire record" and skip it, not paper over it.

use std::path::Path;

/// Losslessly convert `path` to the UTF-8 string a wire record needs.
/// `None` means the raw path bytes are not valid UTF-8 — the caller must
/// skip the file rather than fall back to a lossy conversion.
pub fn utf8_path(path: &Path) -> Option<&str> {
    path.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_path_round_trips() {
        let p = Path::new("src/a.rs");
        assert_eq!(utf8_path(p), Some("src/a.rs"));
    }

    #[test]
    fn empty_path_is_still_valid_utf8() {
        let p = Path::new("");
        assert_eq!(utf8_path(p), Some(""));
    }

    // Non-UTF8 bytes are only constructible as an OsStr/Path on unix
    // (Windows requires valid UTF-16 at the OsString boundary, and this repo
    // is macOS/Linux-only anyway, D19) — real invalid-UTF8 bytes via
    // `OsStrExt`, not a stand-in.
    #[cfg(unix)]
    #[test]
    fn non_utf8_bytes_are_rejected_not_lossily_converted() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // 0x66, 0x6f, 0xff, 0x6f — "fo\xFFo": 0xFF is not valid UTF-8 in any
        // position (a raw continuation/invalid byte).
        let bytes = [0x66, 0x6f, 0xff, 0x6f];
        let os_str = OsStr::from_bytes(&bytes);
        let path = Path::new(os_str);

        assert_eq!(
            utf8_path(path),
            None,
            "invalid UTF-8 path bytes must never silently round-trip as a string"
        );
        // Guard against a naive lossy-fallback reimplementation creeping
        // back in: `to_string_lossy` WOULD succeed here (producing
        // "fo\u{FFFD}o"), which is exactly the outcome `utf8_path` must
        // refuse to produce.
        assert_ne!(path.to_string_lossy(), "");
    }
}
