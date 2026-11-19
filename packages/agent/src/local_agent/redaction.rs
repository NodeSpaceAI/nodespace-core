//! Removing credentials from text a program or a server produced.
//!
//! One redactor, shared by the runners: it works on a stream's raw bytes and
//! converts to text last, so a character cut in half cannot turn the prefix of a
//! credential into text that no longer matches it.

use zeroize::Zeroizing;

/// A credential shorter than this is refused: redacting it would mangle ordinary
/// output, and it is not a credential.
pub(crate) const MIN_REDACTABLE_BYTES: usize = 4;
/// What a redacted value is replaced with.
pub(crate) const REDACTED: &str = "[redacted]";

/// Every place `needle` occurs in `data`, as (start, end) byte spans.
fn occurrences(data: &[u8], needle: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    if needle.is_empty() || data.len() < needle.len() {
        return found;
    }
    for start in 0..=(data.len() - needle.len()) {
        if data[start] == needle[0] && data[start..].starts_with(needle) {
            found.push((start, start + needle.len()));
        }
    }
    found
}

/// The length of the longest suffix of `data` that is a proper prefix of one of
/// the needles, if there is one: the part of a credential a stream was cut
/// inside of.
fn partial_tail(data: &[u8], needles: &[Zeroizing<String>]) -> Option<usize> {
    needles
        .iter()
        .filter_map(|needle| {
            let needle = needle.as_bytes();
            (1..needle.len())
                .rev()
                .find(|&k| data.ends_with(&needle[..k]))
        })
        .max()
}

/// A stream's bytes as text with every credential removed, working on the raw
/// bytes and converting to text last (so a character cut in half cannot turn a
/// credential's prefix into something that no longer matches it).
///
/// - Every occurrence of every credential is found in the original bytes, and
///   overlapping or adjacent occurrences are merged and replaced once by the
///   marker, so one credential that contains or overlaps another leaves no part
///   of itself behind.
/// - When `cut` (the stream ended because the program was stopped, not because
///   it finished), whatever the bytes end with that is the start of a
///   credential is removed too, repeatedly (removing one can uncover another).
///   That tail joins any occurrence it overlaps: such a span gets the marker; a
///   tail that overlaps none is dropped.
///
/// The result holds no complete credential, and, when `cut`, no prefix of one.
pub(crate) fn sanitize(data: &[u8], needles: &[Zeroizing<String>], cut: bool) -> String {
    // (start, end, is a complete occurrence)
    let mut spans: Vec<(usize, usize, bool)> = Vec::new();
    for needle in needles {
        spans.extend(
            occurrences(data, needle.as_bytes())
                .into_iter()
                .map(|(s, e)| (s, e, true)),
        );
    }
    if cut {
        let mut end = data.len();
        while let Some(k) = partial_tail(&data[..end], needles) {
            end -= k;
        }
        if end < data.len() {
            spans.push((end, data.len(), false));
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize, bool)> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.0 <= last.1 => {
                last.1 = last.1.max(span.1);
                last.2 |= span.2;
            }
            _ => merged.push(span),
        }
    }
    let mut out = Vec::with_capacity(data.len());
    let mut position = 0;
    for (start, end, complete) in merged {
        out.extend_from_slice(&data[position..start]);
        if complete {
            out.extend_from_slice(REDACTED.as_bytes());
        }
        position = end;
    }
    out.extend_from_slice(&data[position..]);
    String::from_utf8_lossy(&out).into_owned()
}

