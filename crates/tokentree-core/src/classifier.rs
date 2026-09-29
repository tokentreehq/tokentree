// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BoundaryOutcome {
    CONTINUE,
    CHILD,
    SWITCH,
    UNCERTAIN,
}

impl BoundaryOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CONTINUE => "CONTINUE",
            Self::CHILD => "CHILD",
            Self::SWITCH => "SWITCH",
            Self::UNCERTAIN => "UNCERTAIN",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BoundaryInput<'a> {
    pub text: &'a str,
    pub has_open_parent: bool,
    pub issue_id_changed: bool,
    pub explicit_parent_request: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoundaryResult {
    pub outcome: BoundaryOutcome,
    pub score: f64,
    pub signals: Vec<String>,
    pub derived_label: String,
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "the", "to", "for", "of", "in", "on", "that", "this", "please", "now", "also",
];

#[must_use]
pub fn redacted_label(text: &str) -> String {
    // 1. Redact secrets: sk-..., gh[pousr]_..., api_key/token/password = ...
    let mut cleaned = String::with_capacity(text.len());
    let words_raw: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < words_raw.len() {
        let w = words_raw[i];
        let lower = w.to_ascii_lowercase();
        let is_secret = (lower.starts_with("sk-") && w.len() >= 10)
            || ((lower.starts_with("ghp_")
                || lower.starts_with("gho_")
                || lower.starts_with("ghu_")
                || lower.starts_with("ghs_")
                || lower.starts_with("ghr_"))
                && w.len() >= 10)
            || ((lower.starts_with("api_key")
                || lower.starts_with("apikey")
                || lower.starts_with("token")
                || lower.starts_with("password"))
                && (lower.contains('=') || lower.contains(':')));

        if is_secret {
            cleaned.push_str(" [redacted] ");
        } else if (lower == "api_key" || lower == "token" || lower == "password")
            && i + 2 < words_raw.len()
            && (words_raw[i + 1] == "=" || words_raw[i + 1] == ":")
        {
            cleaned.push_str(" [redacted] ");
            i += 2;
        } else if w.starts_with("http://") || w.starts_with("https://") {
            // strip URLs
        } else {
            cleaned.push(' ');
            cleaned.push_str(w);
        }
        i += 1;
    }

    // 2. Filter non-alphanumeric chars except brackets and hyphen
    let mut filtered = String::with_capacity(cleaned.len());
    for c in cleaned.chars() {
        if c.is_alphanumeric() || c == '[' || c == ']' || c == '-' {
            filtered.push(c);
        } else {
            filtered.push(' ');
        }
    }

    // 3. Extract words, drop stop words, take first 8
    let mut tokens: Vec<String> = Vec::new();
    for word in filtered.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if word == "[redacted]" || !STOP_WORDS.contains(&lower.as_str()) {
            tokens.push(word.to_owned());
            if tokens.len() == 8 {
                break;
            }
        }
    }

    // 4. Pad with "work" to at least 3 words
    while tokens.len() < 3 {
        tokens.push("work".to_owned());
    }

    tokens.join(" ")
}

#[must_use]
pub fn classify_boundary(input: &BoundaryInput<'_>) -> BoundaryResult {
    let lower = input.text.to_ascii_lowercase();
    let mut signals = Vec::new();

    let has_switch_phrase = lower.contains("switch topic")
        || lower.contains("switch topics")
        || lower.contains("new task")
        || lower.contains("unrelated")
        || lower.contains("separately");

    if input.issue_id_changed || has_switch_phrase {
        signals.push(
            if input.issue_id_changed {
                "issue_id_changed"
            } else {
                "explicit_switch"
            }
            .to_owned(),
        );
        return BoundaryResult {
            outcome: BoundaryOutcome::SWITCH,
            score: 0.92,
            signals,
            derived_label: redacted_label(input.text),
        };
    }

    let has_child_phrase = lower.contains("regression test")
        || lower.contains("add test")
        || lower.contains("add a test")
        || lower.contains("document the fix")
        || lower.contains("document this fix");

    if input.has_open_parent && (has_child_phrase || input.explicit_parent_request) {
        signals.push("open_parent".to_owned());
        signals.push("bounded_follow_up".to_owned());
        return BoundaryResult {
            outcome: BoundaryOutcome::CHILD,
            score: 0.90,
            signals,
            derived_label: redacted_label(input.text),
        };
    }

    let has_continue_phrase = lower.contains("also")
        || lower.contains("continue")
        || lower.contains("same")
        || lower.contains("that")
        || lower.contains("it")
        || lower.contains("nearby")
        || lower.contains("follow up")
        || lower.contains("follow-up")
        || lower.contains("followup");

    if has_continue_phrase {
        signals.push("continuity_language".to_owned());
        return BoundaryResult {
            outcome: BoundaryOutcome::CONTINUE,
            score: 0.82,
            signals,
            derived_label: redacted_label(input.text),
        };
    }

    signals.push("insufficient_boundary_evidence".to_owned());
    BoundaryResult {
        outcome: BoundaryOutcome::UNCERTAIN,
        score: 0.45,
        signals,
        derived_label: redacted_label(input.text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_and_pads_to_length() {
        let label = redacted_label("Here is token=sk-1234567890abcdef and password=secret12345");
        assert!(label.contains("[redacted]"));
        assert!(!label.contains("secret12345"));
        let word_count = label.split_whitespace().count();
        assert!((3..=8).contains(&word_count));
    }

    #[test]
    fn classifies_switch_on_new_task() {
        let res = classify_boundary(&BoundaryInput {
            text: "Let's switch topics and work on authentication",
            has_open_parent: true,
            issue_id_changed: false,
            explicit_parent_request: false,
        });
        assert_eq!(res.outcome, BoundaryOutcome::SWITCH);
    }

    #[test]
    fn classifies_child_on_regression_test_with_parent() {
        let res = classify_boundary(&BoundaryInput {
            text: "Add a regression test for the collision bug",
            has_open_parent: true,
            issue_id_changed: false,
            explicit_parent_request: false,
        });
        assert_eq!(res.outcome, BoundaryOutcome::CHILD);
    }

    #[test]
    fn classifies_continue_on_follow_up() {
        let res = classify_boundary(&BoundaryInput {
            text: "Also fix the color of that button",
            has_open_parent: false,
            issue_id_changed: false,
            explicit_parent_request: false,
        });
        assert_eq!(res.outcome, BoundaryOutcome::CONTINUE);
    }
}
