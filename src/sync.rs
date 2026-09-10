//! Two-way position mapping between the source textarea and the preview.
//!
//! The preview side is keyed on the `data-source-line` attributes that
//! `Options::source_locations(true)` puts on every rendered block.
//!
//! Textarea offsets are UTF-16 code units (what the DOM reports), not bytes,
//! so both conversions walk the source rather than indexing into it.

use wasm_bindgen::JsCast;
use web_sys::{Document, Element, ScrollIntoViewOptions, ScrollLogicalPosition};

/// 1-based line containing the given UTF-16 offset.
pub fn line_of_offset(src: &str, offset: usize) -> usize {
    let mut line = 1;
    let mut seen = 0;

    for c in src.chars() {
        if seen >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
        }
        seen += c.len_utf16();
    }

    line
}

/// UTF-16 offset of the start of a 1-based line.
pub fn offset_of_line(src: &str, line: usize) -> usize {
    let mut current = 1;
    let mut offset = 0;

    for c in src.chars() {
        if current >= line {
            break;
        }
        if c == '\n' {
            current += 1;
        }
        offset += c.len_utf16();
    }

    offset
}

/// Scrolls the preview to the block covering `line`.
///
/// Blocks are emitted in source order, so the target is the last annotated
/// element that starts at or before the cursor.
pub fn scroll_preview_to_line(preview: &Document, line: usize) {
    let Some(target) = block_at_or_before(preview, line) else {
        return;
    };

    // Deliberately not `ScrollBehavior::Smooth`: smooth scrolling is dropped on
    // the floor inside the preview iframe (a plain `scrollIntoView` from the
    // parent frame moves it, the smooth variant never starts), so an animated
    // scroll here would simply never happen.
    let opts = ScrollIntoViewOptions::new();
    opts.set_block(ScrollLogicalPosition::Nearest);
    target.scroll_into_view_with_scroll_into_view_options(&opts);
}

fn block_at_or_before(preview: &Document, line: usize) -> Option<Element> {
    let blocks = preview.query_selector_all("[data-source-line]").ok()?;
    let mut best = None;

    for i in 0..blocks.length() {
        // Cast unchecked: these nodes live in the iframe's realm, where an
        // `instanceof` check against this realm's `Element` always fails.
        let element: Element = blocks.item(i)?.unchecked_into();
        if source_line(&element).is_some_and(|l| l <= line) {
            best = Some(element);
        } else {
            break;
        }
    }

    best
}

/// The line a preview element came from, walking up to the nearest annotated
/// ancestor — clicks usually land on an inner node.
pub fn source_line_of_click(target: &Element) -> Option<usize> {
    let block = target.closest("[data-source-line]").ok()??;
    source_line(&block)
}

fn source_line(element: &Element) -> Option<usize> {
    element.get_attribute("data-source-line")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_offsets_to_lines() {
        let src = "one\ntwo\nthree";

        assert_eq!(line_of_offset(src, 0), 1);
        assert_eq!(line_of_offset(src, 3), 1);
        assert_eq!(line_of_offset(src, 4), 2);
        assert_eq!(line_of_offset(src, 9), 3);
    }

    #[test]
    fn maps_lines_back_to_offsets() {
        let src = "one\ntwo\nthree";

        assert_eq!(offset_of_line(src, 1), 0);
        assert_eq!(offset_of_line(src, 2), 4);
        assert_eq!(offset_of_line(src, 3), 8);
        // Past the end clamps to the end of the source.
        assert_eq!(offset_of_line(src, 99), 13);
    }

    /// The DOM counts UTF-16 units, so astral characters advance by two.
    #[test]
    fn offsets_are_utf16_not_bytes() {
        let src = "🦀\nnext";

        assert_eq!(line_of_offset(src, 2), 1);
        assert_eq!(line_of_offset(src, 3), 2);
        assert_eq!(offset_of_line(src, 2), 3);
    }

    #[test]
    fn round_trips() {
        let src = "a\nbb\nccc\n";
        for line in 1..=4 {
            assert_eq!(line_of_offset(src, offset_of_line(src, line)), line);
        }
    }
}
