use chrono::{DateTime, Utc};
use regex::Regex;
use std::fs;
use std::path::Path;

pub struct Backlog {
    pub content: String,
    pub file_path: std::path::PathBuf,
}

impl Backlog {
    /// Produce a diagnostic error when `find_ticket_section` returns None.
    /// Checks whether the ticket string exists at all (format mismatch) vs truly absent.
    fn ticket_not_found_error(&self, ticket_id: &str) -> String {
        let ticket_id_upper = ticket_id.to_uppercase();
        let needle = format!("Ticket {}", ticket_id_upper);
        let file_display = self.file_path.display();

        if self.content.contains(&needle) {
            format!(
                "Ticket {} found in {} but not as a markdown heading. \
                 Expected a line like '## Ticket {}' (H2-H6). \
                 Check the heading level and format.",
                ticket_id_upper, file_display, ticket_id_upper,
            )
        } else {
            format!(
                "Ticket {} not found in backlog file {}. \
                 Ensure the ticket exists with a heading like '## Ticket {}'.",
                ticket_id_upper, file_display, ticket_id_upper,
            )
        }
    }
}

impl Backlog {
    pub fn read(path: &Path) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read backlog file: {}", e))?;
        Ok(Self {
            content,
            file_path: path.to_path_buf(),
        })
    }

    pub fn write(&self) -> Result<(), String> {
        fs::write(&self.file_path, &self.content)
            .map_err(|e| format!("Failed to write backlog file: {}", e))?;
        Ok(())
    }

    fn find_ticket_section(&self, ticket_id: &str) -> Option<(usize, usize)> {
        let ticket_id_upper = ticket_id.to_uppercase();
        // Match any heading level (##, ###, etc.) for resilience
        let header_pattern = format!(
            r"(?m)^(#{{2,6}}) Ticket {}[^\n]*$",
            regex::escape(&ticket_id_upper)
        );
        let header_re = Regex::new(&header_pattern).ok()?;

        let caps = header_re.captures(&self.content)?;
        let header_match = caps.get(0)?;
        let heading_level = caps.get(1)?.as_str();
        let section_start = header_match.start();

        // Next section at same or higher heading level
        let next_section_pattern = format!(r"(?m)^#{{2,{}}} ", heading_level.len());
        let next_section = Regex::new(&next_section_pattern).ok()?;

        let search_from = header_match.end();
        let section_end = next_section
            .find_at(&self.content, search_from)
            .map(|m| m.start())
            .unwrap_or(self.content.len());

        Some((section_start, section_end))
    }

    fn find_metrics_entry(&self, ticket_id: &str) -> Option<(usize, usize)> {
        let ticket_id_upper = ticket_id.to_uppercase();
        let pattern = format!(r"(?m)^- Ticket: {}$", regex::escape(&ticket_id_upper));
        let re = Regex::new(&pattern).ok()?;

        let entry_start = re.find(&self.content)?.start();

        let next_entry = Regex::new(r"(?m)^- Ticket:").ok()?;
        let search_from = entry_start + 1;
        let entry_end = next_entry
            .find_at(&self.content, search_from)
            .map(|m| m.start())
            .unwrap_or_else(|| {
                Regex::new(r"(?m)^## ")
                    .ok()
                    .and_then(|r| r.find_at(&self.content, search_from))
                    .map(|m| m.start())
                    .unwrap_or(self.content.len())
            });

        Some((entry_start, entry_end))
    }

    pub fn update_status(&mut self, ticket_id: &str, status: &str) -> Result<(), String> {
        let (section_start, section_end) = self
            .find_ticket_section(ticket_id)
            .ok_or_else(|| self.ticket_not_found_error(ticket_id))?;

        let section = &self.content[section_start..section_end];
        let status_re =
            Regex::new(r"(?m)^- Status: .*$").map_err(|e| format!("Regex error: {}", e))?;

        let new_status_line = format!("- Status: {}", status);

        if let Some(cap) = status_re.find(section) {
            let old_line_start = section_start + cap.start();
            let old_line_end = section_start + cap.end();
            self.content
                .replace_range(old_line_start..old_line_end, &new_status_line);
            Ok(())
        } else {
            Err(format!("Status field not found for ticket {}", ticket_id))
        }
    }

    pub fn get_status(&self, ticket_id: &str) -> Result<String, String> {
        let (section_start, section_end) = self
            .find_ticket_section(ticket_id)
            .ok_or_else(|| self.ticket_not_found_error(ticket_id))?;

        let section = &self.content[section_start..section_end];
        let status_re =
            Regex::new(r"(?m)^- Status: (.*)$").map_err(|e| format!("Regex error: {}", e))?;

        if let Some(cap) = status_re.captures(section) {
            Ok(cap[1].trim().to_string())
        } else {
            Err(format!("Status field not found for ticket {}", ticket_id))
        }
    }

    pub fn validate_required_ticket_schema(&self, ticket_id: &str) -> Result<(), String> {
        let (section_start, section_end) = self
            .find_ticket_section(ticket_id)
            .ok_or_else(|| self.ticket_not_found_error(ticket_id))?;

        let section = &self.content[section_start..section_end];
        let required_fields = [
            "Goal",
            "In scope",
            "Out of scope",
            "Dependencies",
            "Acceptance criteria",
            "Verification",
        ];

        let mut missing = Vec::new();
        for field in required_fields {
            let field_pattern = format!(r"(?m)^- {}:", regex::escape(field));
            let field_re = Regex::new(&field_pattern).map_err(|e| format!("Regex error: {}", e))?;
            if !field_re.is_match(section) {
                missing.push(field);
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Ticket {} is missing required fields: {}",
                ticket_id,
                missing.join(", ")
            ))
        }
    }

    pub fn update_metric(
        &mut self,
        ticket_id: &str,
        metric: &str,
        value: &str,
    ) -> Result<(), String> {
        let (entry_start, entry_end) = self
            .find_metrics_entry(ticket_id)
            .ok_or_else(|| format!("Metrics entry not found for ticket {}", ticket_id))?;

        let entry = &self.content[entry_start..entry_end];
        let metric_pattern = format!(r"(?m)^- {}:\s*.*$", regex::escape(metric));
        let metric_re = Regex::new(&metric_pattern).map_err(|e| format!("Regex error: {}", e))?;

        let new_line = format!("- {}: {}", metric, value);

        if let Some(cap) = metric_re.find(entry) {
            let abs_start = entry_start + cap.start();
            let abs_end = entry_start + cap.end();
            self.content.replace_range(abs_start..abs_end, &new_line);
            Ok(())
        } else {
            let insert_pos = entry_end;
            self.content
                .insert_str(insert_pos, &format!("\n- {}: {}\n\n", metric, value));
            Ok(())
        }
    }

    pub fn set_start_time(&mut self, ticket_id: &str, time: DateTime<Utc>) -> Result<(), String> {
        let time_str = time.format("%Y-%m-%d %H:%M UTC").to_string();
        self.update_metric(ticket_id, "Start", &time_str)
    }

    pub fn set_end_time(&mut self, ticket_id: &str, time: DateTime<Utc>) -> Result<(), String> {
        let time_str = time.format("%Y-%m-%d %H:%M UTC").to_string();
        self.update_metric(ticket_id, "End", &time_str)
    }

    pub fn set_duration(&mut self, ticket_id: &str, duration: &str) -> Result<(), String> {
        self.update_metric(ticket_id, "Duration", duration)
    }

    pub fn add_tracking_note(&mut self, ticket_id: &str, note: &str) -> Result<(), String> {
        let tracking_section_re =
            Regex::new(r"(?m)^## Tracking Notes\n").map_err(|e| format!("Regex error: {}", e))?;

        // Matches legacy section names that may follow Tracking Notes in old-format
        // backlogs. For new-format backlogs that omit these sections the pattern
        // simply returns None, and the fallback `self.content.len()` is used.
        let next_section_re = Regex::new(r"(?m)^## (Execution Metrics|Closure Evidence)")
            .map_err(|e| format!("Regex error: {}", e))?;

        let insert_pos = if let Some(m) = tracking_section_re.find(&self.content) {
            let search_from = m.end();
            next_section_re
                .find_at(&self.content, search_from)
                .map(|n| n.start())
                .unwrap_or(self.content.len())
        } else {
            let end_pos = self.content.len();
            self.content.push_str("\n## Tracking Notes\n\n");
            end_pos + 21
        };

        let note_line = format!("- [{}] {}\n", ticket_id.to_uppercase(), note);
        self.content.insert_str(insert_pos, &note_line);

        Ok(())
    }

    pub fn ensure_metrics_entry(&mut self, ticket_id: &str) -> Result<(), String> {
        if self.find_metrics_entry(ticket_id).is_some() {
            return Ok(());
        }

        let metrics_section_re = Regex::new(r"(?m)^## Execution Metrics\n")
            .map_err(|e| format!("Regex error: {}", e))?;
        let next_section_re = Regex::new(r"(?m)^## ").map_err(|e| format!("Regex error: {}", e))?;

        let entry = format!(
            "- Ticket: {}\n- Owner: (pending)\n- Complexity: (pending)\n- Risk: (pending)\n- Start: (pending)\n- End: (pending)\n- Duration: (pending)\n- Notes: (pending)\n\n",
            ticket_id.to_uppercase()
        );

        if let Some(m) = metrics_section_re.find(&self.content) {
            let section_start = m.end();
            let section_end = next_section_re
                .find_at(&self.content, section_start)
                .map(|next| next.start())
                .unwrap_or(self.content.len());
            let replacement = format!("\n{}", entry);
            self.content
                .replace_range(section_start..section_end, &replacement);
        } else {
            self.content.push_str("\n## Execution Metrics\n\n");
            self.content.push_str(&entry);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_backlog() -> String {
        [
            "## Ticket TEST-1 — One",
            "- Goal: Complete feature",
            "- In scope:",
            "  - parser",
            "- Out of scope:",
            "  - runtime",
            "- Dependencies: none",
            "- Acceptance criteria:",
            "  - tests pass",
            "- Verification:",
            "  - cargo test",
            "- Status: Done",
            "",
            "## Ticket TEST-2 — Two",
            "- Goal: Add diagnostics",
            "- In scope:",
            "  - typechecker",
            "- Out of scope:",
            "  - vm",
            "- Dependencies: TEST-1",
            "- Acceptance criteria:",
            "  - clear error",
            "- Verification:",
            "  - cargo test",
            "- Status: Todo",
            "",
            "## Tracking Notes",
            "",
            "## Execution Metrics",
            "",
            "## Closure Evidence",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn test_get_status_reads_only_target_ticket_section() {
        let backlog = Backlog {
            content: sample_backlog(),
            file_path: std::path::PathBuf::from("unused.md"),
        };

        let status = backlog.get_status("TEST-2").expect("read status");
        assert_eq!(status, "Todo");
    }

    #[test]
    fn test_update_status_changes_only_target_ticket() {
        let mut backlog = Backlog {
            content: sample_backlog(),
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .update_status("TEST-2", "In Progress")
            .expect("update status");

        assert_eq!(backlog.get_status("TEST-1").expect("status"), "Done");
        assert_eq!(backlog.get_status("TEST-2").expect("status"), "In Progress");
    }

    #[test]
    fn test_add_tracking_note_prefixes_ticket_id() {
        let mut backlog = Backlog {
            content: sample_backlog(),
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .add_tracking_note("test-2", "blocked by dependency")
            .expect("add note");

        assert!(backlog.content.contains("- [TEST-2] blocked by dependency"));
    }

    #[test]
    fn test_validate_required_ticket_schema_success() {
        let backlog = Backlog {
            content: sample_backlog(),
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .validate_required_ticket_schema("TEST-2")
            .expect("schema should be valid");
    }

    #[test]
    fn test_validate_required_ticket_schema_reports_missing_fields() {
        let content = [
            "## Ticket TEST-3 — Missing Fields",
            "- Goal: Incomplete ticket",
            "- Status: Todo",
            "",
        ]
        .join("\n");
        let backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("unused.md"),
        };

        let err = backlog
            .validate_required_ticket_schema("TEST-3")
            .expect_err("schema should fail");
        assert!(err.contains("In scope"));
        assert!(err.contains("Out of scope"));
        assert!(err.contains("Dependencies"));
        assert!(err.contains("Acceptance criteria"));
        assert!(err.contains("Verification"));
    }

    #[test]
    fn test_update_metric_preserves_entry_separation() {
        let content = [
            "## Execution Metrics",
            "",
            "- Ticket: TEST-1",
            "- Start: 2024-01-01 10:00 UTC",
            "- End: 2024-01-01 10:30 UTC",
            "",
            "- Ticket: TEST-2",
            "- Start: 2024-01-01 11:00 UTC",
            "",
        ]
        .join("\n");

        let mut backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .update_metric("TEST-1", "Duration", "00:30:00")
            .expect("update metric");

        assert!(!backlog.content.contains("00:30:00- Ticket: TEST-2"));
        assert!(backlog
            .content
            .contains("- Duration: 00:30:00\n\n- Ticket: TEST-2"));

        backlog
            .update_metric("TEST-1", "Notes", "completed successfully")
            .expect("update metric");
        assert!(!backlog.content.contains("successfully- Ticket: TEST-2"));
        assert!(backlog
            .content
            .contains("- Notes: completed successfully\n\n- Ticket: TEST-2"));
    }

    #[test]
    fn test_update_metric_replaces_blank_placeholder_value() {
        let content = [
            "## Execution Metrics",
            "",
            "- Ticket: TEST-1",
            "- Start:",
            "- End:",
            "",
        ]
        .join("\n");

        let mut backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .update_metric("TEST-1", "Start", "2026-03-29 03:29 UTC")
            .expect("update start");

        assert!(backlog.content.contains("- Start: 2026-03-29 03:29 UTC"));
        assert!(!backlog
            .content
            .contains("- Start:\n- End:\n- Start: 2026-03-29 03:29 UTC"));
    }

    #[test]
    fn test_ensure_metrics_entry_inserts_after_blank_line() {
        let content = ["## Execution Metrics", "", "## Closure Evidence", ""].join("\n");
        let mut backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .ensure_metrics_entry("TEST-3")
            .expect("ensure metrics entry");

        assert!(backlog
            .content
            .contains("## Execution Metrics\n\n- Ticket: TEST-3"));
    }

    #[test]
    fn test_ensure_metrics_entry_does_not_append_extra_blank_line() {
        let content = ["## Execution Metrics", "", "## Closure Evidence", ""].join("\n");
        let mut backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("unused.md"),
        };

        backlog
            .ensure_metrics_entry("TEST-3")
            .expect("ensure metrics entry");

        assert!(backlog
            .content
            .contains("- Notes: (pending)\n\n## Closure Evidence"));
        assert!(!backlog
            .content
            .contains("- Notes: (pending)\n\n\n## Closure Evidence"));
    }

    #[test]
    fn test_h3_tickets_are_found() {
        let content = [
            "## Tickets",
            "",
            "### Ticket TEST-H3 — H3 Heading",
            "- Goal: Test H3 support",
            "- In scope:",
            "  - backlog parser",
            "- Out of scope:",
            "  - nothing",
            "- Dependencies: none",
            "- Acceptance criteria:",
            "  - H3 tickets resolve",
            "- Verification:",
            "  - cargo test",
            "- Status: Todo",
            "",
        ]
        .join("\n");
        let backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("test.md"),
        };

        let status = backlog
            .get_status("TEST-H3")
            .expect("should find H3 ticket");
        assert_eq!(status, "Todo");
    }

    #[test]
    fn test_h3_ticket_update_status() {
        let content = ["### Ticket TEST-H3 — H3 Heading", "- Status: Todo", ""].join("\n");
        let mut backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("test.md"),
        };

        backlog
            .update_status("TEST-H3", "In Progress")
            .expect("should update H3 ticket");
        assert_eq!(
            backlog.get_status("TEST-H3").expect("status"),
            "In Progress"
        );
    }

    #[test]
    fn test_diagnostic_error_format_mismatch() {
        let content = "- References: Ticket GHOST-1 for context\n".to_string();
        let backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("backlog.md"),
        };

        let err = backlog.get_status("GHOST-1").expect_err("should fail");
        assert!(
            err.contains("not as a markdown heading"),
            "error should explain format mismatch, got: {}",
            err
        );
        assert!(err.contains("backlog.md"), "error should name the file");
    }

    #[test]
    fn test_diagnostic_error_ticket_absent() {
        let content = "## Ticket OTHER-1 — Unrelated\n- Status: Done\n".to_string();
        let backlog = Backlog {
            content,
            file_path: std::path::PathBuf::from("backlog.md"),
        };

        let err = backlog.get_status("MISSING-99").expect_err("should fail");
        assert!(
            err.contains("not found in backlog file"),
            "error should say not found, got: {}",
            err
        );
        assert!(err.contains("backlog.md"), "error should name the file");
    }
}
