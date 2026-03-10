use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Severity level for an extracted finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl Severity {
    fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "critical" | "fatal" => Some(Self::Critical),
            "high" | "severe" | "major" => Some(Self::High),
            "medium" | "moderate" | "med" => Some(Self::Medium),
            "low" | "minor" => Some(Self::Low),
            "info" | "informational" | "note" | "nit" => Some(Self::Info),
            _ => None,
        }
    }
}

/// A discrete finding extracted from a model's unstructured response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Deterministic hash of (model_key, summary).
    pub finding_id: String,
    /// Config key of the model that produced this finding.
    pub model_key: String,
    /// Severity if the model indicated one.
    pub severity: Option<Severity>,
    /// One-line summary (the heading text).
    pub summary: String,
    /// Full body text under the heading.
    pub body: String,
    /// File path if the model cited one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Line range if the model cited lines (start, end).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<(u32, u32)>,
    /// Confidence if the model reported it (0.0–1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// Generate a deterministic finding ID from model key + summary.
fn finding_id(model_key: &str, summary: &str) -> String {
    let mut hasher = DefaultHasher::new();
    model_key.hash(&mut hasher);
    summary.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Extract structured findings from a model's free-text response.
///
/// Recognizes these heading patterns (case-insensitive):
/// - `### [severity] Title`
/// - `### N. Title (Confidence: High)`
/// - `### N. **Title** (Confidence: **High**)`
/// - `#### N. **Title** (Confidence: **High**)`
///
/// Body is all text between the heading and the next heading of same or higher level.
pub fn extract_findings(model_key: &str, response: &str) -> Vec<Finding> {
    let lines: Vec<&str> = response.lines().collect();
    let mut findings: Vec<Finding> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Match headings: ### or ####
        if let Some(rest) = line
            .strip_prefix("####")
            .or_else(|| line.strip_prefix("###"))
        {
            let rest = rest.trim();
            if rest.is_empty() {
                i += 1;
                continue;
            }

            let heading_level = if line.starts_with("####") { 4 } else { 3 };

            // Try to parse this heading as a finding
            if let Some((severity, summary, confidence)) = parse_heading(rest) {
                // Collect body: everything until next heading of same or higher level
                let body_start = i + 1;
                let mut body_end = body_start;
                while body_end < lines.len() {
                    let next = lines[body_end].trim();
                    if is_heading_at_level(next, heading_level) {
                        break;
                    }
                    body_end += 1;
                }

                let body = lines[body_start..body_end]
                    .to_vec()
                    .join("\n")
                    .trim()
                    .to_string();

                // Extract file path and line range from body
                let (file_path, line_range) = extract_file_ref(&body);

                // Extract confidence from body if not in heading
                let confidence = confidence.or_else(|| extract_confidence(&body));

                findings.push(Finding {
                    finding_id: finding_id(model_key, &summary),
                    model_key: model_key.to_string(),
                    severity,
                    summary,
                    body,
                    file_path,
                    line_range,
                    confidence,
                });
            }
        }

        i += 1;
    }

    findings
}

/// Check if a line is a heading at the given level or higher (lower number = higher).
fn is_heading_at_level(line: &str, level: usize) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    hashes >= 2 && hashes <= level && line.len() > hashes && line.as_bytes()[hashes] == b' '
}

