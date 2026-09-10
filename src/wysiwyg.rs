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
    inline, list,
    source::{self, LineRange},
};

/// The block the caret is currently in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub line: usize,
    pub kind: Kind,
    /// The line of the title attached to this block, when it has one. Also set
    /// on a title itself, which is its own.
    pub title_line: Option<usize>,
}

/// What a block is, as far as the toolbar is concerned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Body,
    Heading(usize),
    List {
        ordered: bool,
    },
    /// The title attached to a block, such as `.Things that work`.
    Title,
    /// An inline admonition, such as `NOTE: mind the gap`, labelled with one
    /// of [`source::ADMONITIONS`].
    Admonition(&'static str),
}

/// Start line of the source the block was rendered from.
const LINE: &str = "data-edit-line";
/// Last line of that source; absent for a block with no source yet.
const END: &str = "data-edit-end";
/// Heading level, or `0` for body text.
const LEVEL: &str = "data-edit-level";
/// The list marker the source uses, so that editing preserves its style.
const MARKER: &str = "data-edit-marker";
/// Marks a block title, whose source line carries a leading dot.
const TITLE: &str = "data-edit-title";
/// Text the renderer generates in front of a title, such as a table's number.
const PREFIX: &str = "data-edit-prefix";
/// Text the source carries in front of the content but the rendering drops,
/// such as an admonition's `NOTE: ` label.
const LEAD: &str = "data-edit-lead";

/// Explains why a block that looks editable is not.
const REFUSED: &str = "This block can only be edited in source mode";

/// Makes every safely editable block in the rendered document editable.
pub fn mark_editable(content: &Element, src: &str) {
    for block in select(content, "div.paragraph[data-source-line]") {
        let (Some(line), Ok(Some(paragraph))) = (line_of(&block), block.query_selector("p")) else {
            continue;
        };

        let range = source::paragraph_range(src, line);
        offer(&paragraph, range, Kind::Body, &source::text_of(src, range));
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
                    Kind::Heading(level),
                    source::heading_text(&title),
                );
            }
        }
    }

    for block in select(
        content,
        "div.ulist[data-source-line], div.olist[data-source-line]",
    ) {
        let (Some(line), Ok(Some(items))) = (line_of(&block), block.query_selector("ul, ol"))
        else {
            continue;
        };

        // A nested list is edited through the list that contains it: on its
        // own it has no idea how deep it sits, so it could not write its
        // items back with the right number of markers.
        if items
            .parent_element()
            .and_then(|parent| parent.closest("ul, ol").ok().flatten())
            .is_some()
        {
            continue;
        }

        let Some(range) = source::list_range(src, line) else {
            continue;
        };

        // Recorded before the round trip is checked, because writing the list
        // back out reads the marker from here.
        if let Some(marker) =
            source::list_marker(&source::text_of(src, LineRange::single(range.start)))
        {
            let _ = items.set_attribute(MARKER, marker);
        }

        offer(&items, range, Kind::Body, &source::text_of(src, range));
    }

    for block in select(content, "div.admonitionblock[data-source-line]") {
        let (Some(line), Ok(Some(body))) = (line_of(&block), block.query_selector("td.content"))
        else {
            continue;
        };

        let range = source::paragraph_range(src, line);
        let text = source::text_of(src, range);

        // Only the inline form carries its label on the same line; the
        // delimited form is a block of its own and is left to source mode.
        let Some(lead) = source::admonition_lead(&text) else {
            continue;
        };

        let Some(label) = source::admonition_label(lead) else {
            continue;
        };

        let _ = body.set_attribute(LEAD, lead);
        offer(&body, range, Kind::Admonition(label), &text[lead.len()..]);
    }

    for block in select(content, "[data-source-line]") {
        let (Some(line), Ok(Some(title))) =
            (line_of(&block), block.query_selector(":scope > .title"))
        else {
            continue;
        };

        // Only a title the source actually writes can be edited: a `Note`
        // label or a figure caption the renderer invents has no line to
        // write back to.
        let Some(title_line) = source::block_title_line(src, line) else {
            continue;
        };

        let range = LineRange::single(title_line);
        let written = source::block_title_text(&source::text_of(src, range)).to_string();

        // A table numbers its caption, so what is rendered may carry a prefix
        // that is not in the source and must not be written back into it.
        let prefix = serialize(&title)
            .strip_suffix(written.trim_end())
            .unwrap_or_default()
            .to_string();
        if !prefix.is_empty() {
            let _ = title.set_attribute(PREFIX, &prefix);
        }

        offer(&title, range, Kind::Title, &format!("{prefix}{written}"));
    }

    // The document title is rendered from the header rather than from a block,
    // so it carries no source line of its own — but it is always the first
    // level-1 heading in the source.
    if let (Ok(Some(title)), Some(line)) = (
        content.query_selector("h1:not([data-source-line])"),
        title_line(src),
    ) {
        let range = LineRange::single(line);
        offer(
            &title,
            range,
            Kind::Heading(1),
            source::heading_text(&source::text_of(src, range)),
        );
    }
}

