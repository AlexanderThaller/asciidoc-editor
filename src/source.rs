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

/// The lines of the block's own content, beginning at or below `start`.
///
/// A block's recorded line points at the first line attached to it, which may
/// be a title or an attribute list rather than its content; those are skipped.
/// The content then runs to the next blank line or the end of the document,
/// which is the span the rich-text surface replaces when the block is edited.
pub fn paragraph_range(src: &str, start: usize) -> LineRange {
    let mut start = start;

    while is_attached(&text_of(src, LineRange::single(start))) {
        start += 1;
    }

    let mut end = start;
    for (index, line) in src.lines().enumerate().skip(start) {
        if line.trim().is_empty() {
            break;
        }
        end = index + 1;
    }

    LineRange { start, end }
}

/// Whether the line belongs to the block below it rather than being content:
/// a block title or an attribute list.
fn is_attached(line: &str) -> bool {
    is_block_title(line) || (line.starts_with('[') && line.trim_end().ends_with(']'))
}

/// Inserts `text` as a line of its own before `line`.
pub fn insert_line(src: &str, line: usize, text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut inserted = false;

    for (index, existing) in src.lines().enumerate() {
        if index + 1 == line {
            out.push(text);
            inserted = true;
        }
        out.push(existing);
    }

    if !inserted {
        out.push(text);
    }

    restore_trailing_newline(out.join("\n"), src)
}

/// Removes `line`.
pub fn remove_line(src: &str, line: usize) -> String {
    let kept: Vec<&str> = src
        .lines()
        .enumerate()
        .filter(|(index, _)| index + 1 != line)
        .map(|(_, existing)| existing)
        .collect();

    restore_trailing_newline(kept.join("\n"), src)
}

/// The marker a list item line starts with, if it is one.
///
/// Repetition carries nesting depth in AsciiDoc (`**` is a second-level item),
/// so the whole run is returned. `1.`-style numbering is deliberately not
/// recognised: it cannot be written back without renumbering.
pub fn list_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let marker = &trimmed[..trimmed
        .find(|c| !matches!(c, '*' | '-' | '.'))
        .unwrap_or(trimmed.len())];

    let first = marker.chars().next()?;
    if !marker.chars().all(|c| c == first) {
        return None;
    }

    // A marker is only a marker when something follows it: `.Title` is a block
    // title and `----` is a delimiter.
    let rest = &trimmed[marker.len()..];
    (rest.starts_with(' ') && !rest.trim().is_empty()).then_some(marker)
}

/// Admonition labels, in the inline form `NOTE: text`.
pub const ADMONITIONS: [&str; 5] = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];

/// The canonical label `text` begins with, whether that is a bare label or a
/// whole lead such as `NOTE: `.
pub fn admonition_label(text: &str) -> Option<&'static str> {
    ADMONITIONS
        .iter()
        .copied()
        .find(|label| text.starts_with(label))
}

/// The label an inline admonition starts with, including its separator.
///
/// The label lives in the source but not in the rendered content, so editing
/// an admonition has to put it back.
pub fn admonition_lead(line: &str) -> Option<&str> {
    ADMONITIONS.iter().find_map(|label| {
        line.get(..label.len() + 2)
            .filter(|lead| lead.starts_with(label) && lead.ends_with(": "))
    })
}

/// Whether the line is a block title (`.Title`).
pub fn is_block_title(line: &str) -> bool {
    line.starts_with('.')
        && !line.starts_with("..")
        && list_marker(line).is_none()
        && line.len() > 1
}

/// The line holding the title attached to the block starting at `start`.
///
/// A title sits with the block's attribute lines, above its content, and the
/// recorded line points at whichever of those comes first.
pub fn block_title_line(src: &str, start: usize) -> Option<usize> {
    for line_number in start..start + 4 {
        let line = text_of(src, LineRange::single(line_number));

        if is_block_title(&line) {
            return Some(line_number);
        }

        // Anything that is not an attached line is the block's content, and
        // the title cannot be below that.
        let attached = line.starts_with('[') && line.trim_end().ends_with(']');
        if !attached {
            return None;
        }
    }

    None
}

/// The text of a block title line, without its leading dot.
pub fn block_title_text(line: &str) -> &str {
    line.strip_prefix('.').unwrap_or(line)
}

/// Writes `text` back as a block title. Titles occupy a single line.
pub fn as_title(text: &str) -> String {
    format!(".{}", text.replace('\n', " ").trim())
}