/// Parse a heading into (severity, summary, confidence).
///
/// Patterns:
/// - `[critical] Title text` → severity from bracket
/// - `1. **Title** (Confidence: **High**)` → severity from confidence word
/// - `Title (Confidence: High)` → severity None, confidence parsed
/// - `The ML Algorithm Mismatch: GRPO vs. DPO (Fatal)` → severity from trailing paren
/// - `**Title** (Confidence: **99%**)` → confidence as number
fn parse_heading(heading: &str) -> Option<(Option<Severity>, String, Option<f64>)> {
    let heading = heading.trim();
    if heading.is_empty() {
        return None;
    }

    // Strip leading numbering: "1. ", "2. ", etc.
    let heading = strip_leading_number(heading);

    // Try bracket severity: [critical] Title
    if heading.starts_with('[')
        && let Some(bracket_end) = heading.find(']')
    {
        let sev_str = &heading[1..bracket_end];
        let severity = Severity::parse(sev_str);
        let rest = heading[bracket_end + 1..].trim().to_string();
        if !rest.is_empty() {
            let (summary, confidence) = extract_heading_confidence(&rest);
            return Some((severity, clean_summary(&summary), confidence));
        }
    }

    // Try trailing parenthetical: "Title (Fatal)" or "Title (Confidence: High)"
    let (summary, confidence, trailing_severity) = parse_trailing_paren(heading);
    let severity = trailing_severity;

    let summary = clean_summary(&summary);
    if summary.is_empty() {
        return None;
    }

    Some((severity, summary, confidence))
}

/// Strip leading "N. " or "N." numbering from a heading.
fn strip_leading_number(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    // Skip digits
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Skip ". " or "."
    if i > 0 && i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        if i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        &s[i..]
    } else {
        s
    }
}

/// Clean bold markers and extra whitespace from a summary.
fn clean_summary(s: &str) -> String {
    s.replace("**", "").trim().to_string()
}

/// Extract confidence from a trailing parenthetical.
/// Returns (summary_without_paren, confidence, severity_from_paren).
fn parse_trailing_paren(heading: &str) -> (String, Option<f64>, Option<Severity>) {
    // Find the last parenthesized group
    if let Some(paren_start) = heading.rfind('(')
        && heading.ends_with(')')
    {
        let paren_content = &heading[paren_start + 1..heading.len() - 1];
        let summary = heading[..paren_start].trim().to_string();

        // "Confidence: High" or "Confidence: **High**"
        let paren_clean = paren_content.replace("**", "");
        if let Some(conf_str) = paren_clean
            .strip_prefix("Confidence:")
            .or_else(|| paren_clean.strip_prefix("confidence:"))
        {
            let conf_str = conf_str.trim();
            let confidence = parse_confidence_value(conf_str);
            return (summary, confidence, None);
        }

        // Single severity word: "(Fatal)", "(High)", "(Critical)"
        let paren_clean_trimmed = paren_clean.trim();
        if let Some(sev) = Severity::parse(paren_clean_trimmed) {
            return (summary, None, Some(sev));
        }

        // Percentage: "(99%)"
        if let Some(pct) = paren_clean_trimmed.strip_suffix('%')
            && let Ok(n) = pct.trim().parse::<f64>()
        {
            return (summary, Some(n / 100.0), None);
        }
    }
    (heading.to_string(), None, None)
}

/// Extract confidence from a heading's trailing parenthetical.
fn extract_heading_confidence(s: &str) -> (String, Option<f64>) {
    if let Some(paren_start) = s.rfind('(')
        && s.ends_with(')')
    {
        let paren_content = &s[paren_start + 1..s.len() - 1].replace("**", "");
        if let Some(conf_str) = paren_content
            .strip_prefix("Confidence:")
            .or_else(|| paren_content.strip_prefix("confidence:"))
        {
            let summary = s[..paren_start].trim().to_string();
            let confidence = parse_confidence_value(conf_str.trim());
            return (summary, confidence);
        }
    }
    (s.to_string(), None)
}

/// Parse a confidence string like "High", "99%", "0.95" into f64.
fn parse_confidence_value(s: &str) -> Option<f64> {
    match s.to_lowercase().as_str() {
        "high" | "very high" => Some(0.9),
        "medium" | "moderate" | "med" => Some(0.6),
        "low" => Some(0.3),
        _ => {
            // Try percentage: "99%"
            if let Some(pct) = s.strip_suffix('%') {
                pct.trim().parse::<f64>().ok().map(|n| n / 100.0)
            } else {
                // Try raw float: "0.95"
                s.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| *n >= 0.0 && *n <= 1.0)
            }
        }
    }
}

