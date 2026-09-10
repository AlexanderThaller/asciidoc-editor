//! Editing the rendered document in place.
//!
//! # How this works
//!
//! The AsciiDoc source stays the single source of truth. A rendered block is
//! made `contenteditable`, the browser handles typing, selection and IME
//! natively, and on every input the block's DOM is written back to AsciiDoc
//! and spliced into the source over the lines it came from.
//!
//! While a block has focus it is *not* re-rendered — that is what keeps the
//! caret alive without any position-restoration machinery. Instead, when a
//! block's line count changes, the recorded line numbers of the blocks below
//! it are shifted to match. A full re-render happens once focus leaves the
//! document, which is also what normalises newly typed markup.
//!
//! # Why only some blocks are editable
//!
//! A block is editable only if writing its rendered DOM back out reproduces
//! its source *exactly*. That check is the whole safety story: a table, an
//! attribute reference (`{version}` renders as its value), or a character
//! substitution (`--` renders as an em dash) cannot survive the round trip, so
//! those blocks are left alone rather than silently rewritten. Editing them
//! stays a job for source mode.

use leptos::prelude::*;
use wasm_bindgen::{JsCast, convert::FromWasmAbi, prelude::Closure};
use web_sys::{Document, Element, Event, HtmlDocument, KeyboardEvent, Node};

use crate::{
    inline,
    source::{self, LineRange},
};

/// The block the caret is currently in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub line: usize,
    /// Heading level, or `0` for body text.
    pub level: usize,
}

/// Start line of the source the block was rendered from.
const LINE: &str = "data-edit-line";
/// Last line of that source; absent for a block with no source yet.
const END: &str = "data-edit-end";
/// Heading level, or `0` for body text.
const LEVEL: &str = "data-edit-level";

/// Explains why a block that looks editable is not.
const REFUSED: &str = "This block can only be edited in source mode";

/// Makes every safely editable block in the rendered document editable.
pub fn mark_editable(content: &Element, src: &str) {
    for block in select(content, "div.paragraph[data-source-line]") {
        let (Some(line), Ok(Some(paragraph))) = (line_of(&block), block.query_selector("p")) else {
            continue;
        };

        let range = source::paragraph_range(src, line);
        offer(&paragraph, range, 0, &source::text_of(src, range));
    }

    for level in 1..=6 {
        for heading in select(content, &format!("[data-source-line] > h{level}")) {
            let Some(line) = heading.parent_element().and_then(|parent| line_of(&parent)) else {
                continue;
            };

            let title = source::text_of(src, LineRange::single(line));
            if source::heading_level(&title) == Some(level) {
                offer(
                    &heading,
                    LineRange::single(line),
                    level,
                    source::heading_text(&title),
                );
            }
        }
    }

    // The document title is rendered from the header rather than from a block,
    // so it carries no source line of its own — but it is always the first
    // level-1 heading in the source.
    if let (Ok(Some(title)), Some(line)) = (
        content.query_selector("h1:not([data-source-line])"),
        title_line(src),
    ) {
        let range = LineRange::single(line);
        offer(&title, range, 1, source::heading_text(&source::text_of(src, range)));
    }
}

/// Makes `element` editable if it survives the round trip, and says why not
/// when it does not.
fn offer(element: &Element, range: LineRange, level: usize, expected: &str) {
    if round_trips(element, expected) {
        make_editable(element, range, level);
    } else {
        let _ = element.set_attribute("title", REFUSED);
    }
}

/// The line holding the document title.
fn title_line(src: &str) -> Option<usize> {
    src.lines()
        .position(|line| source::heading_level(line) == Some(1))
        .map(|index| index + 1)
}

/// Whether writing this element back out reproduces its source exactly.
fn round_trips(element: &Element, expected: &str) -> bool {
    serialize(element) == expected.trim_end()
}

fn make_editable(element: &Element, range: LineRange, level: usize) {
    let _ = element.set_attribute("contenteditable", "true");
    let _ = element.set_attribute(LINE, &range.start.to_string());
    let _ = element.set_attribute(END, &range.end.to_string());
    let _ = element.set_attribute(LEVEL, &level.to_string());
}

/// The block's content as AsciiDoc.
fn serialize(element: &Element) -> String {
    inline::to_asciidoc(&inline::from_node(element)).trim().to_string()
}

/// Writes an edited block back into the source.
///
/// Blocks below it move when the edit changes how many lines the block
/// occupies, so their recorded numbers are shifted to keep them addressable
/// without a re-render.
pub fn sync_block(block: &Element, content: &Element, source: RwSignal<String>) {
    let Some(start) = attr(block, LINE) else { return };
    let level = attr(block, LEVEL).unwrap_or(0);
    let text = source::as_block(&serialize(block), (level > 0).then_some(level));

    let added = text.split('\n').count();
    let (edited, removed) = match attr(block, END) {
        Some(end) => {
            let range = LineRange { start, end };
            (
                source::replace(&source.get_untracked(), range, &text),
                range.line_count(),
            )
        }
        // A block created by pressing Enter has no source of its own until
        // now; it takes one, plus the blank line that separates it.
        None => (source::insert_block(&source.get_untracked(), start, &text), 0),
    };

    let separator = usize::from(attr(block, END).is_none());
    let _ = block.set_attribute(END, &(start + added - 1).to_string());
    shift_lines_below(content, start, added + separator, removed);

    source.set(edited);
}

