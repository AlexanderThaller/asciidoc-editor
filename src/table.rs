//! Reading and writing the rows of a delimited table.
//!
//! A table's cells have no source lines of their own — the whole `|===` block
//! is one block — so editing any cell rewrites every row. To keep that from
//! reflowing a table the moment a word changes, the source's own layout is
//! recorded as a [`Shape`] and the cells are poured back into it.

use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::inline;

/// One line of a table's source: a row of so many cells, or a blank line
/// separating groups of rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Blank,
    Row(usize),
}

/// How a table's rows were laid out in the source.
pub type Shape = Vec<Slot>;

/// Reads the cells between a table's delimiters, with the layout they were
/// written in.
///
/// Returns `None` for anything this module cannot write back: a cell
/// specifier (`2+|`, `a|`, `^|`) changes what a cell *means*, and re-emitting
/// the text without it would quietly drop a span, an alignment or an embedded
/// block.
pub fn parse(rows: &str) -> Option<(Vec<String>, Shape)> {
    let mut cells = Vec::new();
    let mut shape = Shape::new();

    for line in rows.lines() {
        if line.trim().is_empty() {
            shape.push(Slot::Blank);
            continue;
        }

        let row = split_cells(line)?;
        shape.push(Slot::Row(row.len()));
        cells.extend(row);
    }

    Some((cells, shape))
}

/// Splits one row line into its cells.
fn split_cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    if !line.starts_with('|') {
        return None;
    }

    let mut cells = Vec::new();
    let mut current = String::new();
    let mut characters = line.chars().peekable();

    // The leading separator opens the first cell rather than closing one.
    characters.next();
    let mut previous = ' ';

    while let Some(character) = characters.next() {
        if character == '\\' && characters.peek() == Some(&'|') {
            characters.next();
            current.push('|');
            previous = '|';
            continue;
        }

        if character == '|' {
            // A separator always follows whitespace. Anything else in front of
            // it is a cell specifier, which this module will not rewrite.
            if !previous.is_whitespace() {
                return None;
            }

            cells.push(current.trim().to_string());
            current = String::new();
            previous = '|';
            continue;
        }

        current.push(character);
        previous = character;
    }

    cells.push(current.trim().to_string());
    Some(cells)
}

