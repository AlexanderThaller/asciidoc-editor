//! Turning rendered inline HTML back into AsciiDoc.
//!
//! The rich-text surface lets the browser edit rendered HTML directly, so an
//! edited block has to be written back as AsciiDoc. Rather than walk the DOM
//! and emit text in one pass, the DOM is first read into this small tree; the
//! tree is what the tests exercise, and it is also what tells us whether a
//! block can be edited at all (see [`crate::wysiwyg`]).

use wasm_bindgen::JsCast;
use web_sys::{Element, Node};

/// An inline construct the rich-text surface understands.
///
/// Anything absent from this list — footnotes, images, attribute references,
/// character substitutions — is why a block may be refused for editing: what
/// cannot be represented here cannot be written back faithfully.
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Code(Vec<Inline>),
    Link { href: String, children: Vec<Inline> },
    LineBreak,
}

/// Writes a tree back out as AsciiDoc.
pub fn to_asciidoc(nodes: &[Inline]) -> String {
    let mut out = String::new();

    for node in nodes {
        match node {
            // Browsers pad an edit with non-breaking spaces to hold the
            // spacing around it. They look like spaces, so writing them into
            // the source leaves invisible characters that AsciiDoc does not
            // treat as word boundaries.
            Inline::Text(text) => out.push_str(&text.replace('\u{a0}', " ")),
            Inline::Strong(children) => wrap(&mut out, "*", children),
            Inline::Emphasis(children) => wrap(&mut out, "_", children),
            Inline::Code(children) => wrap(&mut out, "`", children),
            Inline::LineBreak => out.push('\n'),
            Inline::Link { href, children } => {
                let text = to_asciidoc(children);

                // A recognised target shown as itself renders from the bare
                // URL; anything else needs the macro, and a target AsciiDoc
                // would not recognise needs it spelled out.
                if text == *href && crate::source::is_linkable(href) {
                    out.push_str(href);
                } else {
                    out.push_str(&crate::source::link_macro(href, &text));
                }
            }
        }
    }

    out
}

/// Wraps formatted content in its markers, dropping empty formatting outright
/// — an empty `**` would be literal text rather than emphasis.
fn wrap(out: &mut String, marker: &str, children: &[Inline]) {
    let text = to_asciidoc(children);
    if text.is_empty() {
        return;
    }

    out.push_str(marker);
    out.push_str(&text);
    out.push_str(marker);
}

/// Reads the inline content of a rendered element.
pub fn from_node(node: &Node) -> Vec<Inline> {
    let mut out = Vec::new();
    collect(node, &mut out);
    out
}

/// Reads a run of sibling nodes, rather than the children of one parent.
///
/// List items mix inline content with nested lists, so their content has to be
/// read node by node.
pub fn from_nodes(nodes: &[Node]) -> Vec<Inline> {
    let mut out = Vec::new();
    for node in nodes {
        visit(node, &mut out);
    }
    out
}

fn collect(parent: &Node, out: &mut Vec<Inline>) {
    let children = parent.child_nodes();

    for index in 0..children.length() {
        if let Some(node) = children.item(index) {
            visit(&node, out);
        }
    }
}

fn visit(node: &Node, out: &mut Vec<Inline>) {
    match node.node_type() {
        Node::TEXT_NODE => {
            if let Some(text) = node.text_content() {
                out.push(Inline::Text(text));
            }
        }
        Node::ELEMENT_NODE => {
            // Cast unchecked: nodes from the preview iframe belong to its own
            // realm, where an `instanceof` check would fail.
            let element: &Element = node.unchecked_ref();

            // A block title is a label the renderer places inside the block —
            // an admonition keeps one in the same cell as its text — and is
            // written back from its own line, not as part of this content.
            if element.class_list().contains("title") {
                return;
            }

            let mut children = Vec::new();
            collect(node, &mut children);

            match element.tag_name().to_ascii_uppercase().as_str() {
                // `execCommand` emits `b`/`i`; the renderer emits `strong`/`em`.
                "STRONG" | "B" => out.push(Inline::Strong(children)),
                "EM" | "I" => out.push(Inline::Emphasis(children)),
                "CODE" => out.push(Inline::Code(children)),
                "BR" => out.push(Inline::LineBreak),
                "A" => out.push(Inline::Link {
                    href: element.get_attribute("href").unwrap_or_default(),
                    children,
                }),
                // Unknown wrappers contribute their content but not themselves,
                // so a stray `span` cannot break a round trip.
                _ => out.extend(children),
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_string())
    }

    #[test]
    fn writes_formatting_markers() {
        let tree = vec![
            text("plain "),
            Inline::Strong(vec![text("bold")]),
            text(" "),
            Inline::Emphasis(vec![text("italic")]),
            text(" "),
            Inline::Code(vec![text("mono")]),
        ];

        assert_eq!(to_asciidoc(&tree), "plain *bold* _italic_ `mono`");
    }

    #[test]
    fn nests_formatting() {
        let tree = vec![Inline::Strong(vec![
            text("bold "),
            Inline::Emphasis(vec![text("and italic")]),
        ])];

        assert_eq!(to_asciidoc(&tree), "*bold _and italic_*");
    }

    #[test]
    fn drops_empty_formatting() {
        assert_eq!(to_asciidoc(&[Inline::Strong(vec![])]), "");
        assert_eq!(to_asciidoc(&[Inline::Code(vec![text("")])]), "");
    }

    #[test]
    fn writes_bare_and_labelled_links() {
        let bare = Inline::Link {
            href: "https://example.com".to_string(),
            children: vec![text("https://example.com")],
        };
        let labelled = Inline::Link {
            href: "https://example.com".to_string(),
            children: vec![text("Example")],
        };

        assert_eq!(to_asciidoc(&[bare]), "https://example.com");
        assert_eq!(to_asciidoc(&[labelled]), "https://example.com[Example]");
    }

    /// A target AsciiDoc would read as text rather than as a link has to keep
    /// its macro, or the round trip would quietly turn the link into words.
    #[test]
    fn spells_out_links_to_targets_without_a_scheme() {
        let relative = Inline::Link {
            href: "guide.html".to_string(),
            children: vec![text("The guide")],
        };

        assert_eq!(to_asciidoc(&[relative]), "link:guide.html[The guide]");
    }

    #[test]
    fn normalises_non_breaking_spaces() {
        let padded = vec![
            text("before\u{a0}"),
            Inline::Code(vec![text("x")]),
            text("\u{a0}after"),
        ];

        assert_eq!(to_asciidoc(&padded), "before `x` after");
    }

    #[test]
    fn line_breaks_become_newlines() {
        let tree = vec![text("one"), Inline::LineBreak, text("two")];

        assert_eq!(to_asciidoc(&tree), "one\ntwo");
    }
}