/// Adjusts the recorded line numbers of the blocks after `start`.
fn shift_lines_below(content: &Element, start: usize, added: usize, removed: usize) {
    if added == removed {
        return;
    }

    for block in select(content, &format!("[{LINE}]")) {
        let Some(line) = attr(&block, LINE) else { continue };
        if line <= start {
            continue;
        }

        let moved = line + added - removed;
        let _ = block.set_attribute(LINE, &moved.to_string());
        if let Some(end) = attr(&block, END) {
            let _ = block.set_attribute(END, &(end + added - removed).to_string());
        }
    }
}

/// Wires up editing on the preview document.
///
/// `rerender` re-renders the document and, given a line, puts the caret at the
/// start of the block that came from it.
pub fn attach<R>(
    document: &Document,
    content: Element,
    source: RwSignal<String>,
    editing: RwSignal<Option<Block>>,
    rerender: R,
) where
    R: Fn(Option<usize>) + Clone + 'static,
{
    let on_input = {
        let content = content.clone();
        move |ev: Event| {
            if let Some(block) = editable_target(&ev) {
                sync_block(&block, &content, source);
            }
        }
    };

    let on_focus_in = move |ev: Event| {
        editing.set(editable_target(&ev).and_then(|block| {
            Some(Block {
                line: attr(&block, LINE)?,
                level: attr(&block, LEVEL).unwrap_or(0),
            })
        }));
    };

    let on_focus_out = {
        let document = document.clone();
        let rerender = rerender.clone();
        move |_: Event| {
            // Read the new focus *after* the browser has moved it. Moving
            // between two blocks must not re-render, or the click that caused
            // the move would land on a replaced DOM.
            let document = document.clone();
            let rerender = rerender.clone();
            set_timeout(
                move || {
                    let still_editing = document
                        .active_element()
                        .is_some_and(|active| active.has_attribute(LINE));

                    if !still_editing {
                        editing.set(None);
                        rerender(None);
                    }
                },
                std::time::Duration::ZERO,
            );
        }
    };

    let on_key_down = {
        let document = document.clone();
        let content = content.clone();
        move |ev: KeyboardEvent| {
            let Some(block) = editable_target(ev.as_ref()) else {
                return;
            };

            if ev.key() == "Enter" && !ev.shift_key() {
                ev.prevent_default();
                split_block(&document, &content, &block, source, &rerender);
                return;
            }

            if ev.ctrl_key() || ev.meta_key() {
                let command = match ev.key().to_ascii_lowercase().as_str() {
                    "b" => "bold",
                    "i" => "italic",
                    "e" => "code",
                    _ => return,
                };
                ev.prevent_default();
                format(&document, command);
            }
        }
    };

    listen(document, "input", on_input);
    listen(document, "focusin", on_focus_in);
    listen(document, "focusout", on_focus_out);
    listen(document, "keydown", on_key_down);
}

/// Applies inline formatting to the selection in the preview.
///
/// `execCommand` is deprecated but remains the only way to ask the browser to
/// re-shape a selection while keeping its own undo stack intact, which is what
/// makes it worth keeping here. `styleWithCSS` is turned off so it emits tags
/// rather than styled spans — tags are what can be written back as AsciiDoc.
pub fn format(document: &Document, command: &str) {
    // `execCommand` hangs off `HTMLDocument` rather than `Document`.
    let commands: &HtmlDocument = document.unchecked_ref();
    let _ = commands.exec_command_with_show_ui_and_value("styleWithCSS", false, "false");

    if command == "code" {
        let Some(selected) = document.get_selection().ok().flatten() else {
            return;
        };
        let text = selected.to_string().as_string().unwrap_or_default();
        if text.is_empty() {
            return;
        }

        let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let _ = commands.exec_command_with_show_ui_and_value(
            "insertHTML",
            false,
            &format!("<code>{escaped}</code>"),
        );
        return;
    }

    let _ = commands.exec_command(command);
}

/// Re-casts the focused block as a heading of `level`, or as body text.
///
/// A paragraph folded into a heading loses its line breaks — a heading is a
/// single line — so this always re-renders rather than trying to patch the
/// existing DOM.
pub fn set_level<R>(document: &Document, source: RwSignal<String>, level: Option<usize>, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(block) = document.active_element().filter(|block| block.has_attribute(LINE)) else {
        return;
    };
    let Some(start) = attr(&block, LINE) else { return };
    let end = attr(&block, END).unwrap_or(start);

    let text = source::as_block(&serialize(&block), level);
    source.set(source::replace(
        &source.get_untracked(),
        LineRange { start, end },
        &text,
    ));

    rerender(Some(start));
}

