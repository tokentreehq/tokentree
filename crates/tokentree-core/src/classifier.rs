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

/// True when `word` appears as a standalone token in `haystack` — bounded on
/// both sides by a non-alphanumeric character or string edge — rather than as
/// a substring of a larger word (e.g. "it" in "commit" must not match).
#[must_use]
fn contains_word(haystack: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(relative) = haystack[start..].find(word) {
        let abs_start = start + relative;
        let abs_end = abs_start + word.len();
        let before_ok = abs_start == 0
            || !haystack[..abs_start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        let after_ok = abs_end >= haystack.len()
            || !haystack[abs_end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = abs_start + 1;
    }
    false
}

#[must_use]
/// Derive a redacted 3–8 word label from text.
///
/// # Privacy contract (P1)
///
/// This function takes raw prompt-adjacent text and emits prompt fragments
/// (first non-stop-words). In this product it must NEVER be fed raw prompts,
/// and its output must NEVER be persisted to the prompt-derived columns
/// `classification_events.extracted_intent` / `rationale` — doing so would
/// leak prompt content into the ledger. The only sanctioned persisted forms
/// are fingerprints and redacted labels on non-prompt-derived columns, per
/// the privacy section of CLAUDE.md. A CI guard
/// (`classifier_privacy_guard_test.rs`) fails the build if any production
/// INSERT/UPDATE targets those columns.
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

    let has_negative_switch = lower.contains("switch statement")
        || lower.contains("switch case")
        || lower.contains("switch expression")
        || lower.contains("switch block")
        || lower.contains("switch syntax")
        || lower.contains("switch branch");

    let has_switch_phrase = !has_negative_switch
        && (lower.contains("switch topics")
            || lower.contains("switch to")
            || lower.contains("switch gears")
            || lower.contains("switch context")
            || lower.contains("new task")
            || lower.contains("unrelated")
            || lower.contains("separately")
            || lower.contains("start working on")
            || lower.contains("pause this"));

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
        || lower.contains("document this fix")
        || lower.contains("benchmark")
        || lower.contains("microbenchmark")
        || lower.contains("extract")
        || lower.contains("investigate")
        || lower.contains("profile");

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
        || contains_word(&lower, "it")
        || lower.contains("nearby")
        || lower.contains("follow up")
        || lower.contains("follow-up")
        || lower.contains("followup")
        || lower.contains(" too")
        || lower.contains("above")
        || lower.contains("keep going")
        || lower.contains("finish")
        || has_negative_switch;

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

    #[test]
    fn contains_word_respects_word_boundaries() {
        assert!(contains_word("fix it now", "it"));
        assert!(contains_word("it works", "it"));
        assert!(contains_word("do it.", "it"));
        assert!(contains_word("it's fine", "it"));
        assert!(!contains_word("commit the fix", "it"));
        assert!(!contains_word("split item", "it"));
        assert!(!contains_word("with", "it"));
        assert!(!contains_word("", "it"));
        assert!(!contains_word("anything", ""));
    }

    #[test]
    fn it_substring_no_longer_forces_continue() {
        // H12: "commit" contains "it" but carries no continuity signal.
        let res = classify_boundary(&BoundaryInput {
            text: "commit the fix with tests",
            has_open_parent: false,
            issue_id_changed: false,
            explicit_parent_request: false,
        });
        assert_eq!(res.outcome, BoundaryOutcome::UNCERTAIN);
    }

    #[test]
    fn standalone_it_still_signals_continue() {
        let res = classify_boundary(&BoundaryInput {
            text: "also fix it tomorrow",
            has_open_parent: false,
            issue_id_changed: false,
            explicit_parent_request: false,
        });
        assert_eq!(res.outcome, BoundaryOutcome::CONTINUE);
    }

    #[test]
    fn test_dynamic_boundary_evaluation_on_eval_fixture() {
        #[derive(serde::Deserialize)]
        struct EvalRow {
            id: String,
            sanitized_input: String,
            expected: String,
            parent_evidence: Option<bool>,
            issue_id_changed: Option<bool>,
        }

        let eval_jsonl = include_str!("../../../fixtures/boundaries/eval.jsonl");
        let mut total = 0;
        let mut correct = 0;

        for line in eval_jsonl.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let row: EvalRow = serde_json::from_str(line).expect("valid eval JSON");
            total += 1;

            let res = classify_boundary(&BoundaryInput {
                text: &row.sanitized_input,
                has_open_parent: row.parent_evidence.unwrap_or(false),
                issue_id_changed: row.issue_id_changed.unwrap_or(false),
                explicit_parent_request: false,
            });

            if res.outcome.as_str() == row.expected {
                correct += 1;
            } else {
                eprintln!(
                    "Mismatch on {}: expected {}, got {}",
                    row.id,
                    row.expected,
                    res.outcome.as_str()
                );
            }
        }

        assert!(total >= 10, "Expected at least 10 evaluation samples");
        let accuracy = correct as f64 / total as f64;
        let error_rate = (total - correct) as f64 / total as f64;
        let acc_pct = accuracy * 100.0;
        let err_pct = error_rate * 100.0;
        println!(
            "Rust Dynamic Boundary Evaluation: Accuracy={acc_pct:.1}%, ErrorRate={err_pct:.1}%"
        );
        assert!(
            accuracy >= 0.90,
            "Accuracy {acc_pct:.1}% below 90% threshold"
        );
        assert!(
            error_rate <= 0.10,
            "Error rate {err_pct:.1}% exceeds 10% threshold"
        );
    }
}
