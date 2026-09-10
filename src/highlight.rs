//! Line-oriented AsciiDoc tokenizer feeding the editor's highlight overlay.
//!
//! This produces HTML that is laid *behind* a transparent `<textarea>`, so the
//! only hard requirement is that the rendered text is character-for-character
//! identical to the source; the markup is purely decorative.

use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Delimiter lines that open a block whose contents are not marked up further.
const VERBATIM_FENCES: [&str; 3] = ["----", "....", "++++"];

/// Delimiter lines that open a block whose contents keep normal inline markup.
const PLAIN_FENCES: [&str; 5] = ["====", "****", "____", "--", "|==="];

static ADMONITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(NOTE|TIP|IMPORTANT|WARNING|CAUTION): ").unwrap());

static LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([*\-]+|\.+|\d+\.|[a-zA-Z]\.)(\s+)").unwrap());

static ATTR_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:!?[a-zA-Z0-9_][a-zA-Z0-9_-]*!?:").unwrap());

static INLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?P<code>`[^`\n]+`)",
        r"|(?P<bold>\*[^*\s][^*\n]*\*)",
        r"|(?P<italic>_[^_\s][^_\n]*_)",
        r"|(?P<mac>\b(?:https?|link|xref|image|kbd|btn|footnote|mailto):[^\s\[]*\[[^\]\n]*\])",
        r"|(?P<url>\bhttps?://[^\s\[\]]+)",
        r"|(?P<attr>\{[a-zA-Z0-9_][a-zA-Z0-9_-]*\})",
    ))
    .unwrap()
});

/// Renders `src` as highlighted HTML for the overlay.
pub fn highlight(src: &str) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    // The fence that will close the verbatim block we are currently inside.
    let mut verbatim: Option<&str> = None;

    for line in src.split('\n') {
        let trimmed = line.trim_end();

        if let Some(fence) = verbatim {
            push_span(&mut out, "verbatim", line);
            if trimmed == fence {
                verbatim = None;
            }
        } else if let Some(fence) = opening_fence(trimmed) {
            push_span(&mut out, "delim", line);
            if VERBATIM_FENCES.contains(&fence) {
                verbatim = Some(fence);
            }
        } else {
            highlight_line(&mut out, line);
        }
        // Note this leaves one newline beyond the source: `<pre>` swallows a
        // trailing line break, so the guard line keeps the overlay as tall as
        // the textarea when the document ends in a newline.
        out.push('\n');
    }

    out
}

/// Returns the canonical fence for a delimiter line, if it is one.
///
/// AsciiDoc fences may be longer than their canonical four characters (`-----`
/// closes `-----`, not `----`), so the whole run is returned as the delimiter.
fn opening_fence(line: &str) -> Option<&str> {
    if line.len() >= 2 && PLAIN_FENCES.iter().chain(&VERBATIM_FENCES).any(|f| line == *f) {
        return Some(line);
    }

    // Repeated-character fences of non-canonical length.
    let first = line.chars().next()?;
    if line.len() >= 4
        && matches!(first, '-' | '.' | '+' | '=' | '*' | '_')
        && line.chars().all(|c| c == first)
    {
        return Some(line);
    }

    None
}

fn highlight_line(out: &mut String, line: &str) {
    let trimmed_start = line.trim_start();

    if trimmed_start.starts_with("//") {
        return push_span(out, "comment", line);
    }
    if is_heading(line) {
        return push_span(out, "heading", line);
    }
    if ATTR_ENTRY.is_match(line) {
        return push_span(out, "attr-entry", line);
    }
    if line.starts_with('[') && line.trim_end().ends_with(']') {
        return push_span(out, "attr-list", line);
    }
    // A block title (`.Title`) — but not a `. ` list item or `...` fence.
    if line.starts_with('.') && !line.starts_with(". ") && !line.starts_with("..") {
        return push_span(out, "block-title", line);
    }

    if let Some(m) = ADMONITION.find(line) {
        push_span(out, "admonition", m.as_str());
        out.push_str(&inline(&line[m.end()..]));
        return;
    }

    if let Some(caps) = LIST_MARKER.captures(line) {
        out.push_str(&escape(&caps[1]));
        push_span(out, "marker", &caps[2]);
        out.push_str(&escape(&caps[3]));
        out.push_str(&inline(&line[caps[0].len()..]));
        return;
    }

    out.push_str(&inline(line));
}

fn is_heading(line: &str) -> bool {
    let level = line.chars().take_while(|c| *c == '=').count();
    (1..=6).contains(&level) && line[level..].starts_with(' ')
}

/// Wraps inline constructs. Escaping happens first, so the patterns only ever
/// see `&lt;`/`&amp;` — none of which contain the markers being matched.
fn inline(text: &str) -> String {
    let escaped = escape(text);

    INLINE
        .replace_all(&escaped, |caps: &Captures| {
            let (class, matched) = ["code", "bold", "italic", "mac", "url", "attr"]
                .iter()
                .find_map(|name| caps.name(name).map(|m| (*name, m.as_str())))
                .expect("one alternative always matches");

            format!(r#"<span class="ad-{class}">{matched}</span>"#)
        })
        .into_owned()
}

fn push_span(out: &mut String, class: &str, text: &str) {
    out.push_str(r#"<span class="ad-"#);
    out.push_str(class);
    out.push_str(r#"">"#);
    out.push_str(&escape(text));
    out.push_str("</span>");
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strips tags and unescapes, i.e. recovers what the browser will display.
    fn text_of(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }

    /// The overlay must line up with the textarea character for character.
    #[test]
    fn rendered_text_round_trips_the_source() {
        let src = "= Title\n:attr: v\n\n== Section\n\nA *bold* and _it_ and `x < y`.\n\n\
                   [source,rust]\n----\nfn main() { if a < b {} }\n----\n\n* item\n. step\n\n\
                   NOTE: careful.\n// comment\nhttps://example.com[link]\n";

        assert_eq!(text_of(&highlight(src)), format!("{src}\n"));
    }

    #[test]
    fn classifies_line_kinds() {
        assert!(highlight("== Section\n").contains(r#"class="ad-heading""#));
        assert!(highlight(":toc:\n").contains(r#"class="ad-attr-entry""#));
        assert!(highlight("[source,rust]\n").contains(r#"class="ad-attr-list""#));
        assert!(highlight(".Block title\n").contains(r#"class="ad-block-title""#));
        assert!(highlight("// note\n").contains(r#"class="ad-comment""#));
        assert!(highlight("NOTE: hi\n").contains(r#"class="ad-admonition""#));
        assert!(highlight("* item\n").contains(r#"class="ad-marker""#));
    }

    #[test]
    fn verbatim_blocks_suppress_inline_markup() {
        let html = highlight("----\nlet x = *not bold*;\n----\n");

        assert!(html.contains(r#"class="ad-verbatim""#));
        assert!(!html.contains(r#"class="ad-bold""#));
    }

    #[test]
    fn verbatim_block_ends_at_its_own_fence() {
        let html = highlight("----\ncode\n----\n\n*bold*\n");

        assert!(html.contains(r#"class="ad-bold""#), "markup resumes after the fence");
    }

    #[test]
    fn escapes_html_in_source() {
        assert!(highlight("<script>\n").contains("&lt;script&gt;"));
    }
}