/// Makes `element` editable if it survives the round trip, and says why not
/// when it does not.
fn offer(element: &Element, range: LineRange, kind: Kind, expected: &str) {
    if round_trips(element, expected) {
        make_editable(element, range, kind);
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

fn make_editable(element: &Element, range: LineRange, kind: Kind) {
    let _ = element.set_attribute("contenteditable", "true");
    let _ = element.set_attribute(LINE, &range.start.to_string());
    let _ = element.set_attribute(END, &range.end.to_string());

    match kind {
        Kind::Heading(level) => {
            let _ = element.set_attribute(LEVEL, &level.to_string());
        }
        Kind::Title => {
            let _ = element.set_attribute(TITLE, "true");
        }
        _ => {}
    }
}

/// The block's content as AsciiDoc.
fn serialize(element: &Element) -> String {
    if let Some(items) = list::from_element(element) {
        let marker = element
            .get_attribute(MARKER)
            .and_then(|marker| marker.chars().next())
            .unwrap_or('*');

        return list::to_asciidoc(&items, marker);
    }

    inline::to_asciidoc(&inline::from_node(element))
        .trim()
        .to_string()
}

/// The line of the block's title, whether the block *is* the title or merely
/// carries one.
fn title_line_of(block: &Element) -> Option<usize> {
    if block.has_attribute(TITLE) {
        return attr(block, LINE);
    }

    let title = block
        .parent_element()?
        .query_selector(&format!(":scope > [{TITLE}]"))
        .ok()??;

    attr(&title, LINE)
}

fn kind_of(block: &Element) -> Kind {
    if block.has_attribute(TITLE) {
        return Kind::Title;
    }

    if let Some(label) = block
        .get_attribute(LEAD)
        .and_then(|lead| source::admonition_label(&lead))
    {
        return Kind::Admonition(label);
    }

    if is_list(block) {
        return Kind::List {
            ordered: block.tag_name().eq_ignore_ascii_case("ol"),
        };
    }

    match attr(block, LEVEL).unwrap_or(0) {
        0 => Kind::Body,
        level => Kind::Heading(level),
    }
}

/// Whether the block is a list, whose items the browser manages itself.
fn is_list(element: &Element) -> bool {
    matches!(
        element.tag_name().to_ascii_uppercase().as_str(),
        "UL" | "OL"
    )
}

/// Writes an edited block back into the source.
///
/// Blocks below it move when the edit changes how many lines the block
/// occupies, so their recorded numbers are shifted to keep them addressable
/// without a re-render.
pub fn sync_block(block: &Element, content: &Element, source: RwSignal<String>) {
    let Some(start) = attr(block, LINE) else {
        return;
    };
    let text = match kind_of(block) {
        Kind::Title => {
            let written = serialize(block);
            // Drop the generated prefix again. If the caret wandered into it
            // there is nothing to strip, and what the user typed is used whole.
            let prefix = block.get_attribute(PREFIX).unwrap_or_default();
            source::as_title(written.strip_prefix(&prefix).unwrap_or(&written))
        }
        Kind::Heading(level) => source::as_block(&serialize(block), Some(level)),
        // The label is part of the source line but not of what is rendered.
        Kind::Admonition(_) => format!(
            "{}{}",
            block.get_attribute(LEAD).unwrap_or_default(),
            serialize(block)
        ),
        _ => serialize(block),
    };

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
        None => (
            source::insert_block(&source.get_untracked(), start, &text),
            0,
        ),
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
        let Some(line) = attr(&block, LINE) else {
            continue;
        };
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
                kind: kind_of(&block),
                title_line: title_line_of(&block),
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
                // A list handles Enter far better than we could: the browser
                // starts a new item, and the input that follows writes the
                // whole list back out.
                if is_list(&block) {
                    return;
                }

                ev.prevent_default();
                split_block(&document, &content, &block, source, &rerender);
                return;
            }

            if ev.key() == "Tab" && is_list(&block) {
                // Swallowed either way: tab has no business moving focus out
                // of the document while a list is being edited.
                ev.prevent_default();
                reindent(&document, ev.shift_key());
                // Moving nodes fires no input event, so the write-back that
                // normally follows an edit has to be asked for.
                sync_block(&block, &content, source);
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
    let commands = exec(document, "styleWithCSS", Some("false"));

    if command == "code" {
        let Some(selected) = document.get_selection().ok().flatten() else {
            return;
        };
        let text = selected.to_string().as_string().unwrap_or_default();
        if text.is_empty() {
            return;
        }

        let escaped = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let _ = commands.exec_command_with_show_ui_and_value(
            "insertHTML",
            false,
            &format!("<code>{escaped}</code>"),
        );
        return;
    }

    exec(document, command, None);
}

/// Runs an `execCommand`, returning the document it ran against so that a
/// caller can chain another one.
///
/// `execCommand` hangs off `HTMLDocument` rather than `Document`.
fn exec<'a>(document: &'a Document, command: &str, value: Option<&str>) -> &'a HtmlDocument {
    let commands: &HtmlDocument = document.unchecked_ref();
    let _ = commands.exec_command_with_show_ui_and_value(command, false, value.unwrap_or(""));
    commands
}

