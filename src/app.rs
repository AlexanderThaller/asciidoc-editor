//! Split-pane editor: highlighted source on the left, live preview on the right.

use std::time::Duration;

use leptos::{html, prelude::*};
use wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{Element, Event, HtmlIFrameElement, HtmlInputElement, HtmlTextAreaElement, Node};

use crate::{highlight, render, source, storage, sync, wysiwyg};

/// A document as it stood, and where the caret was at the time.
#[derive(Clone, Debug)]
struct Step {
    source: String,
    line: Option<usize>,
}

/// How many steps back the editor remembers.
const HISTORY_DEPTH: usize = 200;

/// How large a file may be before embedding it stops being sensible. Base64
/// adds about a third, and every keystroke copies the whole source.
const MAX_EMBEDDED_BYTES: f64 = 2.0 * 1_048_576.0;

/// Toolbar heading buttons. `=` is the document title in AsciiDoc, so the
/// largest heading a body author writes is `==`.
const HEADINGS: [(&str, usize); 3] = [("H1", 2), ("H2", 3), ("H3", 4)];

/// The symbol each admonition draws in the preview, reused on the buttons that
/// apply it.
const ADMONITION_ICONS: [(&str, &str); 5] = [
    ("NOTE", "\u{2139}\u{fe0f}"),
    ("TIP", "\u{1f4a1}"),
    ("IMPORTANT", "\u{2757}"),
    ("WARNING", "\u{26a0}\u{fe0f}"),
    ("CAUTION", "\u{1f525}"),
];

fn icon_for(label: &str) -> &'static str {
    ADMONITION_ICONS
        .iter()
        .find(|(name, _)| *name == label)
        .map_or("\u{2139}\u{fe0f}", |(_, icon)| icon)
}

/// Re-rendering on every keystroke is wasteful; this is short enough to feel live.
const RENDER_DEBOUNCE: Duration = Duration::from_millis(150);
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Asciidoctor's own stylesheet, built in so that an embedding page has
/// nothing to serve alongside the editor.
const PREVIEW_STYLESHEET: &str = include_str!("../assets/asciidoctor-default.css");

/// The preview iframe is built once and then mutated in place. Re-assigning
/// `srcdoc` would reload it and throw away the scroll position on every render.
fn preview_shell() -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{PREVIEW_STYLESHEET}</style>{PREVIEW_STYLE}</head><body class=\"article\"><div id=\"content\"></div></body></html>"
    )
}

const PREVIEW_STYLE: &str = r#"<style>
/*
 * The body fills the frame however short the document is, so there is always
 * somewhere below the last block to click and carry on writing. The I-beam
 * says so; inside the document the usual cursors apply again.
 */
html{height:100%}
body{margin:0;padding:1.25rem 1.5rem;min-height:100%;box-sizing:border-box;cursor:text}
#content{cursor:auto}
/*
 * What can be edited, and what is being edited. An image is focusable without
 * being an editing host, so it gets none of the ring a browser draws around
 * one of those by itself.
 */
[data-edit-line]{border-radius:3px}
[data-edit-line]:hover{background:rgba(127,180,255,.08)}
[data-edit-line]:focus{outline:2px solid rgba(127,180,255,.55);outline-offset:4px}
[data-edit-line]:focus-visible{outline:2px solid rgba(127,180,255,.55);outline-offset:4px}
/*
 * Admonition icons. The renderer only emits Font Awesome glyphs when the
 * document may enable `icons`, which safe mode forbids, and a webfont would
 * not load offline anyway — so the label draws its own symbol and then hides
 * its own text: the symbol already says which kind it is. Zero font size
 * rather than `display:none`, which would take the symbol with it.
 */
.admonitionblock>table td.icon{width:3rem;text-align:center;vertical-align:top;padding-top:.1rem}
.admonitionblock>table td.icon .title{font-size:0;line-height:0}
.admonitionblock>table td.icon .title::before{display:block;font-size:1.6rem;line-height:1.2;font-style:normal}
.admonitionblock.note>table td.icon .title::before{content:"\2139\FE0F"}
.admonitionblock.tip>table td.icon .title::before{content:"\1F4A1"}
.admonitionblock.important>table td.icon .title::before{content:"\2757"}
.admonitionblock.warning>table td.icon .title::before{content:"\26A0\FE0F"}
.admonitionblock.caution>table td.icon .title::before{content:"\1F525"}
/* Where a new block would be added, while the panel asking for one is open. */
[data-edit-insert-after]{position:relative}
[data-edit-insert-after]::after{content:"";position:absolute;left:0;right:0;bottom:-.55rem;height:2px;border-radius:2px;background:#7fb4ff}
</style>"#;

/// Which surface the document is edited through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Edit the rendered document directly.
    Rich,
    /// Edit the AsciiDoc source, with the rendering alongside it.
    Source,
}

