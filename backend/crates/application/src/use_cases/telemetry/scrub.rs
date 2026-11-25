//! Scrubbing for browser report text (OT-006 Decision 4).
//!
//! The browser applies the same rules before sending (`apps/web/src/lib/telemetry/scrub.ts`);
//! this copy is the authoritative one. Both are driven by
//! `backend/tests/fixtures/client_scrub_cases.json`, so a change here needs the
//! same change there, and a new case in the fixture.
//!
//! In order:
//! 1. **Addresses.** A word that looks like an address (contains `://` or
//!    starts with `/`) loses everything from its first `?` or `#`, except a
//!    trailing `:line` or `:line:column`, so stack frames keep their position.
//!    Words end at whitespace and at `" ' < > ( ) [ ] { }` and backticks.
//! 2. **Emails** become `[EMAIL]`.
//! 3. **`key=value` pairs** left in the text have the value of a sensitive key
//!    replaced (OT-001's [`redact_query`]).
//! 4. **Length:** cut to `max_chars` characters.

use shared::redact::redact_query;

/// Replacement for an email address.
pub const EMAIL_PLACEHOLDER: &str = "[EMAIL]";

/// Apply every rule above to `input`.
#[must_use]
pub fn scrub(input: &str, max_chars: usize) -> String {
    let text = strip_address_queries(input);
    let text = replace_emails(&text);
    let text = redact_key_values(&text);
    truncate_chars(&text, max_chars)
}

const fn ends_word(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '`'
        )
}

/// Rule 1, applied word by word; separators are kept as they are.
fn strip_address_queries(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut word = String::new();
    for c in input.chars() {
        if ends_word(c) {
            out.push_str(&strip_word_query(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&strip_word_query(&word));
    out
}

fn strip_word_query(word: &str) -> String {
    // Where the address's path starts: after `://`, or at a leading `/`.
    let path_start = match word.find("://") {
        Some(i) => i + 3,
        None if word.starts_with('/') => 0,
        None => return word.to_string(),
    };
    let Some(offset) = word[path_start..].find(['?', '#']) else {
        return word.to_string();
    };
    let cut = path_start + offset;
    let mut kept = word[..cut].to_string();
    kept.push_str(position_suffix(&word[cut..]));
    kept
}

/// A trailing `:digits` or `:digits:digits` of `tail`, or `""`.
fn position_suffix(tail: &str) -> &str {
    let mut suffix_start = tail.len();
    for _ in 0..2 {
        let head = &tail[..suffix_start];
        let digits = head.len() - head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || !head[..head.len() - digits].ends_with(':') {
            break;
        }
        suffix_start -= digits + 1;
    }
    &tail[suffix_start..]
}

const fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

const fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// Rule 2, the same matches as `/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g`.
fn replace_emails(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    // Characters before `copied` are already in `out`.
    let mut copied = 0;
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '@' {
            at += 1;
            continue;
        }
        let mut start = at;
        while start > copied && is_local_char(chars[start - 1]) {
            start -= 1;
        }
        let mut run_end = at + 1;
        while run_end < chars.len() && is_domain_char(chars[run_end]) {
            run_end += 1;
        }
        match (start < at).then(|| domain_end(&chars[at + 1..run_end])) {
            Some(Some(len)) => {
                out.extend(&chars[copied..start]);
                out.push_str(EMAIL_PLACEHOLDER);
                copied = at + 1 + len;
                at = copied;
            }
            _ => at += 1,
        }
    }
    out.extend(&chars[copied..]);
    out
}

/// Length of the longest prefix of `run` that is `X.L`, with `X` non-empty and
/// `L` two or more ASCII letters, as the regex's backtracking would find it.
fn domain_end(run: &[char]) -> Option<usize> {
    (1..=run.len()).rev().find(|&end| {
        let prefix = &run[..end];
        let letters = prefix
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_alphabetic())
            .count();
        letters >= 2 && end > letters + 1 && prefix[end - letters - 1] == '.'
    })
}

/// Rule 3, applied to each whitespace-separated word that contains `=`.
fn redact_key_values(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut word = String::new();
    for c in input.chars() {
        if c.is_whitespace() {
            push_redacted(&mut out, &word);
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    push_redacted(&mut out, &word);
    out
}

fn push_redacted(out: &mut String, word: &str) {
    if word.contains('=') {
        out.push_str(&redact_query(word));
    } else {
        out.push_str(word);
    }
}

/// Rule 4.
fn truncate_chars(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((byte, _)) => input[..byte].to_string(),
        None => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// One case of the fixture shared with the browser's copy of these rules.
    #[derive(Deserialize)]
    struct Case {
        name: String,
        input: String,
        max_chars: usize,
        expected: String,
    }

    const FIXTURE: &str = include_str!("../../../../../tests/fixtures/client_scrub_cases.json");

    fn cases() -> Vec<Case> {
        serde_json::from_str(FIXTURE).expect("client_scrub_cases.json is valid")
    }

    #[test]
    fn every_fixture_case_scrubs_to_its_expected_output() {
        let cases = cases();
        assert!(cases.len() >= 5, "the fixture lost its cases");
        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let actual = scrub(&case.input, case.max_chars);
                (actual != case.expected).then(|| {
                    format!(
                        "{}:\n  expected {:?}\n  actual   {actual:?}",
                        case.name, case.expected
                    )
                })
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn scrubbing_twice_changes_nothing() {
        // The browser scrubs, then the server scrubs the result again.
        for case in cases() {
            assert_eq!(
                scrub(&case.expected, case.max_chars),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn position_suffix_keeps_only_line_and_column() {
        assert_eq!(position_suffix("?v=1:12:34"), ":12:34");
        assert_eq!(position_suffix("?v=1:12"), ":12");
        assert_eq!(position_suffix("?v=1"), "");
        assert_eq!(position_suffix("#a:"), "");
        assert_eq!(position_suffix("?a=1:2:3:4"), ":3:4");
    }

    #[test]
    fn email_matching_follows_the_regex() {
        assert_eq!(replace_emails("a@b.c"), "a@b.c");
        assert_eq!(replace_emails("a@b.co"), EMAIL_PLACEHOLDER);
        assert_eq!(replace_emails("x a@b.co."), "x [EMAIL].");
        assert_eq!(replace_emails("a@b@c.com"), "a@[EMAIL]");
        assert_eq!(replace_emails("@b.com"), "@b.com");
        assert_eq!(replace_emails("a@.com"), "a@.com");
        assert_eq!(replace_emails("one@a.io two@b.io"), "[EMAIL] [EMAIL]");
        assert_eq!(replace_emails("é a@b.io é"), "é [EMAIL] é");
    }

    #[test]
    fn truncation_counts_characters() {
        assert_eq!(truncate_chars("ééé", 2), "éé");
        assert_eq!(truncate_chars("ab", 5), "ab");
    }
}
