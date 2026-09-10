//! Split-pane editor: highlighted source on the left, live preview on the right.

use std::time::Duration;

use leptos::{html, prelude::*};
use wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{Element, Event, HtmlIFrameElement, HtmlInputElement, HtmlTextAreaElement, Node};

use crate::{highlight, render, storage, sync};

const SAMPLE: &str = include_str!("../assets/sample.adoc");

/// Re-rendering on every keystroke is wasteful; this is short enough to feel live.
const RENDER_DEBOUNCE: Duration = Duration::from_millis(150);
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The preview iframe is built once and then mutated in place. Re-assigning
/// `srcdoc` would reload it and throw away the scroll position on every render.
const PREVIEW_SHELL: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/asciidoctor-default.css">
<style>body{margin:0;padding:1.25rem 1.5rem}</style></head>
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
        save_timer
            .set_value(set_timeout_with_handle(move || storage::save(&current), AUTOSAVE_DEBOUNCE).ok());
    });

    // Render whenever the source settles and the iframe is ready to receive it.
    Effect::new(move |_| {
        let src = settled.get();
        if !preview_ready.get() {
            return;
        }

        let (html, warns) = render::render(&src);
        if let Some(content) = preview_content(frame) {
            content.set_inner_html(&html);
            // The new DOM has new scroll targets, so a sync from before the
            // render is stale. Re-follow the caret, but only while the user is
            // actually typing — otherwise this would yank the preview away
            // from someone who is just reading it.
            if editor_has_focus(textarea) {
                sync_cursor(textarea, frame, source, last_line, Force::Yes);
            }
        }
        warnings.set(warns);
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
                        attach_click_to_locate(frame, textarea, source);
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
    let Some(ta) = textarea.get_untracked() else { return };
    let Some(offset) = ta.selection_start().ok().flatten() else { return };

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
    let Some(ta) = textarea.get_untracked() else { return false };

    document()
        .active_element()
        .is_some_and(|active| active.is_same_node(Some(&ta)))
}

/// Clicking a block in the preview puts the caret on the line that produced it.
fn attach_click_to_locate(
    frame: NodeRef<html::Iframe>,
    textarea: NodeRef<html::Textarea>,
    source: RwSignal<String>,
) {
    let Some(document) = frame.get_untracked().and_then(|f| f.content_document()) else {
        return;
    };

    let on_click = Closure::<dyn FnMut(Event)>::new(move |ev: Event| {
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
        let Some(line) = sync::source_line_of_click(&target) else { return };
        let Some(ta) = textarea.get_untracked() else { return };

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
