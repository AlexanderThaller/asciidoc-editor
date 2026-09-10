//! Line-addressed edits on the AsciiDoc source.
//!
//! Every block in the rendered document carries the line it came from, so an
//! edit made through the rich-text surface is expressed as "replace these
//! source lines with this text". Keeping that translation here — away from the
//! DOM — makes it testable.

/// A 1-based, inclusive range of source lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

impl LineRange {
    pub fn single(line: usize) -> Self {
        Self {
            start: line,
            end: line,
        }
    }

    pub fn line_count(&self) -> usize {
        self.end + 1 - self.start
    }
}

/// The lines of the paragraph beginning at `start`.
///
/// A paragraph runs until the next blank line or the end of the document,
/// which is exactly the span the rich-text surface replaces when the paragraph
/// is edited.
pub fn paragraph_range(src: &str, start: usize) -> LineRange {
    let mut end = start;

    for (index, line) in src.lines().enumerate().skip(start) {
        if line.trim().is_empty() {
            break;
        }
        end = index + 1;
    }

    LineRange { start, end }
}

/// The source text of `range`.
pub fn text_of(src: &str, range: LineRange) -> String {
    src.lines()
        .skip(range.start - 1)
        .take(range.line_count())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces `range` with `replacement`, which may span several lines.
pub fn replace(src: &str, range: LineRange, replacement: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut replaced = false;

    for (index, line) in src.lines().enumerate() {
        let number = index + 1;
        if number < range.start || number > range.end {
            out.push(line);
        } else if !replaced {
            out.extend(replacement.split('\n'));
            replaced = true;
        }
    }

    if !replaced {
        out.extend(replacement.split('\n'));
    }

    restore_trailing_newline(out.join("\n"), src)
}

/// Inserts `text` as a new block starting at `line`, followed by a blank line.
///
/// Used when the rich-text surface creates a paragraph that has no source of
/// its own yet.
pub fn insert_block(src: &str, line: usize, text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut inserted = false;

    for (index, existing) in src.lines().enumerate() {
        if index + 1 == line {
            out.extend(text.split('\n'));
            out.push("");
            inserted = true;
        }
        out.push(existing);
    }

    if !inserted {
        // Past the end of the document: separate the new block from the last one.
        if !out.is_empty() && !out.last().is_some_and(|last| last.trim().is_empty()) {
            out.push("");
        }
        out.extend(text.split('\n'));
    }

    restore_trailing_newline(out.join("\n"), src)
}

fn restore_trailing_newline(mut text: String, src: &str) -> String {
    if src.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The heading level of a section title line, if it is one.
pub fn heading_level(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|c| *c == '=').count();

    ((1..=6).contains(&level) && line[level..].starts_with(' ')).then_some(level)
}

/// The title text of a section title line, without its `=` prefix.
pub fn heading_text(line: &str) -> &str {
    match heading_level(line) {
        Some(level) => line[level..].trim_start(),
        None => line,
    }
}

/// Renders `text` as a heading of `level`, or as body text when `None`.
///
/// Headings occupy a single line, so any line breaks in the text are folded
/// into spaces.
pub fn as_block(text: &str, level: Option<usize>) -> String {
    match level {
        Some(level) => format!("{} {}", "=".repeat(level), text.replace('\n', " ").trim()),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "= Title\n\nFirst para\nsecond line\n\n== Section\n\nTail\n";

    #[test]
    fn paragraph_extends_to_the_next_blank_line() {
        assert_eq!(paragraph_range(DOC, 3), LineRange { start: 3, end: 4 });
        assert_eq!(paragraph_range(DOC, 8), LineRange { start: 8, end: 8 });
    }

    #[test]
    fn reads_the_text_of_a_range() {
        assert_eq!(text_of(DOC, LineRange { start: 3, end: 4 }), "First para\nsecond line");
        assert_eq!(text_of(DOC, LineRange::single(6)), "== Section");
    }

    #[test]
    fn replaces_a_multi_line_range_with_one_line() {
        let edited = replace(DOC, LineRange { start: 3, end: 4 }, "Rewritten");

        assert_eq!(edited, "= Title\n\nRewritten\n\n== Section\n\nTail\n");
    }

    #[test]
    fn replaces_one_line_with_several() {
        let edited = replace(DOC, LineRange::single(8), "a\n\nb");

        assert_eq!(edited, "= Title\n\nFirst para\nsecond line\n\n== Section\n\na\n\nb\n");
    }

    #[test]
    fn preserves_whether_the_document_ends_in_a_newline() {
        assert!(replace("a\n", LineRange::single(1), "b").ends_with('\n'));
        assert!(!replace("a", LineRange::single(1), "b").ends_with('\n'));
    }

    #[test]
    fn inserts_a_block_before_a_line() {
        let edited = insert_block(DOC, 6, "New para");

        assert_eq!(edited, "= Title\n\nFirst para\nsecond line\n\nNew para\n\n== Section\n\nTail\n");
    }

    #[test]
    fn inserts_a_block_past_the_end() {
        assert_eq!(insert_block("a\n", 99, "b"), "a\n\nb\n");
    }

    #[test]
    fn recognises_heading_lines() {
        assert_eq!(heading_level("== Section"), Some(2));
        assert_eq!(heading_level("====== Deep"), Some(6));
        assert_eq!(heading_level("======= Too deep"), None);
        assert_eq!(heading_level("==Section"), None, "a space is required");
        assert_eq!(heading_level("Body text"), None);
    }

    #[test]
    fn strips_and_applies_heading_prefixes() {
        assert_eq!(heading_text("=== Deep section"), "Deep section");
        assert_eq!(heading_text("Body text"), "Body text");
        assert_eq!(as_block("Title", Some(2)), "== Title");
        assert_eq!(as_block("Title", None), "Title");
        assert_eq!(as_block("Two\nlines", Some(1)), "= Two lines");
    }
}