/// The editor, over a document the caller owns.
///
/// `autosave` names a `localStorage` key to keep the document under; without
/// one nothing is written, which is what an embedding page usually wants.
#[component]
pub fn App(source: RwSignal<String>, autosave: Option<String>, rich: bool) -> impl IntoView {
    let mode = RwSignal::new(if rich { Mode::Rich } else { Mode::Source });
    // Trails `source` by the debounce interval; drives the expensive render.
    let settled = RwSignal::new(source.get_untracked());
    let warnings = RwSignal::new(Vec::<render::Warning>::new());
    let preview_ready = RwSignal::new(false);

    let textarea = NodeRef::<html::Textarea>::new();
    let overlay = NodeRef::<html::Pre>::new();
    let frame = NodeRef::<html::Iframe>::new();

    // The image panel, and anything it has to say back.
    let image_panel = RwSignal::new(false);
    let image_url = RwSignal::new(String::new());
    let image_alt = RwSignal::new(String::new());
    let notice = RwSignal::new(None::<String>);
    // Where a new block would go, taken when the panel opens: filling it in
    // moves focus out of the document, which would otherwise lose the answer.
    let insert_after = RwSignal::new(None::<wysiwyg::Block>);
    // Set when the panel is changing an image rather than adding one.
    let editing_image = RwSignal::new(None::<wysiwyg::Block>);
    // The link panel, and the span in the document it would write over.
    let link_panel = RwSignal::new(false);
    let link_url = RwSignal::new(String::new());
    let link_text = RwSignal::new(String::new());
    // The selection the link will be written over, kept alive while the panel
    // is filled in.
    let link_range = StoredValue::new(None::<web_sys::Range>);
    let link_selected = RwSignal::new(false);

    // The block being edited in place, if any. While it is set the preview is
    // left alone: re-rendering under a live caret would destroy it.
    let editing = RwSignal::new(None::<wysiwyg::Block>);

    // What the document looked like before each settled change, so that a
    // structural edit — or a burst of typing — can be taken back.
    let past = StoredValue::new(Vec::<Step>::new());
    let future = StoredValue::new(Vec::<Step>::new());
    let depth = RwSignal::new((0usize, 0usize));

    let render_timer = StoredValue::new(None::<TimeoutHandle>);
    let save_timer = StoredValue::new(None::<TimeoutHandle>);
    let last_line = StoredValue::new(0usize);

    // Everything downstream of typing is debounced: the render because it is
    // the expensive part, the autosave because localStorage writes are sync.
    Effect::new(move |_| {
        let current = source.get();

        if let Some(handle) = render_timer.get_value() {
            handle.clear();
        }
        let for_render = current.clone();
        render_timer.set_value(
            set_timeout_with_handle(
                move || {
                    // One step per pause in the typing, rather than per
                    // keystroke: the debounce already marks where a change
                    // settled, which is the same place undo should stop.
                    let previous = settled.get_untracked();
                    if previous != for_render {
                        past.update_value(|steps| {
                            steps.push(Step {
                                source: previous,
                                line: editing.get_untracked().map(|block| block.line),
                            });
                            if steps.len() > HISTORY_DEPTH {
                                steps.remove(0);
                            }
                        });
                        future.update_value(Vec::clear);
                        depth.set((past.with_value(Vec::len), 0));
                    }

                    settled.set(for_render);
                },
                RENDER_DEBOUNCE,
            )
            .ok(),
        );

        if let Some(handle) = save_timer.get_value() {
            handle.clear();
        }
        if let Some(key) = autosave.clone() {
            save_timer.set_value(
                set_timeout_with_handle(move || storage::save(&key, &current), AUTOSAVE_DEBOUNCE)
                    .ok(),
            );
        }
    });

    // A toolbar click must not take focus away from the block being edited,
    // hence `prevent_default` on mousedown at every button below.
    let apply_format = move |command: &'static str| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::format(&document, command);
        }
    };

    // Renders the current source into the preview. Given a line, the caret is
    // placed in the block that came from it — how the rich-text surface moves
    // the caret across a re-render.
    let rerender = move |focus_line: Option<usize>| {
        let src = source.get_untracked();
        let (html, warns) = render::render(&src);

        if let Some(content) = preview_content(frame) {
            content.set_inner_html(&html);

            if mode.get_untracked() == Mode::Rich {
                wysiwyg::mark_editable(&content, &src);
            }

            if let Some(line) = focus_line {
                focus_block(frame, &content, line);
            }

            // An operation may have removed the block the caret was in. With
            // nothing focused, the context bar has no business still
            // describing it.
            let focused = preview_document(frame)
                .and_then(|document| document.active_element())
                .is_some_and(|element| element.has_attribute("data-edit-line"));

            if !focused {
                editing.set(None);
            }
        }

        warnings.set(warns);
    };

    // Undo and redo move a step between the two stacks, leaving `settled` in
    // step with the source so the move is not recorded as a change of its own.
    let travel = move |back: bool| {
        let (from, to) = if back { (past, future) } else { (future, past) };
        let Some(step) = from.try_update_value(Vec::pop).flatten() else {
            return;
        };

        to.update_value(|steps| {
            steps.push(Step {
                source: source.get_untracked(),
                line: editing.get_untracked().map(|block| block.line),
            });
        });

        source.set(step.source.clone());
        settled.set(step.source);
        editing.set(None);
        depth.set((past.with_value(Vec::len), future.with_value(Vec::len)));

        rerender(step.line);
    };

    let apply_level = move |level: Option<usize>| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::set_level(&document, source, level, &rerender);
        }
    };

    // Clicking the label a block already carries takes it off again.
    let apply_admonition = move |label: Option<&'static str>| {
        let label = match (label, editing.get_untracked().map(|block| block.kind)) {
            (Some(label), Some(wysiwyg::Kind::Admonition(current))) if current == label => None,
            (label, _) => label,
        };

        if let Some(document) = preview_document(frame) {
            wysiwyg::set_admonition(&document, source, label, &rerender);
        }
    };

    // The selection is taken now: filling in the panel moves focus out of the
    // document, and with it any idea of what was selected.
    let open_link_panel = move || {
        let Some(document) = preview_document(frame) else {
            return;
        };
        let Some((range, selected)) = wysiwyg::selection_range(&document) else {
            return;
        };

        link_selected.set(!selected.is_empty());
        link_range.set_value(Some(range));
        link_text.set(selected);
        link_url.set(String::new());
        image_panel.set(false);
        notice.set(None);
        link_panel.set(true);
    };

    let add_link = move || {
        let (Some(document), Some(range)) = (preview_document(frame), link_range.get_value())
        else {
            return;
        };

        wysiwyg::insert_link(
            &document,
            source,
            &range,
            &link_url.get_untracked(),
            &link_text.get_untracked(),
            &rerender,
        );

        link_panel.set(false);
        link_range.set_value(None);
    };

    let apply_title = move |add: bool| {
        if let Some(document) = preview_document(frame) {
            if add {
                wysiwyg::add_title(&document, source, &rerender);
            } else {
                wysiwyg::remove_title(&document, source, &rerender);
            }
        }
    };

    let apply_row = move |add: bool| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::table_row(&document, source, add, &rerender);
        }
    };

    let insert_image = move |target: String, alt: String| {
        match editing_image.get_untracked() {
            Some(block) => wysiwyg::update_image(source, block, &target, &alt, &rerender),
            None => wysiwyg::insert_block_below(
                source,
                insert_after.get_untracked().map(|block| block.end),
                &source::image_macro(&target, &alt),
                &rerender,
            ),
        }

        editing_image.set(None);
        image_panel.set(false);
        image_url.set(String::new());
        image_alt.set(String::new());
        notice.set(None);
    };

    // Embedding copies the image into the document, so the source grows by
    // about a third more than the file itself. Past a point that is no longer
    // a document anyone wants to edit.
    let embed_image = move |file: web_sys::File| {
        if file.size() > MAX_EMBEDDED_BYTES {
            notice.set(Some(format!(
                "{} is {:.1} MB — too large to embed. Link to it instead.",
                file.name(),
                file.size() / 1_048_576.0
            )));
            return;
        }

        let alt = match image_alt.get_untracked().trim() {
            "" => file
                .name()
                .rsplit_once('.')
                .map_or(file.name(), |(stem, _)| stem.to_string()),
            given => given.to_string(),
        };

        storage::read_data_url(&file, move |data| insert_image(data, alt.clone()));
    };

    // Clicking the wrapper a block already has takes it off again.
    let apply_wrapper = move |style: &'static str| {
        let wanted = match editing.get_untracked().and_then(|block| block.inside) {
            Some(current) if current == style => None,
            _ => Some(style),
        };

        if let Some(document) = preview_document(frame) {
            wysiwyg::set_wrapper(&document, source, wanted, &rerender);
        }
    };

    let insert_below = move |text: &'static str| {
        wysiwyg::insert_block_below(
            source,
            editing.get_untracked().map(|block| block.end),
            text,
            &rerender,
        );
    };

    let apply_column = move |add: bool| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::table_column(&document, source, add, &rerender);
        }
    };

    // Reopens the panel over the image it came from, filled in with what is
    // already there.
    let edit_image = move || {
        let Some(document) = preview_document(frame) else {
            return;
        };
        let Some(block) = document
            .active_element()
            .filter(|block| block.has_attribute("data-edit-image"))
        else {
            return;
        };

        let Some((target, alt)) = wysiwyg::image_of(&block) else {
            return;
        };

        image_url.set(target);
        image_alt.set(alt);
        editing_image.set(editing.get_untracked());
        insert_after.set(None);
        notice.set(None);
        image_panel.set(true);
    };

    let drop_image = move || {
        if let Some(document) = preview_document(frame) {
            wysiwyg::remove_image(&document, source, &rerender);
        }
    };

    let drop_table = move || {
        if let Some(document) = preview_document(frame) {
            wysiwyg::remove_table(&document, source, &rerender);
        }
    };

    let apply_task = move |done: Option<bool>| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::set_task(&document, source, done, &rerender);
        }
    };

    let apply_indent = move |outdent: bool| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::reindent_focused(&document, source, outdent);
        }
    };

    // Clicking the kind a block already is turns it back into body text, the
    // way a list button behaves everywhere else.
    let apply_list = move |ordered: bool| {
        let marker = match editing.get_untracked().map(|block| block.kind) {
            Some(wysiwyg::Kind::List { ordered: current }) if current == ordered => None,
            _ => Some(if ordered { '.' } else { '*' }),
        };

        if let Some(document) = preview_document(frame) {
            wysiwyg::set_list(&document, source, marker, &rerender);
        }
    };

    // A panel points at something in the document — a block, or a selection —
    // so the rendering is held still for as long as one is open.
    Effect::new(move |_| {
        let open = image_panel.get() || link_panel.get();
        if let Some(document) = preview_document(frame) {
            wysiwyg::hold(&document, open);
        }
    });

    // Show in the document itself where a new block would land.
    Effect::new(move |_| {
        let target = image_panel.get().then(|| insert_after.get()).flatten();
        if let Some(document) = preview_document(frame) {
            wysiwyg::mark_insertion_point(&document, target.map(|block| block.line));
        }
    });

    // Render whenever the source settles, the mode changes, or the iframe
    // becomes ready.
    Effect::new(move |_| {
        settled.track();
        mode.track();
        if !preview_ready.get() || editing.get_untracked().is_some() {
            return;
        }

        rerender(None);

        // The new DOM has new scroll targets, so a sync from before the render
        // is stale. Re-follow the caret, but only while the user is actually
        // typing — otherwise this would yank the preview away from someone who
        // is just reading it.
        if editor_has_focus(textarea) {
            sync_cursor(textarea, frame, source, last_line, Force::Yes);
        }
    });

    view! {
        <div class="toolbar" role="toolbar" aria-label="Formatting">
            <span class="brand">"AsciiDoc"</span>

            <div class="modes">
                <button
                    class="button"
                    class:active=move || mode.get() == Mode::Rich
                    on:click=move |_| mode.set(Mode::Rich)
                >
                    "Rich text"
                </button>
                <button
                    class="button"
                    class:active=move || mode.get() == Mode::Source
                    on:click=move |_| mode.set(Mode::Source)
                >
                    "Source"
                </button>
            </div>

            <Show when=move || mode.get() == Mode::Rich>
                <span class="separator"></span>
                <div class="tools">
                    <button
                        class="button icon"
                        title="Bold (ctrl+B)"
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_format("bold")
                    >
                        <b>"B"</b>
                    </button>
                    <button
                        class="button icon"
                        title="Italic (ctrl+I)"
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_format("italic")
                    >
                        <i>"I"</i>
                    </button>
                    <button
                        class="button icon"
                        title="Insert a link"
                        disabled=move || {
                            !editing.get().is_some_and(|block| wysiwyg::takes_links(block.kind))
                        }
                        class:active=move || link_panel.get()
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| open_link_panel()
                    >
                        "🔗"
                    </button>
                    <button
                        class="button icon"
                        title="Monospace (ctrl+E)"
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_format("code")
                    >
                        <code>"<>"</code>
                    </button>
                </div>

                <span class="separator"></span>
                <div class="tools">
                    <button
                        class="button icon"
                        title="Bulleted list"
                        disabled=move || {
                            editing
                                .get()
                                .is_some_and(|block| {
                                    matches!(
                                        block.kind,
                                        wysiwyg::Kind::Title
                                            | wysiwyg::Kind::Admonition(_)
                                            | wysiwyg::Kind::Table { .. }
                                            | wysiwyg::Kind::Image
                                            | wysiwyg::Kind::Attribution(_)
                                            | wysiwyg::Kind::Verbatim
                                    )
                                })
                        }
                        class:active=move || {
                            editing.get().map(|block| block.kind)
                                == Some(wysiwyg::Kind::List { ordered: false })
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_list(false)
                    >
                        "•"
                    </button>
                    <button
                        class="button icon"
                        title="Admonition"
                        disabled=move || {
                            !matches!(
                                editing.get().map(|block| block.kind),
                                Some(wysiwyg::Kind::Body) | Some(wysiwyg::Kind::Admonition(_))
                            )
                        }
                        class:active=move || {
                            matches!(
                                editing.get().map(|block| block.kind),
                                Some(wysiwyg::Kind::Admonition(_))
                            )
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_admonition(Some("NOTE"))
                    >
                        {icon_for("NOTE")}
                    </button>
                    <button
                        class="button icon"
                        title="Numbered list"
                        disabled=move || {
                            editing
                                .get()
                                .is_some_and(|block| {
                                    matches!(
                                        block.kind,
                                        wysiwyg::Kind::Title
                                            | wysiwyg::Kind::Admonition(_)
                                            | wysiwyg::Kind::Table { .. }
                                            | wysiwyg::Kind::Image
                                            | wysiwyg::Kind::Attribution(_)
                                            | wysiwyg::Kind::Verbatim
                                    )
                                })
                        }
                        class:active=move || {
                            editing.get().map(|block| block.kind)
                                == Some(wysiwyg::Kind::List { ordered: true })
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_list(true)
                    >
                        "1."
                    </button>
                </div>

                <span class="separator"></span>
                <div class="tools">
                    <button
                        class="button"
                        title="Body text"
                        disabled=move || {
                            editing.get().is_some_and(|block| {
                                matches!(
                                    block.kind,
                                    wysiwyg::Kind::List { .. }
                                        | wysiwyg::Kind::Title
                                        | wysiwyg::Kind::Admonition(_)
                                        | wysiwyg::Kind::Table { .. }
                                        | wysiwyg::Kind::Image
                                            | wysiwyg::Kind::Attribution(_)
                                            | wysiwyg::Kind::Verbatim
                                )
                            })
                        }
                        class:active=move || {
                            editing.get().map(|block| block.kind) == Some(wysiwyg::Kind::Body)
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_level(None)
                    >
                        "Body"
                    </button>
                    <button
                        class="button"
                        title="Quote block"
                        disabled=move || editing.get().is_none()
                        class:active=move || {
                            editing.get().and_then(|block| block.inside) == Some("quote")
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_wrapper("quote")
                    >
                        "Quote"
                    </button>
                    <button
                        class="button"
                        title="Code block"
                        disabled=move || editing.get().is_none()
                        class:active=move || {
                            editing.get().and_then(|block| block.inside) == Some("source")
                        }
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| apply_wrapper("source")
                    >
                        "Code"
                    </button>
                    {HEADINGS
                        .iter()
                        .map(|(label, level)| {
                            view! {
                                <button
                                    class="button"
                                    title=format!("Heading ({} in AsciiDoc)", "=".repeat(*level))
                                    disabled=move || {
                                        editing.get().is_some_and(|block| {
                                            matches!(
                                                block.kind,
                                                wysiwyg::Kind::List { .. }
                                                    | wysiwyg::Kind::Title
                                                    | wysiwyg::Kind::Admonition(_)
                                                    | wysiwyg::Kind::Table { .. }
                                                    | wysiwyg::Kind::Image
                                            | wysiwyg::Kind::Attribution(_)
                                            | wysiwyg::Kind::Verbatim
                                            )
                                        })
                                    }
                                    class:active=move || {
                                        editing.get().map(|block| block.kind)
                                            == Some(wysiwyg::Kind::Heading(*level))
                                    }
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_level(Some(*level))
                                >
                                    {*label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </Show>

            <span class="separator"></span>
            <button
                class="button icon"
                title="Undo (ctrl+Z)"
                disabled=move || depth.get().0 == 0
                on:mousedown=|ev| ev.prevent_default()
                on:click=move |_| travel(true)
            >
                "↶"
            </button>
            <button
                class="button icon"
                title="Redo (ctrl+shift+Z)"
                disabled=move || depth.get().1 == 0
                on:mousedown=|ev| ev.prevent_default()
                on:click=move |_| travel(false)
            >
                "↷"
            </button>

            <span class="separator"></span>
            <button
                class="button icon"
                title="Insert a table"
                on:mousedown=|ev| ev.prevent_default()
                on:click=move |_| insert_below(source::NEW_TABLE)
            >
                "▦"
            </button>
            <button
                class="button icon"
                title="Insert a thematic break"
                on:mousedown=|ev| ev.prevent_default()
                on:click=move |_| insert_below(source::RULE)
            >
                "—"
            </button>
            <button
                class="button icon"
                title="Insert an image"
                on:mousedown=|ev| ev.prevent_default()
                class:active=move || image_panel.get()
                on:click=move |_| {
                    notice.set(None);
                    let opening = !image_panel.get_untracked();
                    link_panel.set(false);
                    editing_image.set(None);
                    image_url.set(String::new());
                    image_alt.set(String::new());
                    insert_after.set(opening.then(|| editing.get_untracked()).flatten());
                    image_panel.set(opening);
                }
            >
                "🖼"
            </button>

            <div class="spacer"></div>
            <label class="button">
                "Open"
                <input
                    type="file"
                    accept=".adoc,.asciidoc,.txt"
                    on:change=move |ev| {
                        let input: HtmlInputElement = event_target(&ev);
                        if let Some(file) = input.files().and_then(|f| f.get(0)) {
                            storage::read_file(&file, move |text| source.set(text));
                        }
                    }
                />
            </label>
            <button
                class="button"
                on:click=move |_| {
                    storage::download("document.adoc", "text/plain", &source.get_untracked());
                }
            >
                "Save .adoc"
            </button>
            <button
                class="button"
                on:click=move |_| {
                    let html = render::render_standalone(&source.get_untracked());
                    storage::download("document.html", "text/html", &html);
                }
            >
                "Export .html"
            </button>
        </div>

        <Show when=move || mode.get() == Mode::Rich>
            <div class="toolbar context" role="toolbar" aria-label="This block">
                {move || match editing.get() {
                    None => {
                        view! {
                            <span class="hint">"Click any block to edit it"</span>
                        }
                            .into_any()
                    }
                    Some(block) => {
                        let titled = block.title_line.is_some();
                        let is_title = block.kind == wysiwyg::Kind::Title;
                        let is_list = matches!(block.kind, wysiwyg::Kind::List { .. });
                        let is_admonition = matches!(block.kind, wysiwyg::Kind::Admonition(_));
                        let is_table = matches!(block.kind, wysiwyg::Kind::Table { .. });
                        let is_image = block.kind == wysiwyg::Kind::Image;
                        let fixed_columns = block.kind
                            == wysiwyg::Kind::Table {
                                fixed_columns: true,
                            };
                        // A heading is a title already; AsciiDoc gives it none.
                        let takes_title = !matches!(block.kind, wysiwyg::Kind::Heading(_));
                        view! {
                            <span class="chip">{describe(block.kind)}</span>

                            <Show when=move || takes_title>
                                <button
                                    class="button"
                                    title=if titled { "Remove the block title" } else { "Add a block title" }
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_title(!titled)
                                >
                                    {if titled { "Remove title" } else { "Add title" }}
                                </button>
                            </Show>

                            <Show when=move || is_admonition>
                                <span class="separator"></span>
                                {ADMONITION_ICONS
                                    .iter()
                                    .map(|(label, icon)| {
                                        view! {
                                            <button
                                                class="button icon"
                                                title=*label
                                                class:active=move || {
                                                    editing.get().map(|block| block.kind)
                                                        == Some(wysiwyg::Kind::Admonition(label))
                                                }
                                                on:mousedown=|ev| ev.prevent_default()
                                                on:click=move |_| apply_admonition(Some(label))
                                            >
                                                {*icon}
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                                <button
                                    class="button"
                                    title="Turn this back into an ordinary paragraph"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_admonition(None)
                                >
                                    "Remove"
                                </button>
                            </Show>

                            <Show when=move || is_image>
                                <span class="separator"></span>
                                <button
                                    class="button"
                                    title="Point this image somewhere else, or describe it differently"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| edit_image()
                                >
                                    "Edit image"
                                </button>
                                <button
                                    class="button danger"
                                    title="Delete this image"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| drop_image()
                                >
                                    "Remove"
                                </button>
                            </Show>

                            <Show when=move || is_table>
                                <span class="separator"></span>
                                <button
                                    class="button"
                                    title="Add a row below this one"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_row(true)
                                >
                                    "+ Row"
                                </button>
                                <button
                                    class="button"
                                    title="Delete this row"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_row(false)
                                >
                                    "− Row"
                                </button>
                                <button
                                    class="button"
                                    title=if fixed_columns {
                                        "This table's cols attribute pins its columns"
                                    } else {
                                        "Add a column beside this one"
                                    }
                                    disabled=move || fixed_columns
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_column(true)
                                >
                                    "+ Col"
                                </button>
                                <button
                                    class="button"
                                    title=if fixed_columns {
                                        "This table's cols attribute pins its columns"
                                    } else {
                                        "Delete this column"
                                    }
                                    disabled=move || fixed_columns
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_column(false)
                                >
                                    "− Col"
                                </button>

                                <span class="separator"></span>
                                <button
                                    class="button danger"
                                    title="Delete the whole table"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| drop_table()
                                >
                                    "Delete table"
                                </button>
                            </Show>

                            <Show when=move || is_list>
                                <span class="separator"></span>
                                <button
                                    class="button"
                                    title="Make this item a task, or an ordinary one again"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_task(None)
                                >
                                    "Task"
                                </button>
                                <button
                                    class="button"
                                    title="Mark this task done"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_task(Some(true))
                                >
                                    "✓"
                                </button>
                                <button
                                    class="button"
                                    title="Mark this task not done"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_task(Some(false))
                                >
                                    "☐"
                                </button>

                                <span class="separator"></span>
                                <button
                                    class="button icon"
                                    title="Outdent: lift this item out a level (shift+tab)"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_indent(true)
                                >
                                    "⇤"
                                </button>
                                <button
                                    class="button icon"
                                    title="Indent: nest this item under the one above (tab)"
                                    on:mousedown=|ev| ev.prevent_default()
                                    on:click=move |_| apply_indent(false)
                                >
                                    "⇥"
                                </button>
                            </Show>

                            <Show when=move || is_title>
                                <span class="hint">"Titles are a single line"</span>
                            </Show>
                        }
                            .into_any()
                    }
                }}
            </div>
        </Show>


        <Show when=move || link_panel.get()>
            <div class="panel">
                <span class="chip">
                    {move || match link_selected.get() {
                        true => "Over the selection",
                        false => "At the caret",
                    }}
                </span>

                <label>
                    "Link to"
                    <input
                        type="text"
                        placeholder="https://example.com or guide.html"
                        prop:value=link_url
                        on:input=move |ev| link_url.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Text"
                    <input
                        type="text"
                        placeholder="What the link says"
                        prop:value=link_text
                        on:input=move |ev| link_text.set(event_target_value(&ev))
                    />
                </label>

                <button
                    class="button"
                    disabled=move || link_url.get().trim().is_empty()
                    on:click=move |_| add_link()
                >
                    "Insert link"
                </button>

                <div class="spacer"></div>
                <button class="button" on:click=move |_| link_panel.set(false)>
                    "Close"
                </button>
            </div>
        </Show>

        <Show when=move || image_panel.get()>
            <div class="panel">
                <span class="chip">
                    {move || match (editing_image.get(), insert_after.get()) {
                        (Some(_), _) => "Changing this image".to_string(),
                        (None, Some(block)) => {
                            format!("Below the {}", describe(block.kind).to_lowercase())
                        }
                        (None, None) => "At the end of the document".to_string(),
                    }}
                </span>

                <label>
                    "Image URL or path"
                    <input
                        type="text"
                        placeholder="images/diagram.png"
                        prop:value=image_url
                        on:input=move |ev| image_url.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    "Description"
                    <input
                        type="text"
                        placeholder="What the image shows"
                        prop:value=image_alt
                        on:input=move |ev| image_alt.set(event_target_value(&ev))
                    />
                </label>

                <button
                    class="button"
                    disabled=move || image_url.get().trim().is_empty()
                    on:click=move |_| insert_image(image_url.get_untracked(), image_alt.get_untracked())
                >
                    {move || if editing_image.get().is_some() { "Update" } else { "Link to it" }}
                </button>

                <span class="separator"></span>
                <label class="button" title="Copy the image into the document itself">
                    "Embed a file…"
                    <input
                        type="file"
                        accept="image/*"
                        on:change=move |ev| {
                            let input: HtmlInputElement = event_target(&ev);
                            if let Some(file) = input.files().and_then(|files| files.get(0)) {
                                embed_image(file);
                            }
                        }
                    />
                </label>

                <div class="spacer"></div>
                <button
                    class="button"
                    on:click=move |_| {
                        notice.set(None);
                        editing_image.set(None);
                        image_panel.set(false);
                    }
                >
                    "Close"
                </button>
            </div>
        </Show>

        <div class="panes" class:rich=move || mode.get() == Mode::Rich>
            <div class="editor">
                <pre class="overlay" node_ref=overlay inner_html=move || highlight::highlight(&source.get())></pre>
                <textarea
                    class="source"
                    node_ref=textarea
                    spellcheck="false"
                    prop:value=source
                    on:input=move |ev| source.set(event_target_value(&ev))
                    on:scroll=move |_| {
                        // Keep the highlight layer glued to the text above it.
                        if let (Some(ta), Some(pre)) = (textarea.get(), overlay.get()) {
                            pre.set_scroll_top(ta.scroll_top());
                            pre.set_scroll_left(ta.scroll_left());
                        }
                    }
                    on:keyup=move |_| sync_cursor(textarea, frame, source, last_line, Force::No)
                    on:click=move |_| sync_cursor(textarea, frame, source, last_line, Force::No)
                />
            </div>

            <div class="divider"></div>

            <div class="preview">
                <iframe
                    node_ref=frame
                    srcdoc=preview_shell()
                    on:load=move |_| {
                        attach_click_to_locate(frame, textarea, source, mode);

                        if let (Some(document), Some(content)) =
                            (preview_document(frame), preview_content(frame))
                        {
                            wysiwyg::attach(
                                &document,
                                content,
                                source,
                                editing,
                                wysiwyg::Hooks {
                                    rerender,
                                    travel,
                                    on_file: embed_image,
                                    editable: move || mode.get_untracked() == Mode::Rich,
                                },
                            );
                        }

                        preview_ready.set(true);
                    }
                />
            </div>
        </div>

        <div class="status" role="status">
            {move || {
                if let Some(said) = notice.get() {
                    return view! { <span class="warn">{said}</span> }.into_any();
                }

                let warnings = warnings.get();
                if warnings.is_empty() {
                    view! { <span class="ok">"No warnings"</span> }.into_any()
                } else {
                    let summary = warnings
                        .iter()
                        .take(3)
                        .map(|w| format!("line {}: {}", w.line, w.message))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    view! { <span class="warn">{format!("{} warning(s) — {summary}", warnings.len())}</span> }
                        .into_any()
                }
            }}
        </div>
    }
}

/// Names the focused block for the context bar.
fn describe(kind: wysiwyg::Kind) -> String {
    match kind {
        wysiwyg::Kind::Body => "Paragraph".to_string(),
        wysiwyg::Kind::Heading(level) => format!("Heading {}", level.saturating_sub(1).max(1)),
        wysiwyg::Kind::List { ordered: false } => "Bulleted list".to_string(),
        wysiwyg::Kind::List { ordered: true } => "Numbered list".to_string(),
        wysiwyg::Kind::Title => "Block title".to_string(),
        wysiwyg::Kind::Admonition(label) => label.to_string(),
        wysiwyg::Kind::Table { .. } => "Table cell".to_string(),
        wysiwyg::Kind::Image => "Image".to_string(),
        wysiwyg::Kind::Verbatim => "Code block".to_string(),
        wysiwyg::Kind::Attribution(style) => format!("{style} attribution"),
    }
}

/// Puts the caret in the block rendered from `line`.
fn focus_block(frame: NodeRef<html::Iframe>, content: &Element, line: usize) {
    let (Some(document), Ok(Some(block))) = (
        preview_document(frame),
        content.query_selector(&format!("[data-edit-line=\"{line}\"]")),
    ) else {
        return;
    };

    wysiwyg::focus(&document, &block);
}

/// The document inside the preview iframe.
fn preview_document(frame: NodeRef<html::Iframe>) -> Option<web_sys::Document> {
    frame.get_untracked()?.content_document()
}

/// The `#content` div inside the preview iframe.
fn preview_content(frame: NodeRef<html::Iframe>) -> Option<Element> {
    let frame: HtmlIFrameElement = frame.get_untracked()?;
    frame.content_document()?.get_element_by_id("content")
}

/// Whether to sync even though the caret has not moved to a different line.
#[derive(PartialEq)]
enum Force {
    Yes,
    No,
}

/// Scrolls the preview to the block under the caret.
///
/// Unforced, this is a no-op while the caret stays on one line — otherwise
/// every keystroke would re-scroll the preview.
fn sync_cursor(
    textarea: NodeRef<html::Textarea>,
    frame: NodeRef<html::Iframe>,
    source: RwSignal<String>,
    last_line: StoredValue<usize>,
    force: Force,
) {
    let Some(ta) = textarea.get_untracked() else {
        return;
    };
    let Some(offset) = ta.selection_start().ok().flatten() else {
        return;
    };

    let line = source.with_untracked(|src| sync::line_of_offset(src, offset as usize));
    if line == last_line.get_value() && force == Force::No {
        return;
    }
    last_line.set_value(line);

    if let Some(document) = frame.get_untracked().and_then(|f| f.content_document()) {
        sync::scroll_preview_to_line(&document, line);
    }
}

fn editor_has_focus(textarea: NodeRef<html::Textarea>) -> bool {
    let Some(ta) = textarea.get_untracked() else {
        return false;
    };

    document()
        .active_element()
        .is_some_and(|active| active.is_same_node(Some(&ta)))
}

/// Clicking a block in the preview puts the caret on the line that produced it.
fn attach_click_to_locate(
    frame: NodeRef<html::Iframe>,
    textarea: NodeRef<html::Textarea>,
    source: RwSignal<String>,
    mode: RwSignal<Mode>,
) {
    let Some(document) = frame.get_untracked().and_then(|f| f.content_document()) else {
        return;
    };

    let on_click = Closure::<dyn FnMut(Event)>::new(move |ev: Event| {
        // In rich-text mode a click is placing the caret for editing, not
        // asking to jump to the source.
        if mode.get_untracked() == Mode::Rich {
            return;
        }

        // `dyn_into` would test `instanceof` against *this* realm's `Element`,
        // and nodes from inside the iframe belong to the iframe's realm — the
        // cast always fails. Check the node type instead and cast unchecked.
        let Some(target) = ev.target().map(|t| t.unchecked_into::<Node>()) else {
            return;
        };
        if target.node_type() != Node::ELEMENT_NODE {
            return;
        }
        let target: Element = target.unchecked_into();
        let Some(line) = sync::source_line_of_click(&target) else {
            return;
        };
        let Some(ta) = textarea.get_untracked() else {
            return;
        };

        let offset = source.with_untracked(|src| sync::offset_of_line(src, line)) as u32;
        let _ = ta.focus();
        let _ = ta.set_selection_start(Some(offset));
        let _ = ta.set_selection_end(Some(offset));
        scroll_caret_into_view(&ta, source, line);
    });

    let _ = document.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref());
    // Lives as long as the iframe, which lives as long as the app.
    on_click.forget();
}

/// Approximates the scroll offset of a line.
///
/// Exact placement would require measuring wrapped line boxes; proportional
/// placement is close enough to put the caret on screen, and the caret itself
/// is authoritative once the user types.
fn scroll_caret_into_view(ta: &HtmlTextAreaElement, source: RwSignal<String>, line: usize) {
    let total = source.with_untracked(|src| src.lines().count()).max(1);
    let height = ta.scroll_height() as f64;
    let target = height * (line.saturating_sub(1) as f64 / total as f64)
        - f64::from(ta.client_height()) / 2.0;

    ta.set_scroll_top(target.max(0.0) as i32);
}
