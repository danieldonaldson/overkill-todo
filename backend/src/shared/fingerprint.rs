//! Error fingerprinting for grouping similar errors.
//!
//! Generates fingerprints for backend errors to help group similar issues
//! in dashboards and alerts. Fingerprints normalize dynamic values like
//! UUIDs, IDs, timestamps, and connection strings to group errors by their
//! root cause rather than specific instances.
//!
//! # Usage
//!
//! ```
//! use app::shared::fingerprint::generate_error_fingerprint;
//!
//! let fingerprint = generate_error_fingerprint(
//!     "Database",
//!     "connection to 192.168.1.100:5432 timed out after 30s"
//! );
//! // Fingerprint normalizes the IP and timeout value
//! ```

/// Generate a fingerprint for error grouping.
///
/// The fingerprint is created by:
/// 1. Normalizing the error message (removing dynamic values)
/// 2. Combining with the error type
/// 3. Hashing to a short hex string
///
/// # Arguments
///
/// * `error_type` - The error category (e.g., "Internal", "Database", "`ExternalService`")
/// * `message` - The error message to fingerprint
///
/// # Returns
///
/// An 8-character hex string fingerprint suitable for grouping.
#[must_use]
pub fn generate_error_fingerprint(error_type: &str, message: &str) -> String {
    let normalized = normalize_error_message(message);
    let fingerprint_source = format!("{error_type}::{normalized}");

    // Use djb2 hash algorithm (same as frontend for consistency)
    let mut hash: u32 = 5381;
    for byte in fingerprint_source.bytes() {
        hash = hash.wrapping_mul(33) ^ u32::from(byte);
    }

    format!("{hash:08x}")
}

/// Normalize an error message by removing dynamic values.
///
/// This helps group errors that differ only in specific IDs, timestamps,
/// or other runtime-specific values.
fn normalize_error_message(message: &str) -> String {
    let mut normalized = message.to_string();

    // Remove UUIDs (both hyphenated and non-hyphenated)
    // Matches: 550e8400-e29b-41d4-a716-446655440000
    normalized = regex_replace(
        &normalized,
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        "<UUID>",
    );

    // Remove long numeric IDs (5+ digits)
    normalized = regex_replace(&normalized, r"\b\d{5,}\b", "<ID>");

    // Remove IP addresses (IPv4)
    normalized = regex_replace(
        &normalized,
        r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",
        "<IP>",
    );

    // Remove timestamps in various formats (BEFORE ports, since timestamps contain colons)
    // ISO 8601: 2026-01-29T12:00:00Z
    normalized = regex_replace(
        &normalized,
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?",
        "<TIMESTAMP>",
    );

    // Remove port numbers in connection strings (e.g., :5432, :6379)
    // Must be after timestamps since timestamps also have colons
    normalized = regex_replace(&normalized, r":\d{2,5}\b", ":<PORT>");

    // Remove duration values (e.g., 30s, 500ms, 5m)
    normalized = regex_replace(&normalized, r"\b\d+(?:ms|s|m|h)\b", "<DURATION>");

    // Remove byte sizes (e.g., 1024 bytes, 5MB)
    normalized = regex_replace(&normalized, r"\b\d+\s*(?:bytes?|[KMGT]B)\b", "<SIZE>");

    // Remove quoted strings that might contain dynamic content
    // But preserve short strings that might be error codes
    normalized = regex_replace(&normalized, r#""[^"]{20,}""#, r#""<STRING>""#);
    normalized = regex_replace(&normalized, r"'[^']{20,}'", "'<STRING>'");

    // Remove URLs (BEFORE paths, since URLs contain paths)
    normalized = regex_replace(&normalized, r"https?://[^\s]+", "<URL>");

    // Remove file paths (Unix-style)
    normalized = regex_replace(
        &normalized,
        r"/(?:[a-zA-Z0-9_.-]+/)+[a-zA-Z0-9_.-]+",
        "<PATH>",
    );

    // Remove email addresses
    normalized = regex_replace(
        &normalized,
        r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b",
        "<EMAIL>",
    );

    // Collapse multiple spaces
    normalized = regex_replace(&normalized, r"\s+", " ");

    normalized.trim().to_string()
}

/// Simple regex replacement without pulling in the regex crate.
///
/// Uses a basic pattern matching approach for common patterns.
fn regex_replace(input: &str, pattern: &str, replacement: &str) -> String {
    // For simplicity, we use a simple state machine approach for the patterns we need.
    // This avoids adding the regex crate as a dependency.

    match pattern {
        // UUID pattern
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" => {
            replace_uuids(input, replacement)
        }
        // Long numeric IDs
        r"\b\d{5,}\b" => replace_long_numbers(input, replacement),
        // IPv4 addresses
        r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b" => replace_ipv4(input, replacement),
        // Port numbers
        r":\d{2,5}\b" => replace_ports(input, replacement),
        // ISO timestamps
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?" => {
            replace_iso_timestamps(input, replacement)
        }
        // Durations
        r"\b\d+(?:ms|s|m|h)\b" => replace_durations(input, replacement),
        // Byte sizes
        r"\b\d+\s*(?:bytes?|[KMGT]B)\b" => replace_byte_sizes(input, replacement),
        // Long quoted strings (double quotes)
        r#""[^"]{20,}""# => replace_long_quoted_strings(input, replacement, '"'),
        // Long quoted strings (single quotes)
        r"'[^']{20,}'" => replace_long_quoted_strings(input, replacement, '\''),
        // Unix file paths
        r"/(?:[a-zA-Z0-9_.-]+/)+[a-zA-Z0-9_.-]+" => replace_unix_paths(input, replacement),
        // Email addresses
        r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b" => {
            replace_emails(input, replacement)
        }
        // URLs
        r"https?://[^\s]+" => replace_urls(input, replacement),
        // Multiple spaces
        r"\s+" => collapse_whitespace(input),
        _ => input.to_string(),
    }
}