/// Writes cells back as the rows of a table.
///
/// The recorded shape is used when it still accounts for every cell; once rows
/// or columns have been added or removed it cannot, and the table is written
/// out one row per line instead.
pub fn to_asciidoc(cells: &[String], shape: &Shape, columns: usize, header: bool) -> String {
    let accounted: usize = shape
        .iter()
        .map(|slot| match slot {
            Slot::Row(cells) => *cells,
            Slot::Blank => 0,
        })
        .sum();

    if accounted == cells.len() {
        let mut remaining = cells.iter();
        let lines: Vec<String> = shape
            .iter()
            .map(|slot| match slot {
                Slot::Blank => String::new(),
                Slot::Row(count) => row_line(remaining.by_ref().take(*count)),
            })
            .collect();

        return lines.join("\n");
    }

    let mut lines = Vec::new();
    for (index, row) in cells.chunks(columns.max(1)).enumerate() {
        lines.push(row_line(row.iter()));

        // A blank line after the first row is what marks it as the header.
        if header && index == 0 {
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

fn row_line<'a>(cells: impl Iterator<Item = &'a String>) -> String {
    let line = cells
        .map(|cell| format!("| {}", escape(cell)))
        .collect::<Vec<_>>()
        .join(" ");

    // An empty cell would otherwise leave its separator trailing whitespace.
    line.trim_end().to_string()
}

/// A cell is one line, and a bare `|` would start a new one.
fn escape(cell: &str) -> String {
    cell.replace('|', "\\|")
        .replace('\n', " ")
        .trim()
        .to_string()
}

/// Records a shape so it can travel on the element it belongs to.
pub fn encode(shape: &Shape) -> String {
    shape
        .iter()
        .map(|slot| match slot {
            Slot::Blank => "b".to_string(),
            Slot::Row(cells) => cells.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn decode(text: &str) -> Shape {
    text.split(',')
        .filter_map(|entry| match entry {
            "b" => Some(Slot::Blank),
            cells => cells.parse().ok().map(Slot::Row),
        })
        .collect()
}

/// Reads the cells of a rendered table, in order.
pub fn cells_from_dom(table: &Element) -> Vec<String> {
    rows_of(table)
        .flat_map(|row| {
            cells_of(&row)
                .map(|cell| {
                    inline::to_asciidoc(&inline::from_node(&cell))
                        .trim()
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// How many cells the table's widest row has.
pub fn columns(table: &Element) -> usize {
    rows_of(table)
        .map(|row| cells_of(&row).count())
        .max()
        .unwrap_or(0)
}

/// Whether the table's first row is a header.
pub fn has_header(table: &Element) -> bool {
    table.query_selector("thead tr").ok().flatten().is_some()
}

pub fn rows_of(table: &Element) -> impl Iterator<Item = Element> + use<> {
    let rows = table.query_selector_all("tr").ok();
    let length = rows.as_ref().map_or(0, |rows| rows.length());

    (0..length).filter_map(move |index| {
        rows.as_ref()
            .and_then(|rows| rows.item(index))
            .map(JsCast::unchecked_into)
    })
}

pub fn cells_of(row: &Element) -> impl Iterator<Item = Element> + use<> {
    let cells = row.query_selector_all("th, td").ok();
    let length = cells.as_ref().map_or(0, |cells| cells.length());

    (0..length).filter_map(move |index| {
        cells
            .as_ref()
            .and_then(|cells| cells.item(index))
            .map(JsCast::unchecked_into)
    })
}

/// The cell an event happened in, and where it sits in the table.
pub fn position(cell: &Element) -> Option<(usize, usize)> {
    let row = cell.parent_element()?;
    let table = row.closest("table").ok()??;

    let column = cells_of(&row).position(|candidate| candidate == *cell)?;
    let index = rows_of(&table).position(|candidate| candidate == row)?;

    Some((index, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn reads_rows_and_their_layout() {
        let rows = "| Crate | Role\n\n| parser\n| Parses\n\n| renderer\n| Renders";

        let (found, shape) = parse(rows).expect("plain cells");

        assert_eq!(
            found,
            cells(&["Crate", "Role", "parser", "Parses", "renderer", "Renders"])
        );
        assert_eq!(
            shape,
            vec![
                Slot::Row(2),
                Slot::Blank,
                Slot::Row(1),
                Slot::Row(1),
                Slot::Blank,
                Slot::Row(1),
                Slot::Row(1)
            ]
        );
    }

    #[test]
    fn refuses_cell_specifiers() {
        assert_eq!(parse("| a 2+| spans two"), None);
        assert_eq!(parse("| a a| an asciidoc cell"), None);
        assert_eq!(parse("| a ^| centred"), None);
        assert_eq!(parse("not a row"), None);
    }

    #[test]
    fn reads_escaped_separators() {
        let (found, _) = parse("| a \\| b | c").expect("escaped pipe");

        assert_eq!(found, cells(&["a | b", "c"]));
    }

    #[test]
    fn writes_cells_back_into_the_layout_they_came_from() {
        let rows = "| Crate | Role\n\n| parser\n| Parses";
        let (found, shape) = parse(rows).expect("plain cells");

        assert_eq!(to_asciidoc(&found, &shape, 2, true), rows);
    }

    #[test]
    fn falls_back_to_one_row_per_line_when_the_shape_no_longer_fits() {
        let (_, shape) = parse("| a | b").expect("plain cells");
        let grown = cells(&["a", "b", "c", "d"]);

        assert_eq!(to_asciidoc(&grown, &shape, 2, false), "| a | b\n| c | d");
    }

    #[test]
    fn keeps_the_header_separated_when_reflowing() {
        let (_, shape) = parse("| a | b").expect("plain cells");
        let grown = cells(&["a", "b", "c", "d"]);

        assert_eq!(to_asciidoc(&grown, &shape, 2, true), "| a | b\n\n| c | d");
    }

    #[test]
    fn a_shape_survives_being_written_down() {
        let (_, shape) = parse("| a | b\n\n| c\n| d").expect("plain cells");

        assert_eq!(encode(&shape), "2,b,1,1");
        assert_eq!(decode(&encode(&shape)), shape);
    }

    #[test]
    fn leaves_no_trailing_space_after_an_empty_cell() {
        let (_, shape) = parse("| a | b").expect("plain cells");
        let with_empties = cells(&["a", "b", "", ""]);

        assert_eq!(
            to_asciidoc(&with_empties, &shape, 2, false),
            "| a | b\n|  |"
        );
    }

    #[test]
    fn escapes_separators_and_flattens_line_breaks() {
        let awkward = cells(&["a | b", "two\nlines"]);
        let (_, shape) = parse("| x | y").expect("plain cells");

        assert_eq!(
            to_asciidoc(&awkward, &shape, 2, false),
            "| a \\| b | two lines"
        );
    }
}