/// The lines of the list rendered from the block starting at `start`.
///
/// The block's recorded line may point at a title or attribute line attached
/// to the list, neither of which is part of the list itself, so the range
/// starts at the first actual item.
pub fn list_range(src: &str, start: usize) -> Option<LineRange> {
    let block = paragraph_range(src, start);

    let first = (block.start..=block.end)
        .find(|line| list_marker(&text_of(src, LineRange::single(*line))).is_some())?;

    Some(LineRange {
        start: first,
        end: block.end,
    })
}

/// What a table's `cols` attribute says about its columns.
#[derive(Clone, Debug, PartialEq)]
pub enum Columns {
    /// No `cols` attribute: the columns follow the rows and need no upkeep.
    Implicit,
    /// A plain list of widths, which can be kept in step with the rows.
    Widths { line: usize, values: Vec<String> },
    /// A `cols` this module will not rewrite — a repeat (`3*`), an alignment
    /// or a per-column style. Changing the columns under it would describe a
    /// table that no longer exists.
    Opaque,
}

/// Reads the `cols` attribute attached to the table starting at `start`.
pub fn columns_attribute(src: &str, start: usize) -> Columns {
    let mut line = start;

    loop {
        let text = text_of(src, LineRange::single(line));
        if !is_attached(&text) {
            return Columns::Implicit;
        }

        if let Some(values) = column_widths(&text) {
            return Columns::Widths { line, values };
        }

        if text.contains("cols=") {
            return Columns::Opaque;
        }

        line += 1;
    }
}