fn replace_uuids(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Check if we might be at the start of a UUID (8-4-4-4-12 hex pattern)
        if i + 36 <= chars.len() && is_uuid_at(&chars, i) {
            result.push_str(replacement);
            i += 36;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn is_uuid_at(chars: &[char], start: usize) -> bool {
    // UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars)
    if start + 36 > chars.len() {
        return false;
    }

    let positions = [8, 13, 18, 23]; // positions of hyphens
    for (i, &c) in chars[start..start + 36].iter().enumerate() {
        if positions.contains(&i) {
            if c != '-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn replace_long_numbers(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut num_start: Option<usize> = None;
    let chars: Vec<char> = input.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_digit() {
            if num_start.is_none() {
                // Check if preceded by a word boundary
                if i == 0 || !chars[i - 1].is_alphanumeric() {
                    num_start = Some(i);
                } else {
                    // Not at word boundary, just push the digit
                    result.push(c);
                }
            }
            // else: num_start is set, we're accumulating digits (don't push yet)
        } else {
            if let Some(start) = num_start {
                let len = i - start;
                // Check word boundary after
                if len >= 5 && !c.is_alphanumeric() {
                    result.push_str(replacement);
                } else {
                    result.push_str(&chars[start..i].iter().collect::<String>());
                }
                num_start = None;
            }
            result.push(c);
        }
    }

    // Handle trailing number
    if let Some(start) = num_start {
        let len = chars.len() - start;
        if len >= 5 {
            result.push_str(replacement);
        } else {
            result.push_str(&chars[start..].iter().collect::<String>());
        }
    }

    result
}

fn replace_ipv4(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if let Some(ip_len) = try_parse_ipv4(&chars, i) {
            // Check word boundaries
            let start_ok = i == 0 || !chars[i - 1].is_alphanumeric();
            let end_ok = i + ip_len >= chars.len() || !chars[i + ip_len].is_alphanumeric();

            if start_ok && end_ok {
                result.push_str(replacement);
                i += ip_len;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

fn try_parse_ipv4(chars: &[char], start: usize) -> Option<usize> {
    let mut pos = start;
    let mut octets = 0;

    while octets < 4 && pos < chars.len() {
        // Parse octet (1-3 digits)
        let octet_start = pos;
        while pos < chars.len() && chars[pos].is_ascii_digit() && pos - octet_start < 3 {
            pos += 1;
        }

        if pos == octet_start {
            return None;
        }

        // Parse the number and validate range
        let octet_str: String = chars[octet_start..pos].iter().collect();
        if octet_str.parse::<u32>().map_or(true, |n| n > 255) {
            return None;
        }

        octets += 1;

        // Expect dot between octets
        if octets < 4 {
            if pos >= chars.len() || chars[pos] != '.' {
                return None;
            }
            pos += 1;
        }
    }

    if octets == 4 { Some(pos - start) } else { None }
}

fn replace_ports(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == ':' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() && j - i <= 5 {
                j += 1;
            }
            let port_len = j - i - 1;
            // Check word boundary after
            let end_ok = j >= chars.len() || !chars[j].is_alphanumeric();
            if (2..=5).contains(&port_len) && end_ok {
                result.push_str(replacement);
                i = j;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

fn replace_iso_timestamps(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if let Some(ts_len) = try_parse_iso_timestamp(&chars, i) {
            result.push_str(replacement);
            i += ts_len;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn try_parse_iso_timestamp(chars: &[char], start: usize) -> Option<usize> {
    // Minimum: 2026-01-29T12:00:00Z (20 chars)
    if start + 20 > chars.len() {
        return None;
    }

    // Check date part: YYYY-MM-DD
    for i in 0..4 {
        if !chars[start + i].is_ascii_digit() {
            return None;
        }
    }
    if chars[start + 4] != '-' || chars[start + 7] != '-' {
        return None;
    }
    for i in [5, 6, 8, 9] {
        if !chars[start + i].is_ascii_digit() {
            return None;
        }
    }

    // Check T separator
    if chars[start + 10] != 'T' {
        return None;
    }

    // Check time part: HH:MM:SS
    if chars[start + 13] != ':' || chars[start + 16] != ':' {
        return None;
    }
    for i in [11, 12, 14, 15, 17, 18] {
        if !chars[start + i].is_ascii_digit() {
            return None;
        }
    }

    let mut pos = start + 19;

    // Optional fractional seconds
    if pos < chars.len() && chars[pos] == '.' {
        pos += 1;
        while pos < chars.len() && chars[pos].is_ascii_digit() {
            pos += 1;
        }
    }

    // Timezone: Z or +/-HH:MM
    if pos < chars.len() {
        if chars[pos] == 'Z' {
            pos += 1;
        } else if chars[pos] == '+' || chars[pos] == '-' {
            pos += 1;
            // HH:MM or HHMM
            let mut tz_digits = 0;
            while pos < chars.len() && (chars[pos].is_ascii_digit() || chars[pos] == ':') {
                if chars[pos].is_ascii_digit() {
                    tz_digits += 1;
                }
                pos += 1;
                if tz_digits >= 4 {
                    break;
                }
            }
        }
    }

    Some(pos - start)
}

fn replace_durations(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Check for word boundary at start
        let start_ok = i == 0 || !chars[i - 1].is_alphanumeric();

        if start_ok && chars[i].is_ascii_digit() {
            let num_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }

            // Check for duration suffix
            if i < chars.len() {
                let has_ms = i + 2 <= chars.len()
                    && chars[i..i + 2].iter().collect::<String>().to_lowercase() == "ms";
                let has_single = matches!(chars[i].to_ascii_lowercase(), 's' | 'm' | 'h');

                if has_ms {
                    // Check word boundary after
                    if i + 2 >= chars.len() || !chars[i + 2].is_alphanumeric() {
                        result.push_str(replacement);
                        i += 2;
                        continue;
                    }
                } else if has_single {
                    // Check word boundary after
                    if i + 1 >= chars.len() || !chars[i + 1].is_alphanumeric() {
                        result.push_str(replacement);
                        i += 1;
                        continue;
                    }
                }
            }

            // Not a duration, push the number
            result.push_str(&chars[num_start..i].iter().collect::<String>());
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn replace_byte_sizes(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Check for word boundary at start
        let start_ok = i == 0 || !chars[i - 1].is_alphanumeric();

        if start_ok && chars[i].is_ascii_digit() {
            let num_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }

            // Skip optional whitespace
            let space_start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }

            // Check for size suffix
            let remaining: String = chars[i..].iter().take(5).collect();
            let remaining_lower = remaining.to_lowercase();

            let suffix_len = if remaining_lower.starts_with("bytes") {
                5
            } else if remaining_lower.starts_with("byte") {
                4
            } else if remaining_lower.len() >= 2
                && matches!(
                    remaining.chars().next(),
                    Some('K' | 'M' | 'G' | 'T' | 'k' | 'm' | 'g' | 't')
                )
                && matches!(remaining.chars().nth(1), Some('B' | 'b'))
            {
                2
            } else {
                0
            };

            if suffix_len > 0 {
                // Check word boundary after
                let after_pos = i + suffix_len;
                if after_pos >= chars.len() || !chars[after_pos].is_alphanumeric() {
                    result.push_str(replacement);
                    i = after_pos;
                    continue;
                }
            }

            // Not a byte size, push original
            result.push_str(&chars[num_start..space_start].iter().collect::<String>());
            i = space_start;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn replace_long_quoted_strings(input: &str, replacement: &str, quote: char) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == quote {
            // Find closing quote
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }

            if i < chars.len() {
                let content_len = i - start - 1;
                i += 1; // skip closing quote

                if content_len >= 20 {
                    result.push(quote);
                    result.push_str(replacement);
                    result.push(quote);
                } else {
                    result.push_str(&chars[start..i].iter().collect::<String>());
                }
            } else {
                // No closing quote found
                result.push_str(&chars[start..].iter().collect::<String>());
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn replace_unix_paths(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '/' {
            let start = i;
            let mut segments = 0;

            while i < chars.len() {
                if chars[i] == '/' {
                    i += 1;
                    // Parse path segment
                    let seg_start = i;
                    while i < chars.len()
                        && (chars[i].is_alphanumeric()
                            || chars[i] == '_'
                            || chars[i] == '.'
                            || chars[i] == '-')
                    {
                        i += 1;
                    }
                    if i > seg_start {
                        segments += 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            // Need at least 2 segments (e.g., /foo/bar)
            if segments >= 2 {
                result.push_str(replacement);
            } else {
                result.push_str(&chars[start..i].iter().collect::<String>());
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn replace_emails(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for @ symbol and try to parse email around it
        if chars[i] == '@' {
            // Find start of local part
            let mut local_start = i;
            while local_start > 0 {
                let c = chars[local_start - 1];
                if c.is_alphanumeric() || c == '.' || c == '_' || c == '%' || c == '+' || c == '-' {
                    local_start -= 1;
                } else {
                    break;
                }
            }

            // Find end of domain
            let mut domain_end = i + 1;
            let mut has_dot = false;
            while domain_end < chars.len() {
                let c = chars[domain_end];
                if c == '.' {
                    has_dot = true;
                    domain_end += 1;
                } else if c.is_alphanumeric() || c == '-' {
                    domain_end += 1;
                } else {
                    break;
                }
            }

            // Validate: local part exists, domain has at least one dot, ends with 2+ letter TLD
            let local_len = i - local_start;
            let domain_part: String = chars[i + 1..domain_end].iter().collect();

            if local_len > 0
                && has_dot
                && domain_part
                    .split('.')
                    .next_back()
                    .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(char::is_alphabetic))
            {
                // Replace everything up to local_start with what we've accumulated
                result.truncate(result.len() - local_len);
                result.push_str(replacement);
                i = domain_end;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn replace_urls(input: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for http:// or https://
        let remaining: String = chars[i..].iter().take(8).collect();
        let is_http = remaining.starts_with("http://");
        let is_https = remaining.starts_with("https://");

        if is_http || is_https {
            i += if is_https { 8 } else { 7 };

            // Consume until whitespace
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }

            result.push_str(replacement);
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn collapse_whitespace(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut prev_was_space = false;

    for c in input.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(c);
            prev_was_space = false;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_basic() {
        let fp1 = generate_error_fingerprint("Database", "connection failed");
        let fp2 = generate_error_fingerprint("Database", "connection failed");
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 8);
    }

    #[test]
    fn test_fingerprint_different_types() {
        let fp1 = generate_error_fingerprint("Database", "connection failed");
        let fp2 = generate_error_fingerprint("Internal", "connection failed");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_normalize_uuid() {
        let msg = "Failed to find user 550e8400-e29b-41d4-a716-446655440000 in database";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<UUID>"));
        assert!(!normalized.contains("550e8400"));
    }

    #[test]
    fn test_normalize_long_numbers() {
        let msg = "Error processing order 123456789";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<ID>"));
        assert!(!normalized.contains("123456789"));
    }

    #[test]
    fn test_normalize_short_numbers_preserved() {
        let msg = "Error code 404";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("404"));
    }

    #[test]
    fn test_normalize_ipv4() {
        let msg = "Connection to 192.168.1.100 failed";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<IP>"));
        assert!(!normalized.contains("192.168.1.100"));
    }

    #[test]
    fn test_normalize_port() {
        let msg = "Cannot connect to localhost:5432";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains(":<PORT>"));
        assert!(!normalized.contains(":5432"));
    }

    #[test]
    fn test_normalize_timestamp() {
        let msg = "Error at 2026-01-29T12:30:45.123Z";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<TIMESTAMP>"));
        assert!(!normalized.contains("2026-01-29"));
    }

    #[test]
    fn test_normalize_duration() {
        let msg = "Timeout after 30s waiting for response";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<DURATION>"));
        assert!(!normalized.contains("30s"));
    }

    #[test]
    fn test_normalize_duration_ms() {
        let msg = "Request took 500ms";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<DURATION>"));
        assert!(!normalized.contains("500ms"));
    }

    #[test]
    fn test_normalize_byte_size() {
        let msg = "File too large: 1024 bytes";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<SIZE>"));
        assert!(!normalized.contains("1024 bytes"));
    }

    #[test]
    fn test_normalize_byte_size_mb() {
        let msg = "Payload exceeds 5MB limit";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<SIZE>"));
        assert!(!normalized.contains("5MB"));
    }

    #[test]
    fn test_normalize_long_quoted_string() {
        let msg = r#"Invalid value "this is a very long string that should be normalized""#;
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains(r#""<STRING>""#));
    }

    #[test]
    fn test_normalize_short_quoted_string_preserved() {
        let msg = r#"Invalid type "user""#;
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains(r#""user""#));
    }

    #[test]
    fn test_normalize_unix_path() {
        let msg = "Cannot read /var/log/app/error.log";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<PATH>"));
        assert!(!normalized.contains("/var/log/app/error.log"));
    }

    #[test]
    fn test_normalize_email() {
        let msg = "Failed to send email to user@example.com";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<EMAIL>"));
        assert!(!normalized.contains("user@example.com"));
    }

    #[test]
    fn test_normalize_url() {
        let msg = "Request to https://api.example.com/v1/users failed";
        let normalized = normalize_error_message(msg);
        assert!(normalized.contains("<URL>"));
        assert!(!normalized.contains("https://api.example.com"));
    }

    #[test]
    fn test_fingerprint_same_error_different_ids() {
        let fp1 = generate_error_fingerprint(
            "Database",
            "Failed to find user 550e8400-e29b-41d4-a716-446655440000",
        );
        let fp2 = generate_error_fingerprint(
            "Database",
            "Failed to find user 123e4567-e89b-12d3-a456-426614174000",
        );
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_same_error_different_ips() {
        let fp1 =
            generate_error_fingerprint("Database", "Connection to 192.168.1.100:5432 refused");
        let fp2 = generate_error_fingerprint("Database", "Connection to 10.0.0.1:5432 refused");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_different_errors() {
        let fp1 = generate_error_fingerprint("Database", "Connection refused");
        let fp2 = generate_error_fingerprint("Database", "Timeout waiting for response");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_complex_database_error() {
        let msg =
            "error returned from database: connection to 192.168.1.50:5432 timed out after 30s";
        let fp1 = generate_error_fingerprint("Database", msg);

        let msg2 = "error returned from database: connection to 10.0.0.1:5432 timed out after 60s";
        let fp2 = generate_error_fingerprint("Database", msg2);

        assert_eq!(fp1, fp2);
    }
}