/// Moves the list item holding the caret one level deeper, or shallower.
///
/// The browser cannot help here: `execCommand("indent")` refuses to restructure
/// the element it was given as the editing root, and that root is the list
/// itself. Moving the `li` by hand carries the caret with it, because the text
/// node the selection points at moves along with the item.
fn reindent(document: &Document, outdent: bool) {
    let Some(item) = caret_item(document) else {
        return;
    };

    // Where the caret sits, so it can be put back afterwards. Moving an item
    // re-parents its text nodes without replacing them, so the same node and
    // offset still describe the same spot once the move is done.
    let caret = document
        .get_selection()
        .ok()
        .flatten()
        .and_then(|selection| Some((selection.focus_node()?, selection.focus_offset())));

    if outdent {
        outdent_item(&item);
    } else {
        indent_item(document, &item);
    }

    if let Some((node, offset)) = caret {
        restore_caret(document, &node, offset);
    }
}

fn restore_caret(document: &Document, node: &Node, offset: u32) -> Option<()> {
    let selection = document.get_selection().ok()??;
    let range = document.create_range().ok()?;

    range.set_start(node, offset).ok()?;
    range.collapse_with_to_start(true);
    selection.remove_all_ranges().ok()?;
    selection.add_range(&range).ok()?;
    Some(())
}

/// Nests the item under the one before it.
fn indent_item(document: &Document, item: &Element) -> Option<()> {
    // An item can only nest under one that precedes it, in AsciiDoc as in
    // HTML, so the first item of a list has nothing to nest under.
    let previous = previous_item(item)?;
    let list = item.parent_element()?;

    let nested = match child_list(&previous) {
        Some(existing) => existing,
        None => {
            let created = document.create_element(&list.tag_name()).ok()?;
            previous.append_child(&created).ok()?;
            created
        }
    };

    nested.append_child(item).ok()?;
    Some(())
}