/// The plain numeric widths of a `cols="1,2"` attribute.
fn column_widths(line: &str) -> Option<Vec<String>> {
    let (_, rest) = line.split_once("cols=\"")?;
    let (values, _) = rest.split_once('"')?;

    let values: Vec<String> = values.split(',').map(str::trim).map(String::from).collect();
    values
        .iter()
        .all(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .then_some(values)
}

/// Rewrites an attribute line's `cols` to the given widths.
pub fn with_columns(line: &str, values: &[String]) -> String {
    let Some((before, rest)) = line.split_once("cols=\"") else {
        return line.to_string();
    };
    let Some((_, after)) = rest.split_once('"') else {
        return line.to_string();
    };

    format!("{before}cols=\"{}\"{after}", values.join(","))
}

/// Every line of the table starting at `start`, delimiters and attachments
/// included.
pub fn table_block_range(src: &str, start: usize) -> Option<LineRange> {
    let (_, closing) = table_delimiters(src, start)?;

    Some(LineRange {
        start,
        end: closing,
    })
}

/// Removes `range` outright, rather than leaving a blank line behind.
pub fn remove_lines(src: &str, range: LineRange) -> String {
    let kept: Vec<&str> = src
        .lines()
        .enumerate()
        .filter(|(index, _)| !(range.start..=range.end).contains(&(index + 1)))
        .map(|(_, line)| line)
        .collect();

    restore_trailing_newline(kept.join("\n"), src)
}

/// The rows between the delimiters of the table starting at `start`.
///
/// The delimiters and anything attached above them stay where they are; only
/// the rows between are ever rewritten.
pub fn table_range(src: &str, start: usize) -> Option<LineRange> {
    let (opening, closing) = table_delimiters(src, start)?;

    // A table with no rows has nothing to address.
    (closing > opening + 1).then_some(LineRange {
        start: opening + 1,
        end: closing - 1,
    })
}

fn table_delimiters(src: &str, start: usize) -> Option<(usize, usize)> {
    let mut opening = start;
    while is_attached(&text_of(src, LineRange::single(opening))) {
        opening += 1;
    }

    if text_of(src, LineRange::single(opening)).trim() != "|===" {
        return None;
    }

    let total = src.lines().count();
    let closing = (opening + 1..=total)
        .find(|line| text_of(src, LineRange::single(*line)).trim() == "|===")?;

    Some((opening, closing))
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

/// Re-casts a block as a list of `marker`, or back into body text.
///
/// The text may already be a list, in which case its markers are swapped while
/// nesting depth is kept; otherwise the block becomes a single item. Turning a
/// list back into body text gives each item its own paragraph.
pub fn as_list(text: &str, marker: Option<char>) -> String {
    let was_list = text.lines().any(|line| list_marker(line).is_some());

    let Some(marker) = marker else {
        if !was_list {
            return text.to_string();
        }

        return text
            .lines()
            .map(|line| match list_marker(line) {
                Some(found) => line.trim_start()[found.len()..].trim(),
                None => line.trim(),
            })
            .collect::<Vec<_>>()
            .join("\n\n");
    };

    if !was_list {
        // A paragraph is one thought, so it becomes one item; its line breaks
        // are only wrapping and would otherwise split it.
        return format!("{} {}", marker, text.replace('\n', " ").trim());
    }

    text.lines()
        .map(|line| match list_marker(line) {
            Some(found) => format!(
                "{} {}",
                marker.to_string().repeat(found.len()),
                line.trim_start()[found.len()..].trim()
            ),
            None => line.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    fn paragraph_range_skips_lines_attached_above_the_block() {
        let titled = "= T\n\n.A title\n[.lead]\nThe paragraph\nwraps here\n\nAfter\n";

        assert_eq!(paragraph_range(titled, 3), LineRange { start: 5, end: 6 });
    }

    #[test]
    fn inserts_and_removes_single_lines() {
        let doc = "= T\n\nBody\n";

        assert_eq!(insert_line(doc, 3, ".A title"), "= T\n\n.A title\nBody\n");
        assert_eq!(remove_line("= T\n\n.A title\nBody\n", 3), "= T\n\nBody\n");
        assert_eq!(insert_line(doc, 99, "Tail"), "= T\n\nBody\nTail\n");
    }

    #[test]
    fn reads_the_text_of_a_range() {
        assert_eq!(
            text_of(DOC, LineRange { start: 3, end: 4 }),
            "First para\nsecond line"
        );
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

        assert_eq!(
            edited,
            "= Title\n\nFirst para\nsecond line\n\n== Section\n\na\n\nb\n"
        );
    }

    #[test]
    fn preserves_whether_the_document_ends_in_a_newline() {
        assert!(replace("a\n", LineRange::single(1), "b").ends_with('\n'));
        assert!(!replace("a", LineRange::single(1), "b").ends_with('\n'));
    }

    #[test]
    fn inserts_a_block_before_a_line() {
        let edited = insert_block(DOC, 6, "New para");

        assert_eq!(
            edited,
            "= Title\n\nFirst para\nsecond line\n\nNew para\n\n== Section\n\nTail\n"
        );
    }

    #[test]
    fn inserts_a_block_past_the_end() {
        assert_eq!(insert_block("a\n", 99, "b"), "a\n\nb\n");
    }

    #[test]
    fn finds_the_rows_between_table_delimiters() {
        let doc = "= T\n\n.A table\n[cols=\"1,2\"]\n|===\n| a | b\n| c | d\n|===\n\nAfter\n";

        assert_eq!(table_range(doc, 3), Some(LineRange { start: 6, end: 7 }));
    }

    #[test]
    fn reads_and_rewrites_plain_column_widths() {
        let doc = "= T\n\n.A table\n[cols=\"1,2\"]\n|===\n| a | b\n|===\n";

        assert_eq!(
            columns_attribute(doc, 3),
            Columns::Widths {
                line: 4,
                values: vec!["1".to_string(), "2".to_string()]
            }
        );
        assert_eq!(
            with_columns(
                "[cols=\"1,2\",options=\"header\"]",
                &["1".to_string(), "1".to_string()]
            ),
            "[cols=\"1,1\",options=\"header\"]"
        );
    }

    #[test]
    fn leaves_alone_the_column_specs_it_cannot_keep_in_step() {
        let repeated = "= T\n\n[cols=\"3*\"]\n|===\n| a\n|===\n";
        let aligned = "= T\n\n[cols=\"^1,>2\"]\n|===\n| a\n|===\n";
        let none = "= T\n\n|===\n| a\n|===\n";

        assert_eq!(columns_attribute(repeated, 3), Columns::Opaque);
        assert_eq!(columns_attribute(aligned, 3), Columns::Opaque);
        assert_eq!(columns_attribute(none, 3), Columns::Implicit);
    }

    #[test]
    fn removes_a_whole_table() {
        let doc = "= T\n\nBefore\n\n.A table\n|===\n| a\n|===\n\nAfter\n";
        let range = table_block_range(doc, 5).expect("a table");

        assert_eq!(range, LineRange { start: 5, end: 8 });
        assert_eq!(remove_lines(doc, range), "= T\n\nBefore\n\n\nAfter\n");
    }

    #[test]
    fn refuses_tables_it_cannot_address() {
        assert_eq!(table_range("= T\n\n|===\n|===\n", 3), None, "no rows");
        assert_eq!(table_range("= T\n\n|===\n| a\n", 3), None, "never closed");
        assert_eq!(table_range("= T\n\nA paragraph\n", 3), None);
    }

    #[test]
    fn recognises_admonition_labels() {
        assert_eq!(admonition_lead("NOTE: something"), Some("NOTE: "));
        assert_eq!(admonition_lead("WARNING: careful"), Some("WARNING: "));
        assert_eq!(admonition_lead("NOTE:no space"), None);
        assert_eq!(admonition_lead("Note: lowercase"), None);
        assert_eq!(admonition_lead("NOTE"), None);
        assert_eq!(admonition_lead("Body text"), None);
    }

    #[test]
    fn matches_labels_to_their_canonical_form() {
        assert_eq!(admonition_label("NOTE: text"), Some("NOTE"));
        assert_eq!(admonition_label("WARNING: "), Some("WARNING"));
        assert_eq!(admonition_label("TIP"), Some("TIP"));
        assert_eq!(admonition_label("Nope"), None);
    }

    #[test]
    fn recognises_block_titles() {
        assert!(is_block_title(".Things that work"));
        assert!(!is_block_title(". a list item"));
        assert!(!is_block_title("...."), "a delimiter");
        assert!(!is_block_title("..nested title"));
        assert!(!is_block_title("."));
        assert!(!is_block_title("Body text"));
    }

    #[test]
    fn finds_a_title_above_its_block() {
        let doc = "= T\n\n.A table\n[cols=\"1,2\"]\n|===\n| a\n|===\n";

        // Recorded line is the title itself.
        assert_eq!(block_title_line(doc, 3), Some(3));
        // Or an attribute line above it.
        let attrs_first = "= T\n\n[cols=\"1,2\"]\n.A table\n|===\n";
        assert_eq!(block_title_line(attrs_first, 3), Some(4));
    }

    #[test]
    fn finds_no_title_when_there_is_none() {
        assert_eq!(block_title_line("= T\n\nJust a paragraph\n", 3), None);
        assert_eq!(block_title_line("= T\n\nNOTE: an admonition\n", 3), None);
        assert_eq!(block_title_line("= T\n\n", 3), None);
    }

    #[test]
    fn writes_titles_back() {
        assert_eq!(block_title_text(".A table"), "A table");
        assert_eq!(as_title("A table"), ".A table");
        assert_eq!(as_title(" folded\nover lines "), ".folded over lines");
    }

    #[test]
    fn recognises_list_markers() {
        assert_eq!(list_marker("* item"), Some("*"));
        assert_eq!(list_marker("- item"), Some("-"));
        assert_eq!(list_marker("** nested"), Some("**"));
        assert_eq!(list_marker(". step"), Some("."));
        assert_eq!(list_marker("  * indented"), Some("*"));
        assert_eq!(
            list_marker(".Block title"),
            None,
            "no space after the marker"
        );
        assert_eq!(list_marker("----"), None, "a delimiter, not a marker");
        assert_eq!(list_marker("*-* mixed"), None);
        assert_eq!(list_marker("1. numbered"), None, "cannot be written back");
        assert_eq!(list_marker("text"), None);
        assert_eq!(list_marker("*  "), None, "a marker with no item text");
    }

    #[test]
    fn list_range_skips_an_attached_title() {
        let doc = "= T\n\n.A title\n* one\n* two\n\nAfter\n";

        assert_eq!(list_range(doc, 3), Some(LineRange { start: 4, end: 5 }));
    }

    #[test]
    fn list_range_needs_at_least_one_item() {
        assert_eq!(list_range("= T\n\nJust a paragraph\n", 3), None);
    }

    #[test]
    fn turns_a_paragraph_into_a_single_item() {
        assert_eq!(as_list("Some text", Some('*')), "* Some text");
        assert_eq!(
            as_list("wrapped\nover lines", Some('.')),
            ". wrapped over lines"
        );
    }

    #[test]
    fn swaps_markers_while_keeping_depth() {
        let list = "* one\n** nested\n* two";

        assert_eq!(as_list(list, Some('.')), ". one\n.. nested\n. two");
        assert_eq!(as_list(list, Some('-')), "- one\n-- nested\n- two");
    }

    #[test]
    fn turns_a_list_back_into_paragraphs() {
        assert_eq!(as_list("* one\n* two", None), "one\n\ntwo");
    }

    #[test]
    fn leaves_body_text_alone() {
        assert_eq!(as_list("Some text", None), "Some text");
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