/// Extract file path and optional line range from finding body text.
///
/// Patterns:
/// - `File: path/to/file.rs:42`
/// - `File: path/to/file.rs:42-50`
/// - `**File**: path/to/file.rs:42`
/// - `- File: path/to/file.rs:42`
/// - Inline backtick: `path/to/file.rs:42`
fn extract_file_ref(body: &str) -> (Option<String>, Option<(u32, u32)>) {
    // Pattern 1: explicit "File:" label
    for line in body.lines() {
        let line = line.trim().trim_start_matches('-').trim();
        let line = line.replace("**", "");
        if let Some(rest) = line
            .strip_prefix("File:")
            .or_else(|| line.strip_prefix("file:"))
        {
            let rest = rest.trim().trim_start_matches('`').trim_end_matches('`');
            return parse_file_with_lines(rest);
        }
    }

    // Pattern 2: backtick-quoted path with extension and line number
    for line in body.lines() {
        let line = line.trim();
        // Find `path/file.ext:NNN` pattern in backticks
        let mut start = 0;
        while let Some(tick_start) = line[start..].find('`') {
            let abs_start = start + tick_start + 1;
            if let Some(tick_end) = line[abs_start..].find('`') {
                let content = &line[abs_start..abs_start + tick_end];
                // Must look like a file path with extension and line number
                if content.contains('/')
                    && content.contains('.')
                    && content.contains(':')
                    && !content.contains(' ')
                {
                    return parse_file_with_lines(content);
                }
                start = abs_start + tick_end + 1;
            } else {
                break;
            }
        }
    }

    (None, None)
}

/// Parse "path/to/file.rs:42" or "path/to/file.rs:42-50" into (path, range).
fn parse_file_with_lines(s: &str) -> (Option<String>, Option<(u32, u32)>) {
    let s = s.trim();
    if let Some(colon_pos) = s.rfind(':') {
        let path = &s[..colon_pos];
        let line_part = &s[colon_pos + 1..];

        // Try range: "42-50"
        if let Some(dash_pos) = line_part.find('-') {
            let start = line_part[..dash_pos].trim().parse::<u32>().ok();
            let end = line_part[dash_pos + 1..].trim().parse::<u32>().ok();
            if let (Some(s), Some(e)) = (start, end) {
                return (Some(path.to_string()), Some((s, e)));
            }
        }

        // Try single line: "42"
        if let Ok(line) = line_part.trim().parse::<u32>() {
            return (Some(path.to_string()), Some((line, line)));
        }

        // Has colon but line part isn't numeric — still return path
        if !path.is_empty() && path.contains('.') {
            return (Some(path.to_string()), None);
        }
    }

    // No colon — bare path
    if !s.is_empty() && s.contains('.') && s.contains('/') {
        return (Some(s.to_string()), None);
    }

    (None, None)
}

/// Extract confidence from the body text (not heading).
///
/// Looks for "Confidence: High" or "**Confidence: 99%**" patterns.
fn extract_confidence(body: &str) -> Option<f64> {
    for line in body.lines() {
        let line = line.trim().replace("**", "");
        if let Some(rest) = line
            .strip_prefix("- Confidence:")
            .or_else(|| line.strip_prefix("Confidence:"))
            .or_else(|| line.strip_prefix("- confidence:"))
        {
            return parse_confidence_value(rest.trim());
        }
    }
    None
}

