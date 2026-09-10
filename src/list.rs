//! Turning a rendered list back into AsciiDoc.
//!
//! A list is edited as a single block rather than item by item: the rendered
//! `ul`/`ol` is what becomes editable, so the browser's own list behaviour —
//! Enter starting an item, Backspace merging one — works untouched, and the
//! whole list is written back over the source lines it came from.
//!
//! This matters because the renderer marks the *list* with a source line, not
//! its items; an individual item has no line of its own to write back to.

use wasm_bindgen::JsCast;
use web_sys::{Element, Node};

use crate::inline::{self, Inline};

/// A rendered list, as far as the rich-text surface understands one.
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub ordered: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub content: Vec<Inline>,
    pub nested: Option<List>,
}

/// Writes a list back out, one line per item.
///
/// `marker` is the character the source already uses (`*` or `-` for unordered
/// lists), so that editing a list does not rewrite its style. Nesting repeats
/// the marker, which is how AsciiDoc expresses depth.
pub fn to_asciidoc(list: &List, marker: char) -> String {
    let mut lines = Vec::new();
    write(list, marker, 1, &mut lines);
    lines.join("\n")
}

fn write(list: &List, marker: char, depth: usize, lines: &mut Vec<String>) {
    // Only unordered lists carry a style worth preserving; ordered items are
    // always written with `.`, whatever numbering the source used.
    let marker = if list.ordered { '.' } else { marker };

    for item in &list.items {
        let text = inline::to_asciidoc(&item.content);
        let text = text.trim();

        // An item with neither text nor children is one the user has started
        // but not filled in; it has nothing to write.
        if text.is_empty() && item.nested.is_none() {
            continue;
        }

        lines.push(format!("{} {}", marker.to_string().repeat(depth), text));

        if let Some(nested) = &item.nested {
            write(nested, marker, depth + 1, lines);
        }
    }
}

/// Reads a rendered `ul` or `ol`.
pub fn from_element(element: &Element) -> Option<List> {
    let ordered = match element.tag_name().to_ascii_uppercase().as_str() {
        "UL" => false,
        "OL" => true,
        _ => return None,
    };

    let items = children(element)
        .filter(|node| is_element(node, "LI"))
        .map(|node| item_from_li(&node))
        .collect();

    Some(List { ordered, items })
}

fn item_from_li(li: &Node) -> Item {
    let mut content = Vec::new();
    let mut nested = None;

    for child in children(li) {
        match as_list(&child) {
            Some(list) => nested = Some(list),
            // Held back and read together so that a run of text and inline
            // elements is not split into fragments.
            None => content.push(child),
        }
    }

    Item {
        content: inline::from_nodes(&content),
        nested,
    }
}

/// Reads a node as a nested list, seeing through the wrapper the renderer puts
/// around one (`<div class="ulist"><ul>…`).
fn as_list(node: &Node) -> Option<List> {
    if node.node_type() != Node::ELEMENT_NODE {
        return None;
    }

    let element: &Element = node.unchecked_ref();
    match element.tag_name().to_ascii_uppercase().as_str() {
        "UL" | "OL" => from_element(element),
        "DIV" => children(node).filter_map(|child| as_list(&child)).next(),
        _ => None,
    }
}

fn is_element(node: &Node, tag: &str) -> bool {
    node.node_type() == Node::ELEMENT_NODE
        && node
            .unchecked_ref::<Element>()
            .tag_name()
            .to_ascii_uppercase()
            == tag
}

fn children(node: &Node) -> impl Iterator<Item = Node> + use<> {
    let nodes = node.child_nodes();
    (0..nodes.length()).filter_map(move |index| nodes.item(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> Item {
        Item {
            content: vec![Inline::Text(text.to_string())],
            nested: None,
        }
    }

    fn list(items: Vec<Item>) -> List {
        List {
            ordered: false,
            items,
        }
    }

    #[test]
    fn writes_one_line_per_item() {
        let list = list(vec![item("one"), item("two")]);

        assert_eq!(to_asciidoc(&list, '*'), "* one\n* two");
    }

    #[test]
    fn keeps_the_marker_the_source_uses() {
        assert_eq!(to_asciidoc(&list(vec![item("one")]), '-'), "- one");
    }

    #[test]
    fn ordered_items_are_always_dots() {
        let ordered = List {
            ordered: true,
            items: vec![item("first"), item("second")],
        };

        assert_eq!(to_asciidoc(&ordered, '*'), ". first\n. second");
    }

    #[test]
    fn nesting_repeats_the_marker() {
        let nested = List {
            ordered: false,
            items: vec![
                Item {
                    content: vec![Inline::Text("outer".to_string())],
                    nested: Some(list(vec![item("inner")])),
                },
                item("after"),
            ],
        };

        assert_eq!(to_asciidoc(&nested, '*'), "* outer\n** inner\n* after");
    }

    #[test]
    fn keeps_inline_formatting() {
        let formatted = list(vec![Item {
            content: vec![
                Inline::Text("a ".to_string()),
                Inline::Code(vec![Inline::Text("code".to_string())]),
            ],
            nested: None,
        }]);

        assert_eq!(to_asciidoc(&formatted, '*'), "* a `code`");
    }

    /// The renderer indents its markup, so items arrive padded with newlines.
    #[test]
    fn trims_surrounding_whitespace() {
        let padded = list(vec![Item {
            content: vec![Inline::Text("\n  text  \n".to_string())],
            nested: None,
        }]);

        assert_eq!(to_asciidoc(&padded, '*'), "* text");
    }

    /// Pressing Enter creates an item before there is anything to put in it.
    #[test]
    fn skips_items_with_nothing_in_them() {
        let with_empty = list(vec![item("one"), item("  "), item("two")]);

        assert_eq!(to_asciidoc(&with_empty, '*'), "* one\n* two");
    }
}
