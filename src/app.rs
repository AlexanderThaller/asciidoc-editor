//! Split-pane editor: highlighted source on the left, live preview on the right.

use std::time::Duration;

use leptos::{html, prelude::*};
use wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{Element, Event, HtmlIFrameElement, HtmlInputElement, HtmlTextAreaElement, Node};

use crate::{highlight, render, storage, sync, wysiwyg};

const SAMPLE: &str = include_str!("../assets/sample.adoc");

/// Toolbar heading buttons. `=` is the document title in AsciiDoc, so the
/// largest heading a body author writes is `==`.
const HEADINGS: [(&str, usize); 3] = [("H1", 2), ("H2", 3), ("H3", 4)];

/// Re-rendering on every keystroke is wasteful; this is short enough to feel live.
const RENDER_DEBOUNCE: Duration = Duration::from_millis(150);
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The preview iframe is built once and then mutated in place. Re-assigning
/// `srcdoc` would reload it and throw away the scroll position on every render.
const PREVIEW_SHELL: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/asciidoctor-default.css">
<style>
body{margin:0;padding:1.25rem 1.5rem}
[data-edit-line]{border-radius:3px}
[data-edit-line]:hover{background:rgba(127,180,255,.08)}
[data-edit-line]:focus{outline:2px solid rgba(127,180,255,.5);outline-offset:4px}
</style></head>
<body class="article"><div id="content"></div></body></html>"#;

/// Which surface the document is edited through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Edit the rendered document directly.
    Rich,
    /// Edit the AsciiDoc source, with the rendering alongside it.
    Source,
}

#[component]
pub fn App() -> impl IntoView {
    let mode = RwSignal::new(Mode::Rich);
    let source = RwSignal::new(storage::load().unwrap_or_else(|| SAMPLE.to_string()));
    // Trails `source` by the debounce interval; drives the expensive render.
    let settled = RwSignal::new(source.get_untracked());
    let warnings = RwSignal::new(Vec::<render::Warning>::new());
    let preview_ready = RwSignal::new(false);

    let textarea = NodeRef::<html::Textarea>::new();
    let overlay = NodeRef::<html::Pre>::new();
    let frame = NodeRef::<html::Iframe>::new();

    // The block being edited in place, if any. While it is set the preview is
    // left alone: re-rendering under a live caret would destroy it.
    let editing = RwSignal::new(None::<wysiwyg::Block>);

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
            set_timeout_with_handle(move || settled.set(for_render), RENDER_DEBOUNCE).ok(),
        );

        if let Some(handle) = save_timer.get_value() {
            handle.clear();
        }
        save_timer.set_value(
            set_timeout_with_handle(move || storage::save(&current), AUTOSAVE_DEBOUNCE).ok(),
        );
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
        }

        warnings.set(warns);
    };

    let apply_level = move |level: Option<usize>| {
        if let Some(document) = preview_document(frame) {
            wysiwyg::set_level(&document, source, level, &rerender);
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
        <header class="toolbar">
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
                        title="Numbered list"
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
                                matches!(block.kind, wysiwyg::Kind::List { .. })
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
                    {HEADINGS
                        .iter()
                        .map(|(label, level)| {
                            view! {
                                <button
                                    class="button"
                                    title=format!("Heading ({} in AsciiDoc)", "=".repeat(*level))
                                    disabled=move || {
                                        editing.get().is_some_and(|block| {
                                            matches!(block.kind, wysiwyg::Kind::List { .. })
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
        </header>

        <main class="panes" class:rich=move || mode.get() == Mode::Rich>
            <section class="editor">
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
            </section>

            <div class="divider"></div>

            <section class="preview">
                <iframe
                    node_ref=frame
                    srcdoc=PREVIEW_SHELL
                    on:load=move |_| {
                        attach_click_to_locate(frame, textarea, source, mode);

                        if let (Some(document), Some(content)) =
                            (preview_document(frame), preview_content(frame))
                        {
                            wysiwyg::attach(&document, content, source, editing, rerender);
                        }

                        preview_ready.set(true);
                    }
                />
            </section>
        </main>

        <footer class="status">
            {move || {
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
        </footer>
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