/// Persist extracted findings alongside the review results file.
///
/// Writes to `.squall/reviews/{review_stem}_findings.json`.
pub async fn persist_findings(
    results_file: &str,
    findings: &[Finding],
) -> Result<String, std::io::Error> {
    if findings.is_empty() {
        return Ok(String::new());
    }

    let results_path = PathBuf::from(results_file);
    let stem = results_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let findings_filename = format!("{stem}_findings.json");
    let findings_path = results_path
        .parent()
        .unwrap_or(&PathBuf::from(".squall/reviews"))
        .join(&findings_filename);

    let json = serde_json::to_string_pretty(findings).map_err(std::io::Error::other)?;

    // Atomic write
    let tmp_path = findings_path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp_path, json.as_bytes()).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &findings_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(findings_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_severity_finding() {
        let response = "\
### [critical] SQL injection in user input handler
- File: src/server.rs:42
- Detail: User input passed directly to query without sanitization.
- Confidence: High

### [low] Minor style issue
Tabs vs spaces inconsistency.
";
        let findings = extract_findings("grok", response);
        assert_eq!(findings.len(), 2);

        assert_eq!(findings[0].severity, Some(Severity::Critical));
        assert_eq!(findings[0].summary, "SQL injection in user input handler");
        assert_eq!(findings[0].file_path, Some("src/server.rs".to_string()));
        assert_eq!(findings[0].line_range, Some((42, 42)));
        assert_eq!(findings[0].confidence, Some(0.9));
        assert_eq!(findings[0].model_key, "grok");

        assert_eq!(findings[1].severity, Some(Severity::Low));
        assert_eq!(findings[1].summary, "Minor style issue");
    }

    #[test]
    fn numbered_heading_with_bold_and_confidence() {
        let response = "\
#### 1. **Invalid Mapping: Squall Memory ≠ ACT Training Signal** (Confidence: **High**)
   - ACT needs dense pairs. Squall reviews output holistic summaries.
   - File: src/memory/local.rs:150-200

#### 2. **Data Volume Insufficient** (Confidence: **Medium**)
   Not enough events for training.
";
        let findings = extract_findings("codex", response);
        assert_eq!(findings.len(), 2);

        assert_eq!(findings[0].severity, None);
        assert_eq!(
            findings[0].summary,
            "Invalid Mapping: Squall Memory ≠ ACT Training Signal"
        );
        assert_eq!(findings[0].confidence, Some(0.9));
        assert_eq!(
            findings[0].file_path,
            Some("src/memory/local.rs".to_string())
        );
        assert_eq!(findings[0].line_range, Some((150, 200)));

        assert_eq!(findings[1].summary, "Data Volume Insufficient");
        assert_eq!(findings[1].confidence, Some(0.6));
    }

    #[test]
    fn trailing_severity_in_parens() {
        let response = "\
### The ML Algorithm Mismatch: GRPO vs. DPO (Fatal)
GRPO is an online RL algorithm requiring policy to generate new responses.
Historical pairs are offline preference data = DPO.
";
        let findings = extract_findings("gemini", response);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Some(Severity::Critical));
        assert_eq!(
            findings[0].summary,
            "The ML Algorithm Mismatch: GRPO vs. DPO"
        );
    }

    #[test]
    fn percentage_confidence() {
        let response = "\
### 1. Algorithm Mismatch (99%)
Description of finding.
";
        let findings = extract_findings("gemini", response);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].confidence, Some(0.99));
    }

    #[test]
    fn backtick_file_ref() {
        let response = "\
### [high] Race condition in dispatch
The code at `src/review.rs:350` uses shared state without synchronization.
";
        let findings = extract_findings("kimi-k2.5", response);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file_path, Some("src/review.rs".to_string()));
        assert_eq!(findings[0].line_range, Some((350, 350)));
    }

    #[test]
    fn no_findings_in_prose() {
        let response = "\
This code looks good overall. The architecture is clean and well-organized.
I don't see any significant issues worth flagging.
";
        let findings = extract_findings("grok", response);
        assert_eq!(findings.len(), 0);
    }

    #[test]
    fn finding_ids_are_deterministic() {
        let response = "### [high] Bug\nDetails.";
        let f1 = extract_findings("grok", response);
        let f2 = extract_findings("grok", response);
        assert_eq!(f1[0].finding_id, f2[0].finding_id);

        // Different model = different ID
        let f3 = extract_findings("codex", response);
        assert_ne!(f1[0].finding_id, f3[0].finding_id);
    }

    #[test]
    fn mixed_heading_levels() {
        let response = "\
### Overview
This is a summary section, not a finding.

### [high] Real Finding
- File: src/lib.rs:10
- Detail: Important bug.

#### Sub-detail
More info about the finding above.

### [medium] Another Finding
Second issue.
";
        let findings = extract_findings("test", response);
        // All ### and #### headings become findings:
        // 0: "Overview" (severity=None), 1: "Real Finding" (high),
        // 2: "Sub-detail" (severity=None), 3: "Another Finding" (medium)
        assert_eq!(findings.len(), 4);
        assert_eq!(findings[0].severity, None);
        assert_eq!(findings[0].summary, "Overview");
        assert_eq!(findings[1].severity, Some(Severity::High));
        assert_eq!(findings[1].file_path, Some("src/lib.rs".to_string()));
        assert_eq!(findings[2].summary, "Sub-detail");
        assert_eq!(findings[3].severity, Some(Severity::Medium));
    }

    #[test]
    fn body_confidence_extraction() {
        let response = "\
### [high] Unsafe memory access
- File: src/dispatch/cli.rs:88
- Confidence: 95%
- Detail: Buffer overread possible.
";
        let findings = extract_findings("test", response);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].confidence, Some(0.95));
    }

    #[test]
    fn real_grok_response_format() {
        // Based on actual grok output from ACT review
        let response = "\
### Proof of Overengineering: Complexity Cost >> Value at Squall Scale

**Scale Context**: ~314 DuckDB events across ~12 models.

#### 1. **Invalid Mapping: Squall Memory ≠ ACT Training Signal** (Confidence: **High**)
   - ACT needs dense (expert_action, alternative_action) pairs.
   - File: src/memory/local.rs:100

#### 2. **GRPO Is Wrong Algorithm** (Confidence: **High**)
   GRPO requires online policy rollouts. Historical pairs = DPO.
";
        let findings = extract_findings("grok", response);
        // The ### heading has no bracket/confidence — parsed as a finding with no severity
        // The two #### headings are proper findings
        assert!(findings.len() >= 2);

        let invalid = findings
            .iter()
            .find(|f| f.summary.contains("Invalid Mapping"));
        assert!(invalid.is_some());
        let invalid = invalid.unwrap();
        assert_eq!(invalid.confidence, Some(0.9));
        assert_eq!(invalid.file_path, Some("src/memory/local.rs".to_string()));

        let grpo = findings.iter().find(|f| f.summary.contains("GRPO"));
        assert!(grpo.is_some());
    }

    #[test]
    fn real_gemini_response_format() {
        // Based on actual gemini output
        let response = "\
### 1. The ML Algorithm Mismatch: GRPO vs. DPO (Fatal)
**Confidence: 99%**

The proposal suggests using GRPO on exported pairs. This is a fundamental misunderstanding.

*   **The Seam**: GRPO is an *online* algorithm requiring policy to generate new responses.

### 2. Reward Signal Misspecified (High)
The `acted_on` label conflates correctness with urgency.
";
        let findings = extract_findings("gemini", response);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Some(Severity::Critical));
        assert_eq!(findings[0].confidence, Some(0.99));
        assert_eq!(findings[1].severity, Some(Severity::High));
    }

    #[tokio::test]
    async fn persist_findings_writes_json() {
        let dir = std::env::temp_dir().join(format!("squall_findings_test_{}", std::process::id()));
        let reviews_dir = dir.join(".squall/reviews");
        tokio::fs::create_dir_all(&reviews_dir).await.unwrap();

        let results_file = reviews_dir.join("123_456_0.json");
        tokio::fs::write(&results_file, "{}").await.unwrap();

        let findings = vec![Finding {
            finding_id: "abc123".to_string(),
            model_key: "grok".to_string(),
            severity: Some(Severity::High),
            summary: "Test finding".to_string(),
            body: "Details here.".to_string(),
            file_path: Some("src/lib.rs".to_string()),
            line_range: Some((10, 20)),
            confidence: Some(0.9),
        }];

        let path = persist_findings(results_file.to_str().unwrap(), &findings)
            .await
            .unwrap();
        assert!(path.contains("123_456_0_findings.json"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: Vec<Finding> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].summary, "Test finding");

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn persist_empty_findings_returns_empty() {
        let path = persist_findings(".squall/reviews/test.json", &[])
            .await
            .unwrap();
        assert!(path.is_empty());
    }
}