/// Lifts the item out to its parent's level.
fn outdent_item(item: &Element) -> Option<()> {
    let list = item.parent_element()?;
    let parent_item = list.parent_element()?.closest("li").ok()??;
    let outer = parent_item.parent_element()?;

    // Items below this one were deeper than it and must stay that way, so they
    // follow it down a level.
    let following: Vec<Element> = siblings_after(item);
    if !following.is_empty() {
        let sub = match child_list(item) {
            Some(existing) => existing,
            None => {
                let created = item
                    .owner_document()?
                    .create_element(&list.tag_name())
                    .ok()?;
                item.append_child(&created).ok()?;
                created
            }
        };

        for sibling in following {
            sub.append_child(&sibling).ok()?;
        }
    }

    outer
        .insert_before(item, parent_item.next_sibling().as_ref())
        .ok()?;

    // The list it came from may now be empty.
    if list.query_selector("li").ok().flatten().is_none() {
        match list.parent_element() {
            // The renderer wraps a nested list in a div; drop that too.
            Some(wrapper) if wrapper.tag_name().eq_ignore_ascii_case("div") => wrapper.remove(),
            _ => list.remove(),
        }
    }

    Some(())
}

/// The list nested inside an item, seeing through the renderer's wrapper.
fn child_list(item: &Element) -> Option<Element> {
    item.query_selector("ul, ol").ok()?
}

/// The item's following siblings, in order.
fn siblings_after(item: &Element) -> Vec<Element> {
    let mut out = Vec::new();
    let mut sibling = item.next_element_sibling();

    while let Some(candidate) = sibling {
        sibling = candidate.next_element_sibling();
        if candidate.tag_name().eq_ignore_ascii_case("li") {
            out.push(candidate);
        }
    }

    out
}

/// The list item the caret sits in.
fn caret_item(document: &Document) -> Option<Element> {
    let selection = document.get_selection().ok()??;
    let node = selection.focus_node()?;

    let element = match node.node_type() {
        Node::ELEMENT_NODE => node.unchecked_into::<Element>(),
        _ => node.parent_element()?,
    };

    element.closest("li").ok()?
}

fn previous_item(item: &Element) -> Option<Element> {
    let mut sibling = item.previous_element_sibling();

    while let Some(candidate) = sibling {
        if candidate.tag_name().eq_ignore_ascii_case("li") {
            return Some(candidate);
        }
        sibling = candidate.previous_element_sibling();
    }

    None
}

/// Re-casts the focused block as a heading of `level`, or as body text.
///
/// A paragraph folded into a heading loses its line breaks — a heading is a
/// single line — so this always re-renders rather than trying to patch the
/// existing DOM.
pub fn set_level<R>(
    document: &Document,
    source: RwSignal<String>,
    level: Option<usize>,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    let Some(block) = document
        .active_element()
        .filter(|block| block.has_attribute(LINE))
    else {
        return;
    };
    let Some(start) = attr(&block, LINE) else {
        return;
    };
    let end = attr(&block, END).unwrap_or(start);

    let text = source::as_block(&serialize(&block), level);
    source.set(source::replace(
        &source.get_untracked(),
        LineRange { start, end },
        &text,
    ));

    rerender(Some(start));
}

/// Re-casts the focused block as a list of `marker`, or back into body text.
pub fn set_list<R>(
    document: &Document,
    source: RwSignal<String>,
    marker: Option<char>,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    let Some(block) = document.active_element().filter(|block| {
        block.has_attribute(LINE) && !block.has_attribute(TITLE) && !block.has_attribute(LEAD)
    }) else {
        return;
    };
    let Some(start) = attr(&block, LINE) else {
        return;
    };
    let end = attr(&block, END).unwrap_or(start);

    let text = source::as_list(&serialize(&block), marker);
    source.set(source::replace(
        &source.get_untracked(),
        LineRange { start, end },
        &text,
    ));

    rerender(Some(start));
}

/// Placeholder for a title that has just been added, selected so that the
/// first keystroke replaces it.
const NEW_TITLE: &str = "Title";

