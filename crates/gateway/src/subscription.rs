//! Subscription subject matching with wildcard support.
//!
//! Supports NATS-style wildcards:
//! - `*` matches any single segment
//! - `>` matches one or more segments (only at the end)

/// Check if a subscription pattern matches a subject.
///
/// Pattern syntax:
/// - `*` matches exactly one segment
/// - `>` matches one or more segments (greedy, only at end)
/// - Literal segments match exactly
///
/// Examples:
/// - `agg123.market456.token789` matches `agg123.market456.token789`
/// - `agg123.*.token789` matches `agg123.anything.token789`
/// - `agg123.>` matches `agg123.a.b.c` (one or more segments)
pub fn matches_subject(pattern: &str, subject_parts: &[&str]) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('.').collect();
    matches_parts(&pattern_parts, subject_parts)
}

fn matches_parts(pattern_parts: &[&str], subject_parts: &[&str]) -> bool {
    let mut pi = 0; // pattern index
    let mut si = 0; // subject index

    while pi < pattern_parts.len() && si < subject_parts.len() {
        let p = pattern_parts[pi];

        if p == ">" {
            // Greedy match: consumes rest of subject
            // Must have at least one segment to match
            return si < subject_parts.len();
        } else if p == "*" {
            // Single segment wildcard
            pi += 1;
            si += 1;
        } else if p == subject_parts[si] {
            // Exact match
            pi += 1;
            si += 1;
        } else {
            // No match
            return false;
        }
    }

    // Check if we consumed all parts
    // Pattern ending with ">" already returned above
    pi == pattern_parts.len() && si == subject_parts.len()
}

/// Parse a subject string into its components.
pub fn parse_subject(subject: &str) -> Vec<String> {
    subject.split('.').map(|s| s.to_string()).collect()
}

/// Build a NATS subject from components.
pub fn build_subject(aggregate_id: &str, hashed_market_id: &str, clob_token_id: &str) -> String {
    format!("{}.{}.{}", aggregate_id, hashed_market_id, clob_token_id)
}

/// Extract subject components from a NATS subject.
/// Returns (aggregate_id, hashed_market_id, clob_token_id) if valid.
pub fn extract_subject_parts(subject: &str) -> Option<(String, String, String)> {
    // Subject format: orderbook.changes.{aggregate_id}.{hashed_market_id}.{clob_token_id}
    let parts: Vec<&str> = subject.split('.').collect();
    if parts.len() >= 5 && parts[0] == "orderbook" && parts[1] == "changes" {
        Some((
            parts[2].to_string(),
            parts[3].to_string(),
            parts[4].to_string(),
        ))
    } else if parts.len() >= 3 {
        // Direct format: {aggregate_id}.{hashed_market_id}.{clob_token_id}
        Some((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        ))
    } else {
        None
    }
}

/// Validate a subscription subject pattern.
/// Returns an error message if invalid, None if valid.
pub fn validate_pattern(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return Some("Empty pattern".to_string());
    }

    let parts: Vec<&str> = pattern.split('.').collect();

    // Check for invalid wildcard usage
    for (i, part) in parts.iter().enumerate() {
        if *part == ">" && i != parts.len() - 1 {
            return Some("> wildcard can only be at the end".to_string());
        }
        if part.is_empty() {
            return Some("Empty segment in pattern".to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let subject = ["agg123", "market456", "token789"];
        assert!(matches_subject("agg123.market456.token789", &subject));
        assert!(!matches_subject("agg123.market456.token000", &subject));
    }

    #[test]
    fn test_single_wildcard() {
        let subject = ["agg123", "market456", "token789"];
        assert!(matches_subject("agg123.*.token789", &subject));
        assert!(matches_subject("*.market456.token789", &subject));
        assert!(matches_subject("agg123.market456.*", &subject));
        assert!(matches_subject("*.*.*", &subject));
    }

    #[test]
    fn test_greedy_wildcard() {
        let subject = ["agg123", "market456", "token789"];
        assert!(matches_subject("agg123.>", &subject));
        assert!(matches_subject(">", &subject));
        assert!(!matches_subject("agg123.market456.token789.>", &subject));
    }

    #[test]
    fn test_mixed_wildcards() {
        let subject = ["agg123", "market456", "token789", "extra"];
        assert!(matches_subject("agg123.*.>", &subject));
        assert!(matches_subject("*.market456.>", &subject));
    }

    #[test]
    fn test_validate_pattern() {
        assert!(validate_pattern("agg123.market456.token789").is_none());
        assert!(validate_pattern("agg123.*.token789").is_none());
        assert!(validate_pattern("agg123.>").is_none());
        assert!(validate_pattern(">.agg123").is_some()); // Invalid: > not at end
        assert!(validate_pattern("").is_some()); // Invalid: empty
    }
}
