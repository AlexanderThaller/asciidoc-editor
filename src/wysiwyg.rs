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
use web_sys::{Document, Element, Event, HtmlDocument, KeyboardEvent, Node, Range};

use crate::{
    inline, list,
    source::{self, LineRange},
    table,
};

/// The block the caret is currently in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub line: usize,
    /// Last line of the block's source.
    pub end: usize,
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
    /// The attribution under a quote or a verse, which lives in the block's
    /// attribute line rather than in its body.
    Attribution(&'static str),
    /// An image block. It has no text to edit, so it is focused rather than
    /// typed into, and changed through the panel that made it.
    Image,
    /// A cell of a table. Every cell writes the whole table back.
    Table {
        /// Whether the column count is pinned by a `cols` attribute that this
        /// module will not rewrite.
        fixed_columns: bool,
    },
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
/// Marks an attribution, holding the style whose line it belongs to.
const ATTRIBUTION: &str = "data-edit-attribution";
/// Text the renderer generates in front of a title, such as a table's number.
const PREFIX: &str = "data-edit-prefix";
/// Text the source carries in front of the content but the rendering drops,
/// such as an admonition's `NOTE: ` label.
const LEAD: &str = "data-edit-lead";
/// How a table's rows were laid out in the source.
const SHAPE: &str = "data-edit-shape";
/// Marks the block that a new one would be added below.
const INSERT: &str = "data-edit-insert-after";
/// Set while a panel is open, to keep the rendering still under it.
const HELD: &str = "data-edit-held";

/// Holds the document still, or lets it settle again.
pub fn hold(document: &Document, held: bool) {
    let Some(body) = document.body() else { return };

    if held {
        let _ = body.set_attribute(HELD, "true");
    } else {
        let _ = body.remove_attribute(HELD);
    }
}

/// An image block, along with what it points at and how it is described.
const IMAGE: &str = "data-edit-image";
const IMAGE_ALT: &str = "data-edit-image-alt";
/// Marks a table whose `cols` attribute cannot be kept in step, and whose
/// columns therefore cannot be changed here.
const FIXED: &str = "data-edit-fixed-columns";

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

    for block in select(
        content,
        "div.quoteblock[data-source-line], div.verseblock[data-source-line]",
    ) {
        let (Some(line), Ok(Some(attribution))) = (
            line_of(&block),
            block.query_selector(":scope > .attribution"),
        ) else {
            continue;
        };

        let range = LineRange::single(line);
        let Some((style, author)) = source::attribution(&source::text_of(src, range)) else {
            let _ = attribution.set_attribute("title", REFUSED);
            continue;
        };

        // The dash in front is the renderer's, not the document's.
        let prefix = serialize(&attribution)
            .strip_suffix(author.trim_end())
            .unwrap_or_default()
            .to_string();
        if !prefix.is_empty() {
            let _ = attribution.set_attribute(PREFIX, &prefix);
        }

        let _ = attribution.set_attribute(ATTRIBUTION, style);
        offer(
            &attribution,
            range,
            Kind::Attribution(style),
            &format!("{prefix}{author}"),
        );
    }

    for image in select(content, "div.imageblock[data-source-line]") {
        let Some(line) = line_of(&image) else {
            continue;
        };

        let range = source::paragraph_range(src, line);
        let Some((target, alt)) = source::image_parts(&source::text_of(src, range)) else {
            let _ = image.set_attribute("title", REFUSED);
            continue;
        };

        // Nothing here is typed into, so the block is focusable rather than
        // editable: the panel that made it is where it changes.
        let _ = image.set_attribute("tabindex", "0");
        let _ = image.set_attribute(IMAGE, &target);
        let _ = image.set_attribute(IMAGE_ALT, &alt);
        let _ = image.set_attribute(LINE, &range.start.to_string());
        let _ = image.set_attribute(END, &range.end.to_string());
    }

    for table in select(content, "table[data-source-line]") {
        let (Some(line), rendered) = (line_of(&table), table::cells_from_dom(&table)) else {
            continue;
        };

        let Some(range) = source::table_range(src, line) else {
            continue;
        };

        // The cells must read back exactly as the source wrote them; a
        // specifier the source carries would be dropped by rewriting.
        let Some((written, shape)) = table::parse(&source::text_of(src, range)) else {
            continue;
        };
        if rendered != written {
            continue;
        }

        // A cell has no line of its own, so each carries the range of the rows
        // as a whole and writes all of them back.
        let shape = table::encode(&shape);
        let fixed = source::columns_attribute(src, line) == source::Columns::Opaque;

        for row in table::rows_of(&table) {
            for cell in table::cells_of(&row) {
                let _ = cell.set_attribute(SHAPE, &shape);
                if fixed {
                    let _ = cell.set_attribute(FIXED, "true");
                }
                make_editable(
                    &cell,
                    range,
                    Kind::Table {
                        fixed_columns: fixed,
                    },
                );
            }
        }
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

        // A title sits in the same cell as the text, and the browser treats
        // that cell as one editing host — a nested `contenteditable` inside it
        // is not a second one. So the cell owns the title's line as well, and
        // writes both back together.
        let range = match block_title_in(&body) {
            Some(title) if source::block_title_line(src, line) == Some(range.start - 1) => {
                let written = source::text_of(src, LineRange::single(range.start - 1));
                if title.text_content().unwrap_or_default() != source::block_title_text(&written) {
                    continue;
                }
                LineRange {
                    start: range.start - 1,
                    end: range.end,
                }
            }
            _ => range,
        };

        offer(&body, range, Kind::Admonition(label), &text[lead.len()..]);
    }

    for block in select(content, "[data-source-line]") {
        let (Some(line), Ok(Some(title))) =
            // An admonition keeps its title inside the content cell rather
            // than beside it.
            (
                line_of(&block),
                block.query_selector(":scope > .title, :scope td.content > .title"),
            )
        else {
            continue;
        };

        // A title inside an editing host is edited through that host, since
        // nesting one inside another is not something the browser honours. A
        // block that is merely focusable — an image — is not one, and its
        // title is marked in its own right.
        if title
            .closest("[contenteditable=\"true\"]")
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }

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
    if let Some(cells) = table_of(element) {
        return cells;
    }

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

    // A title marked in its own right knows which line it came from.
    if let Some(title) = block_title_in(block).filter(|title| title.has_attribute(TITLE)) {
        return attr(&title, LINE);
    }

    // One the block holds instead of marking — an admonition's — has no line
    // of its own: it is the block's first.
    if block_title_in(block).is_some() {
        return attr(block, LINE);
    }

    // A cell's own line is a row; the table's title is its caption.
    if block.has_attribute(SHAPE) {
        return table_of_cell(block)
            .and_then(|table| {
                table
                    .query_selector(&format!("caption[{LINE}]"))
                    .ok()
                    .flatten()
            })
            .and_then(|caption| attr(&caption, LINE));
    }

    let beside = block.parent_element().and_then(|parent| {
        parent
            .query_selector(&format!(":scope > [{TITLE}]"))
            .ok()
            .flatten()
    });

    // Beside the block for most kinds, inside it for an admonition.
    let title = beside.or_else(|| {
        block
            .query_selector(&format!(":scope > [{TITLE}]"))
            .ok()
            .flatten()
    })?;

    attr(&title, LINE)
}