/// Labels the focused block as an admonition, or takes the label away.
///
/// What is rendered never contains the label, so the block's text can simply
/// be written back out under a different one — or under none.
pub fn set_admonition<R>(
    document: &Document,
    source: RwSignal<String>,
    label: Option<&str>,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    let Some(block) = focused(document) else {
        return;
    };
    // Only body text can take a label, and only an admonition can lose one.
    if !matches!(kind_of(&block), Kind::Body | Kind::Admonition(_)) {
        return;
    }

    let Some(start) = attr(&block, LINE) else {
        return;
    };
    let end = attr(&block, END).unwrap_or(start);

    let text = serialize(&block);
    let replacement = match label {
        Some(label) => format!("{label}: {text}"),
        None => text,
    };

    source.set(source::replace(
        &source.get_untracked(),
        LineRange { start, end },
        &replacement,
    ));

    rerender(Some(start));
}

/// Gives the focused block a title, and puts the caret in it.
pub fn add_title<R>(document: &Document, source: RwSignal<String>, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(block) = focused(document) else {
        return;
    };
    let Some(line) = attr(&block, LINE) else {
        return;
    };

    source.set(source::insert_line(
        &source.get_untracked(),
        line,
        &source::as_title(NEW_TITLE),
    ));

    rerender(Some(line));

    // Select the placeholder rather than leaving a caret beside it: the point
    // of adding a title is to type one.
    if let Some(title) = block_at(document, line) {
        select_contents(document, &title);
    }
}

/// Takes the title away from the focused block.
pub fn remove_title<R>(document: &Document, source: RwSignal<String>, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(block) = focused(document) else {
        return;
    };
    let Some(line) = title_line_of(&block) else {
        return;
    };

    source.set(source::remove_line(&source.get_untracked(), line));

    // The block itself has moved up into the line the title occupied.
    rerender(Some(line));
}

/// Indents or outdents the focused list item, as tab does.
pub fn reindent_focused(document: &Document, source: RwSignal<String>, outdent: bool) {
    let (Some(block), Some(content)) = (focused(document), content_of(document)) else {
        return;
    };
    if !is_list(&block) {
        return;
    }

    reindent(document, outdent);
    sync_block(&block, &content, source);
}

fn focused(document: &Document) -> Option<Element> {
    document
        .active_element()
        .filter(|block| block.has_attribute(LINE))
}

fn block_at(document: &Document, line: usize) -> Option<Element> {
    content_of(document)?
        .query_selector(&format!("[{LINE}=\"{line}\"]"))
        .ok()?
}

fn content_of(document: &Document) -> Option<Element> {
    document.get_element_by_id("content")
}

/// Focuses `element` with all of its text selected.
fn select_contents(document: &Document, element: &Element) -> Option<()> {
    let html: &web_sys::HtmlElement = element.unchecked_ref();
    let _ = html.focus();

    let selection = document.get_selection().ok()??;
    let range = document.create_range().ok()?;
    range.select_node_contents(element).ok()?;
    selection.remove_all_ranges().ok()?;
    selection.add_range(&range).ok()?;
    Some(())
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
    let Some(start) = attr(block, LINE) else {
        return;
    };
    let end = attr(block, END).unwrap_or(start);
    let text = serialize(block);

    // A title belongs to the block below it; splitting it would put a stray
    // paragraph between the two.
    if kind_of(block) == Kind::Title {
        return;
    }

    // Splitting a heading would produce a second heading; a new paragraph
    // under it is what pressing Enter there is actually asking for.
    let offset = if attr(block, LEVEL).unwrap_or(0) > 0 {
        text.len()
    } else {
        caret_offset(document, block)
            .unwrap_or(text.len())
            .min(text.len())
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
    Some(
        inline::to_asciidoc(&inline::from_node(&fragment))
            .trim_start()
            .len(),
    )
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
        return target
            .parent_element()?
            .closest(&format!("[{LINE}]"))
            .ok()?;
    }

    target
        .unchecked_into::<Element>()
        .closest(&format!("[{LINE}]"))
        .ok()?
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