/// Splits a paragraph at the caret, or starts a new paragraph after a heading.
fn split_block<R>(
    document: &Document,
    content: &Element,
    block: &Element,
    source: RwSignal<String>,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    let Some(start) = attr(block, LINE) else { return };
    let end = attr(block, END).unwrap_or(start);
    let text = serialize(block);

    // Splitting a heading would produce a second heading; a new paragraph
    // under it is what pressing Enter there is actually asking for.
    let offset = if attr(block, LEVEL).unwrap_or(0) > 0 {
        text.len()
    } else {
        caret_offset(document, block).unwrap_or(text.len()).min(text.len())
    };

    if !text.is_char_boundary(offset) {
        return;
    }
    let (before, after) = text.split_at(offset);

    if after.trim().is_empty() {
        // Nothing to carry down, so there is no block to render yet. Give the
        // caret somewhere to live and let the first keystroke create the source.
        start_pending_block(document, content, block, end + 2);
        return;
    }

    let replacement = format!("{}\n\n{}", before.trim_end(), after.trim_start());
    source.set(source::replace(
        &source.get_untracked(),
        LineRange { start, end },
        &replacement,
    ));

    rerender(Some(start + before.trim_end().split('\n').count() + 1));
}

/// Adds an empty paragraph that has no source behind it yet.
fn start_pending_block(document: &Document, content: &Element, after: &Element, line: usize) {
    let Ok(paragraph) = document.create_element("p") else {
        return;
    };

    let _ = paragraph.set_attribute("contenteditable", "true");
    let _ = paragraph.set_attribute(LINE, &line.to_string());
    let _ = paragraph.set_attribute(LEVEL, "0");
    // An empty block has no height to click on or place a caret in.
    let _ = paragraph.append_child(&document.create_element("br").unwrap().into());

    let wrapper = after.parent_element().unwrap_or_else(|| after.clone());
    let container = wrapper.parent_element().unwrap_or_else(|| content.clone());
    let _ = container.insert_before(&paragraph, wrapper.next_sibling().as_ref());

    focus(document, &paragraph);
}

/// How far into the block's AsciiDoc text the caret sits.
///
/// Serialising the content *before* the caret gives the answer directly: the
/// same function that writes the block back out defines the mapping, so the
/// two can never disagree.
fn caret_offset(document: &Document, block: &Element) -> Option<usize> {
    let selection = document.get_selection().ok()??;
    let focus_node = selection.focus_node()?;

    let range = document.create_range().ok()?;
    range.set_start(block, 0).ok()?;
    range.set_end(&focus_node, selection.focus_offset()).ok()?;

    let fragment = range.clone_contents().ok()?;
    Some(inline::to_asciidoc(&inline::from_node(&fragment)).trim_start().len())
}

/// Puts the caret at the start of `element`.
pub fn focus(document: &Document, element: &Element) {
    let html: &web_sys::HtmlElement = element.unchecked_ref();
    let _ = html.focus();

    let (Some(selection), Ok(range)) = (
        document.get_selection().ok().flatten(),
        document.create_range(),
    ) else {
        return;
    };

    let _ = range.set_start(element, 0);
    range.collapse_with_to_start(true);
    let _ = selection.remove_all_ranges();
    let _ = selection.add_range(&range);
}

/// The editable block an event happened inside, if any.
fn editable_target(ev: &Event) -> Option<Element> {
    let target = ev.target()?.unchecked_into::<Node>();
    if target.node_type() != Node::ELEMENT_NODE {
        return target.parent_element()?.closest(&format!("[{LINE}]")).ok()?;
    }

    target.unchecked_into::<Element>().closest(&format!("[{LINE}]")).ok()?
}

fn listen<E, F>(document: &Document, event: &str, handler: F)
where
    E: FromWasmAbi + 'static,
    F: FnMut(E) + 'static,
{
    let closure = Closure::<dyn FnMut(E)>::new(handler);
    let _ = document.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
    // Lives as long as the preview document, which lives as long as the app.
    closure.forget();
}

fn select(content: &Element, selector: &str) -> Vec<Element> {
    let Ok(nodes) = content.query_selector_all(selector) else {
        return Vec::new();
    };

    (0..nodes.length())
        .filter_map(|index| nodes.item(index).map(JsCast::unchecked_into))
        .collect()
}

fn line_of(element: &Element) -> Option<usize> {
    element.get_attribute("data-source-line")?.parse().ok()
}

fn attr(element: &Element, name: &str) -> Option<usize> {
    element.get_attribute(name)?.parse().ok()
}