/// The block title a block carries inside itself, as an admonition does.
fn block_title_in(block: &Element) -> Option<Element> {
    block.query_selector(":scope > .title").ok()?
}

fn kind_of(block: &Element) -> Kind {
    if block.has_attribute(TITLE) {
        return Kind::Title;
    }

    if let Some(style) = block.get_attribute(ATTRIBUTION).and_then(|style| {
        source::ATTRIBUTED
            .iter()
            .copied()
            .find(|known| *known == style)
    }) {
        return Kind::Attribution(style);
    }

    if block.has_attribute(IMAGE) {
        return Kind::Image;
    }

    if block.has_attribute(SHAPE) {
        return Kind::Table {
            fixed_columns: block.has_attribute(FIXED),
        };
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

fn table_of_cell(cell: &Element) -> Option<Element> {
    cell.closest("table").ok()?
}

/// Adds a row under the focused cell's row, or takes that row away.
///
/// Either way the row count changes, so the recorded layout no longer fits and
/// the table is written out afresh.
pub fn table_row<R>(document: &Document, source: RwSignal<String>, add: bool, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(cell) = focused(document).filter(|cell| cell.has_attribute(SHAPE)) else {
        return;
    };
    let (Some(table), Some((row, _))) = (table_of_cell(&cell), table::position(&cell)) else {
        return;
    };

    let columns = table::columns(&table).max(1);
    let mut cells = table::cells_from_dom(&table);
    let start = row * columns;

    if add {
        let after = (start + columns).min(cells.len());
        for _ in 0..columns {
            cells.insert(after, String::new());
        }
    } else {
        // A table needs a row; removing the last one would leave delimiters
        // around nothing, which is no longer a table this module can find.
        if cells.len() <= columns {
            return;
        }
        cells.drain(start..(start + columns).min(cells.len()));
    }

    let (Some(first), Some(last)) = (attr(&cell, LINE), attr(&cell, END)) else {
        return;
    };

    let rows = table::to_asciidoc(
        &cells,
        &table::decode(&cell.get_attribute(SHAPE).unwrap_or_default()),
        columns,
        table::has_header(&table),
    );

    source.set(source::replace(
        &source.get_untracked(),
        LineRange {
            start: first,
            end: last,
        },
        &rows,
    ));

    rerender(Some(first));
}

/// Adds a column beside the focused cell's column, or takes that column away.
///
/// A `cols` attribute describes the columns, so it has to change with them.
pub fn table_column<R>(document: &Document, source: RwSignal<String>, add: bool, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(cell) = focused(document).filter(|cell| cell.has_attribute(SHAPE)) else {
        return;
    };
    let (Some(table), Some((_, column)), Some(block)) = (
        table_of_cell(&cell),
        table::position(&cell),
        table_of_cell(&cell).and_then(|table| line_of(&table)),
    ) else {
        return;
    };

    let columns = table::columns(&table).max(1);
    if !add && columns <= 1 {
        // The last column is the table.
        return;
    }

    let mut cells = table::cells_from_dom(&table);
    let rows = cells.len().div_ceil(columns);

    // From the last row up, so that the indices ahead stay put.
    for row in (0..rows).rev() {
        let at = row * columns + column;
        if add {
            let after = (at + 1).min(cells.len());
            cells.insert(after, String::new());
        } else if at < cells.len() {
            cells.remove(at);
        }
    }

    let widened = if add { columns + 1 } else { columns - 1 };
    let (Some(first), Some(last)) = (attr(&cell, LINE), attr(&cell, END)) else {
        return;
    };

    let rows_text = table::to_asciidoc(
        &cells,
        &table::decode(&cell.get_attribute(SHAPE).unwrap_or_default()),
        widened,
        table::has_header(&table),
    );

    let edited = source::replace(
        &source.get_untracked(),
        LineRange {
            start: first,
            end: last,
        },
        &rows_text,
    );

    // The attribute sits above the rows, so its line is unmoved by the rewrite.
    let edited = match source::columns_attribute(&source.get_untracked(), block) {
        source::Columns::Widths { line, mut values } if values.len() == columns => {
            if add {
                values.insert(column + 1, "1".to_string());
            } else {
                values.remove(column);
            }

            let attribute = source::text_of(&edited, LineRange::single(line));
            source::replace(
                &edited,
                LineRange::single(line),
                &source::with_columns(&attribute, &values),
            )
        }
        _ => edited,
    };

    source.set(edited);
    rerender(Some(first));
}

/// What an image block points at, and how it is described.
pub fn image_of(block: &Element) -> Option<(String, String)> {
    Some((
        block.get_attribute(IMAGE)?,
        block.get_attribute(IMAGE_ALT).unwrap_or_default(),
    ))
}

/// Points an image block at something else, or describes it differently.
pub fn update_image<R>(
    source: RwSignal<String>,
    block: Block,
    target: &str,
    alt: &str,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    source.set(source::replace(
        &source.get_untracked(),
        LineRange {
            start: block.line,
            end: block.end,
        },
        &source::image_macro(target, alt),
    ));

    rerender(Some(block.line));
}

/// Removes the focused image, and any title that came with it.
pub fn remove_image<R>(document: &Document, source: RwSignal<String>, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(block) = focused(document).filter(|block| block.has_attribute(IMAGE)) else {
        return;
    };

    // From the block's own first line, so an attached title goes with it.
    let (Some(start), Some(end)) = (line_of(&block), attr(&block, END)) else {
        return;
    };

    source.set(source::remove_lines(
        &source.get_untracked(),
        LineRange { start, end },
    ));
    rerender(None);
}

/// Adds a block of its own below the block ending at `after`, or at the end of
/// the document when there is none.
///
/// The caller says where rather than the focus, which by the time a panel has
/// been filled in has moved out of the document entirely.
pub fn insert_block_below<R>(
    source: RwSignal<String>,
    after: Option<usize>,
    text: &str,
    rerender: &R,
) where
    R: Fn(Option<usize>),
{
    // Past the block and the blank line that closes it.
    let below = after.map_or(usize::MAX, |end| end + 2);

    source.set(source::insert_block(&source.get_untracked(), below, text));
    rerender(None);
}

/// Marks the block a new one would go below, and clears any earlier mark.
pub fn mark_insertion_point(document: &Document, line: Option<usize>) -> Option<()> {
    let content = content_of(document)?;

    for marked in select(&content, &format!("[{INSERT}]")) {
        let _ = marked.remove_attribute(INSERT);
    }

    let target = line?;
    let block = content
        .query_selector(&format!("[{LINE}=\"{target}\"]"))
        .ok()??;

    block.set_attribute(INSERT, "true").ok()
}

/// Removes the focused cell's table outright.
pub fn remove_table<R>(document: &Document, source: RwSignal<String>, rerender: &R)
where
    R: Fn(Option<usize>),
{
    let Some(cell) = focused(document).filter(|cell| cell.has_attribute(SHAPE)) else {
        return;
    };
    let Some(block) = table_of_cell(&cell).and_then(|table| line_of(&table)) else {
        return;
    };
    let Some(range) = source::table_block_range(&source.get_untracked(), block) else {
        return;
    };

    source.set(source::remove_lines(&source.get_untracked(), range));
    rerender(None);
}

/// The whole table a cell belongs to, written out as rows.
fn table_of(cell: &Element) -> Option<String> {
    let shape = cell.get_attribute(SHAPE)?;
    let table = cell.closest("table").ok()??;

    Some(table::to_asciidoc(
        &table::cells_from_dom(&table),
        &table::decode(&shape),
        table::columns(&table),
        table::has_header(&table),
    ))
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
    write_block(block, content, source, &serialize(block));
}

/// Writes `written` back as the block's source, in whatever shape its kind
/// takes: a title's leading dot, a heading's `=`, an admonition's label.
fn write_block(block: &Element, content: &Element, source: RwSignal<String>, written: &str) {
    let Some(start) = attr(block, LINE) else {
        return;
    };
    let text = match kind_of(block) {
        Kind::Title => {
            let written = written.to_string();
            // Drop the generated prefix again. If the caret wandered into it
            // there is nothing to strip, and what the user typed is used whole.
            let prefix = block.get_attribute(PREFIX).unwrap_or_default();
            source::as_title(written.strip_prefix(&prefix).unwrap_or(&written))
        }
        Kind::Heading(level) => source::as_block(written, Some(level)),
        Kind::Attribution(style) => {
            let prefix = block.get_attribute(PREFIX).unwrap_or_default();
            source::with_attribution(style, written.strip_prefix(&prefix).unwrap_or(written))
        }
        // The label is part of the source line but not of what is rendered,
        // and a title of its own occupies the line above.
        Kind::Admonition(_) => {
            let text = format!(
                "{}{}",
                block.get_attribute(LEAD).unwrap_or_default(),
                written
            );

            match block_title_in(block) {
                Some(title) => format!(
                    "{}\n{text}",
                    source::as_title(&title.text_content().unwrap_or_default())
                ),
                None => text,
            }
        }
        _ => written.to_string(),
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
pub fn attach<R, T>(
    document: &Document,
    content: Element,
    source: RwSignal<String>,
    editing: RwSignal<Option<Block>>,
    rerender: R,
    travel: T,
) where
    R: Fn(Option<usize>) + Clone + 'static,
    T: Fn(bool) + 'static,
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
                end: attr(&block, END).or_else(|| attr(&block, LINE))?,
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

                    // A panel takes focus while pointing at something in the
                    // document; re-rendering now would replace what it points
                    // at.
                    let held = document.body().is_some_and(|body| body.has_attribute(HELD));

                    if !still_editing && !held {
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

            // A block is its own editing host, so the arrow keys stop at its
            // edges. Carry them across to the next block, which is the only
            // way out of a list without reaching for the mouse.
            if matches!(ev.key().as_str(), "ArrowDown" | "ArrowUp")
                && !ev.shift_key()
                && !ev.ctrl_key()
                && !ev.meta_key()
            {
                let down = ev.key() == "ArrowDown";
                // A block with no text has no caret to sit at the edge of.
                let leaving =
                    !block.has_attribute("contenteditable") || at_edge(&document, &block, down);

                if leaving && let Some(next) = neighbour(&content, &block, down) {
                    ev.prevent_default();
                    if down {
                        focus(&document, &next);
                    } else {
                        focus_end(&document, &next);
                    }
                    return;
                }
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
                // The document's own history, not the browser's: an edit here
                // may have rewritten lines the browser never saw.
                if ev.key().eq_ignore_ascii_case("z") {
                    ev.prevent_default();
                    travel(!ev.shift_key());
                    return;
                }

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
    exec(document, "styleWithCSS", Some("false"));

    if command == "code" {
        toggle_code(document);
        return;
    }

    exec(document, command, None);
}

/// Wraps the selection in monospace, or takes the wrapping off again.
///
/// `execCommand` has no monospace command, so wrapping writes the markup with
/// `insertHTML`. Unwrapping cannot do the same in reverse: replacing the
/// selection with one covering the whole element first makes the following
/// command a no-op, because a selection set from script is not the selection
/// `execCommand` acts on. `removeFormat` needs no such help — `code` is one of
/// the elements it strips — and both directions stay on the browser's undo
/// stack this way.
fn toggle_code(document: &Document) -> Option<()> {
    let selection = document.get_selection().ok()??;

    if enclosing_code(&selection).is_some() {
        exec(document, "removeFormat", None);
        return Some(());
    }

    let text = selection.to_string().as_string().unwrap_or_default();
    if text.is_empty() {
        return None;
    }

    let commands: &HtmlDocument = document.unchecked_ref();
    let _ = commands.exec_command_with_show_ui_and_value(
        "insertHTML",
        false,
        &format!("<code>{}</code>", escape(&text)),
    );
    Some(())
}

/// The monospace element the selection sits inside, if any.
fn enclosing_code(selection: &web_sys::Selection) -> Option<Element> {
    let node = selection.focus_node()?;
    let element = match node.node_type() {
        Node::ELEMENT_NODE => node.unchecked_into::<Element>(),
        _ => node.parent_element()?,
    };

    // Only within an editable block: the rendering elsewhere is not ours to
    // rewrite.
    element.closest(&format!("[{LINE}] code")).ok()?
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    // paragraph between the two. A table cell has no line to split at all.
    if matches!(kind_of(block), Kind::Title | Kind::Table { .. }) {
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

/// Whether a link can be written into this kind of block.
///
/// Anything with text can hold one; an image has none.
pub fn takes_links(kind: Kind) -> bool {
    kind != Kind::Image
}

/// The selection as it stands, and the text it covers.
///
/// The range is kept alive rather than measured: writing a link is a change
/// to the document, which every kind of block already knows how to write back
/// — so there is nothing to measure against, and lists and tables come along
/// without their markers and rows having to be accounted for.
pub fn selection_range(document: &Document) -> Option<(Range, String)> {
    let block = focused(document)?;
    if !takes_links(kind_of(&block)) {
        return None;
    }

    let selection = document.get_selection().ok()??;
    let range = selection.get_range_at(0).ok()?;

    Some((
        range.clone_range(),
        selection.to_string().as_string().unwrap_or_default(),
    ))
}

/// Writes a link over what the range covers.
pub fn insert_link<R>(
    document: &Document,
    source: RwSignal<String>,
    range: &Range,
    url: &str,
    label: &str,
    rerender: &R,
) -> Option<()>
where
    R: Fn(Option<usize>),
{
    let content = content_of(document)?;
    let anchored = range.common_ancestor_container().ok()?;
    let block = match anchored.node_type() {
        Node::ELEMENT_NODE => anchored.unchecked_into::<Element>(),
        _ => anchored.parent_element()?,
    }
    .closest(&format!("[{LINE}]"))
    .ok()??;

    let anchor = document.create_element("a").ok()?;
    anchor.set_attribute("href", url).ok()?;
    anchor.set_text_content(Some(label));

    range.delete_contents().ok()?;
    range.insert_node(&anchor).ok()?;

    // The block writes itself back as it always does, so a list keeps its
    // markers and a table its rows.
    sync_block(&block, &content, source);
    rerender(attr(&block, LINE));
    Some(())
}

/// How far into the block's AsciiDoc text the caret sits.
///
/// Serialising the content *before* the caret gives the answer directly: the
/// same function that writes the block back out defines the mapping, so the
/// two can never disagree.
fn caret_offset(document: &Document, block: &Element) -> Option<usize> {
    let selection = document.get_selection().ok()??;
    let focus_node = selection.focus_node()?;

    offset_within(document, block, &focus_node, selection.focus_offset())
}

/// Serialising the content *before* a point gives its offset directly: the
/// same function that writes the block out defines the mapping, so the two
/// can never disagree.
fn offset_within(document: &Document, block: &Element, node: &Node, offset: u32) -> Option<usize> {
    let range = document.create_range().ok()?;
    range.set_start(block, 0).ok()?;
    range.set_end(node, offset).ok()?;

    let fragment = range.clone_contents().ok()?;
    Some(
        inline::to_asciidoc(&inline::from_node(&fragment))
            .trim_start()
            .len(),
    )
}

/// Whether the caret sits on the block's first or last line.
fn at_edge(document: &Document, block: &Element, down: bool) -> bool {
    let Some(caret) = document
        .get_selection()
        .ok()
        .flatten()
        .and_then(|selection| selection.get_range_at(0).ok())
    else {
        return false;
    };

    let rect = caret.get_bounding_client_rect();
    if rect.height() == 0.0 {
        // A caret placed from script has no rectangle until the browser has
        // normalised it, which is precisely the case after arriving here from
        // the block above or below.
        return at_content_edge(document, block, &caret, down);
    }

    // Half a line of slack: a block's own padding leaves the caret a little
    // short of its edge, while the line above is a whole line away.
    let slack = rect.height() / 2.0;
    let bounds = block.get_bounding_client_rect();

    if down {
        rect.bottom() >= bounds.bottom() - slack
    } else {
        rect.top() <= bounds.top() + slack
    }
}

/// Whether the caret is at the very start or end of the block's content,
/// judged by position rather than by where it was painted.
fn at_content_edge(document: &Document, block: &Element, caret: &Range, down: bool) -> bool {
    let Ok(content) = document.create_range() else {
        return false;
    };
    if content.select_node_contents(block).is_err() {
        return false;
    }

    match down {
        true => content
            .compare_boundary_points(Range::END_TO_END, caret)
            .is_ok_and(|order| order <= 0),
        false => content
            .compare_boundary_points(Range::START_TO_START, caret)
            .is_ok_and(|order| order >= 0),
    }
}

/// The editable block before or after this one, in reading order.
fn neighbour(content: &Element, block: &Element, down: bool) -> Option<Element> {
    let blocks = select(content, &format!("[{LINE}]"));
    let index = blocks.iter().position(|candidate| candidate == block)?;

    match down {
        true => blocks.get(index + 1).cloned(),
        false => index
            .checked_sub(1)
            .and_then(|index| blocks.get(index).cloned()),
    }
}

/// Puts the caret at the end of `element`.
pub fn focus_end(document: &Document, element: &Element) -> Option<()> {
    let html: &web_sys::HtmlElement = element.unchecked_ref();
    let _ = html.focus();

    if !element.has_attribute("contenteditable") {
        return Some(());
    }

    let selection = document.get_selection().ok()??;
    let range = document.create_range().ok()?;
    range.select_node_contents(element).ok()?;
    range.collapse_with_to_start(false);
    selection.remove_all_ranges().ok()?;
    selection.add_range(&range).ok()?;
    Some(())
}

/// Puts the caret at the start of `element`.
pub fn focus(document: &Document, element: &Element) {
    let html: &web_sys::HtmlElement = element.unchecked_ref();
    let _ = html.focus();

    // An image is focused but never typed into; a caret inside it would be a
    // caret in nothing.
    if !element.has_attribute("contenteditable") {
        return;
    }

    let (Some(selection), Ok(range)) = (
        document.get_selection().ok().flatten(),
        document.create_range(),
    ) else {
        return;
    };

    // Inside the content rather than at the element's own offset zero: a caret
    // placed there has no rectangle, and the edge test has nothing to measure.
    let _ = range.select_node_contents(element);
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