/// `text` cut to at most `max` bytes at a character boundary.
pub(crate) fn cut(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- sanitize: the cut-output invariant, exhaustively ------------------

    fn needles_of(values: &[&str]) -> Vec<Zeroizing<String>> {
        values
            .iter()
            .map(|v| Zeroizing::new(v.to_string()))
            .collect()
    }

    /// The invariant: whatever the cut position, the text holds no complete
    /// credential and, when the stream was cut, no prefix of one that is at
    /// least four bytes (a shorter prefix is not a credential).
    fn assert_no_leak(raw: &[u8], values: &[&str]) {
        let needles = needles_of(values);
        // Uncut: no whole credential.
        let whole = sanitize(raw, &needles, false);
        for v in values {
            assert!(!whole.contains(v), "complete {v:?} in {whole:?}");
        }
        // Cut at every position.
        for at in 0..=raw.len() {
            let text = sanitize(&raw[..at], &needles, true);
            for v in values {
                let bytes = v.as_bytes();
                for k in MIN_REDACTABLE_BYTES..=bytes.len() {
                    let prefix = &bytes[..k];
                    assert!(
                        !text.as_bytes().windows(k).any(|w| w == prefix),
                        "a {k}-byte prefix of {v:?} leaked cutting at {at} of {:?}: {text:?}",
                        String::from_utf8_lossy(raw)
                    );
                }
            }
        }
    }

    #[test]
    fn independent_ascii_credentials_leave_no_prefix_at_any_cut() {
        assert_no_leak(
            b"aaaa TOKENALPHA1 bb SECONDKEY99 cc TOKENALPHA1 dd SEC",
            &["TOKENALPHA1", "SECONDKEY99"],
        );
        // One 40-byte credential, the reviewer's shape.
        assert_no_leak(
            b"FFFFFFFFFFSECRETSECRETSECRETSECRETSECRETABCDFFFFFFFFFFFFFFFFFFFFSECRETSECRET",
            &["SECRETSECRETSECRETSECRETSECRETABCD"],
        );
    }

    #[test]
    fn a_credential_that_is_a_prefix_of_another_leaves_none_of_the_longer_one() {
        assert_no_leak(
            b"xx XYZ1234 yy XYZ1234QQ77WW88 zz XYZ1234QQ77WW88",
            &["XYZ1234", "XYZ1234QQ77WW88"],
        );
        // The order the values are listed in does not matter.
        assert_no_leak(
            b"xx XYZ1234QQ77WW88 yy XYZ1234",
            &["XYZ1234QQ77WW88", "XYZ1234"],
        );
    }

    #[test]
    fn credentials_that_overlap_or_repeat_themselves_leave_no_prefix() {
        // The end of one is the start of the other.
        assert_no_leak(
            b"xxABCDEF1234ZZZZ99yy EF1234ZZZZ99 ABCDEF1234",
            &["ABCDEF1234", "EF1234ZZZZ99"],
        );
        // A credential that overlaps itself.
        assert_no_leak(b"AAAA1AAAA1AAAA1AAAA", &["AAAA1AAAA1"]);
        assert_no_leak(b"ABABABAB ABABABABAB ABAB", &["ABABABAB"]);
    }

    #[test]
    fn credentials_with_multibyte_characters_leave_no_prefix_at_any_cut() {
        let value = "ABCDé€FGHIJ";
        let raw = format!("xx {value} yy {value} z ABCDé").into_bytes();
        assert_no_leak(&raw, &[value]);
        // Made only of multibyte characters.
        let only = "éé€€éé";
        let raw = format!("a {only} b {only} c é€").into_bytes();
        assert_no_leak(&raw, &[only]);
        // And alongside an ASCII one.
        assert_no_leak(
            format!("{value}|WXYZ1234|{value}").as_bytes(),
            &[value, "WXYZ1234"],
        );
    }

    /// Stripping a trailing part of one credential can uncover the start of
    /// another: one pass is not enough.
    #[test]
    fn stripping_a_tail_can_uncover_another_and_is_repeated() {
        let needles = needles_of(&["WXYZ1", "ZZ99QQ"]);
        let text = sanitize(b"xxxZZ99QWXY", &needles, true);
        assert!(!text.contains("ZZ99"), "{text:?}");
        assert_eq!(text, "xxx");
        assert_no_leak(b"xxxZZ99QWXY", &["WXYZ1", "ZZ99QQ"]);
    }

    #[test]
    fn a_tail_that_overlaps_nothing_is_dropped_and_one_that_overlaps_a_match_is_redacted_once() {
        let needles = needles_of(&["ABCDEF", "DEFGHI"]);
        // Ends with the whole of the first, which is also the start of the second.
        assert_eq!(sanitize(b"..ABCDEF", &needles, true), "..[redacted]");
        // Ends with a partial that touches nothing: dropped, no marker.
        assert_eq!(sanitize(b"..ABC", &needles_of(&["ABCDEF"]), true), "..");
        // Not cut: the same ending is ordinary text.
        assert_eq!(sanitize(b"..ABC", &needles_of(&["ABCDEF"]), false), "..ABC");
    }

    /// A complete occurrence that a trailing partial one overlaps: one marker,
    /// whichever span comes first.
    #[test]
    fn an_occurrence_overlapped_by_a_trailing_partial_keeps_its_marker() {
        let needles = needles_of(&["ABCDE", "CDEFGH"]);
        assert_eq!(sanitize(b"ABCDEFG", &needles, true), "[redacted]");
        assert_no_leak(b"xxABCDEFGHyy ABCDEFG", &["ABCDE", "CDEFGH"]);
    }
}
