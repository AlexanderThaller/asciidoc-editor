//! An AsciiDoc editor that a web page can put on itself.
//!
//! ```js
//! import init, { mount } from "./asciidoc_editor.js";
//!
//! await init();
//! const editor = mount("#editor", { value: "= Title\n\nWords." });
//!
//! editor.value();            // the document as AsciiDoc
//! editor.setValue("= New");
//! editor.onChange(doc => save(doc));
//! editor.destroy();
//! ```
//!
//! Everything the editor needs travels with it: the stylesheets are built into
//! the wasm, so there is nothing for the page to serve alongside. They go onto
//! the page as constructed stylesheets rather than `<style>` elements, which a
//! strict Content-Security-Policy has nothing to say about.

mod app;
mod highlight;
mod inline;
mod line_ends;
mod list;
mod render;
mod source;
mod storage;
mod style;
mod sync;
mod table;
mod wysiwyg;

use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
};

use leptos::prelude::*;
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::HtmlElement;

/// The document the editor opens with when the page names none.
pub const SAMPLE: &str = include_str!("../assets/sample.adoc");

/// The editor's own styling, put on the page once however many editors it
/// holds.
const STYLE: &str = include_str!("../assets/app.css");
const STYLE_ID: &str = "asciidoc-editor-style";

/// What the styling hangs off, put on the element the editor is given.
const CLASS: &str = "asciidoc-editor";

/// The types a TypeScript caller sees. wasm-bindgen would otherwise describe
/// the options as `object` and the target as `any`, which says nothing about
/// what either may hold.
#[wasm_bindgen(typescript_custom_section)]
const TYPES: &'static str = r#"
/** What the editor may be told when it is mounted. */
export interface MountOptions {
    /** The document to open with. Defaults to a sample document. */
    value?: string;
    /** A `localStorage` key to keep the document under. */
    autosave?: string;
    /** Start in rich text rather than source. Defaults to `true`. */
    richText?: boolean;
    /**
     * Takes a file that was pasted, dropped or picked, and answers with the
     * AsciiDoc to put in the document for it, such as `image::a.png[]`.
     * Without it, images are embedded in the document as `data:` URIs.
     */
    onFile?: FileCallback;
    /**
     * What relative image and link targets in the rendering resolve against,
     * such as `/files/`. Defaults to the page's own address.
     */
    baseUrl?: string;
}

/** The element to mount into, or a selector naming it. */
export type MountTarget = Element | string;

/** Called with the document, as AsciiDoc, whenever it changes. */
export type ChangeCallback = (document: string) => void;

/**
 * Given a file the user pasted, dropped or picked, says what goes in the
 * document for it: AsciiDoc, put in as a block of its own. Nothing, or an
 * empty string, puts nothing in; a rejection is shown to the user.
 */
export type FileCallback = (
    file: File,
) => string | null | undefined | Promise<string | null | undefined>;
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "MountOptions")]
    pub type MountOptions;

    #[wasm_bindgen(typescript_type = "MountTarget")]
    pub type MountTarget;

    #[wasm_bindgen(typescript_type = "ChangeCallback")]
    pub type ChangeCallback;
}

/// A mounted editor.
///
/// Hold on to it: letting it go takes the editor off the page, which is what
/// [`Editor::destroy`] does deliberately.
#[wasm_bindgen]
pub struct Editor {
    source: RwSignal<String>,
    /// Whether the document came with CRLF line ends. The editor works on LF
    /// and hands them back.
    crlf: Rc<Cell<bool>>,
    /// Kept so that unmounting can happen on demand rather than never.
    mounted: Option<Box<dyn Any>>,
    /// Kept so that destroying gives the element back as it was found.
    host: HtmlElement,
}

#[wasm_bindgen]
impl Editor {
    /// The document, as AsciiDoc.
    pub fn value(&self) -> String {
        line_ends::restore(self.source.get_untracked(), self.crlf.get())
    }

    /// Replaces the document.
    #[wasm_bindgen(js_name = setValue)]
    pub fn set_value(&self, source: String) {
        self.crlf.set(line_ends::is_crlf(&source));
        self.source.set(line_ends::to_lf(source));
    }

    /// Calls `callback` with the document whenever it changes.
    #[wasm_bindgen(js_name = onChange)]
    pub fn on_change(&self, callback: ChangeCallback) {
        let source = self.source;
        let crlf = Rc::clone(&self.crlf);
        let callback: js_sys::Function = callback.unchecked_into();

        Effect::new(move |_| {
            let document = line_ends::restore(source.get(), crlf.get());
            let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&document));
        });
    }

    /// Takes the editor off the page, leaving the element as it was found.
    pub fn destroy(&mut self) {
        self.mounted = None;
        let _ = self.host.class_list().remove_1(CLASS);
    }
}

/// Puts an editor inside the element named by `target`, a CSS selector or the
/// element itself.
///
/// Options, all of them optional: `value` to open with, `autosave` naming a
/// `localStorage` key to keep the document under, `richText` to start in
/// rich text rather than source (the default), `onFile` to be handed pasted,
/// dropped and picked files rather than embedding them, and `baseUrl` for
/// relative targets in the rendering to resolve against.
#[wasm_bindgen]
pub fn mount(target: &MountTarget, options: Option<MountOptions>) -> Result<Editor, JsValue> {
    console_error_panic_hook::set_once();

    let host = resolve(target.as_ref())?;
    let options = options.map(JsValue::from).unwrap_or(JsValue::UNDEFINED);

    let autosave = string_option(&options, "autosave");
    let value = string_option(&options, "value")
        .or_else(|| autosave.as_deref().and_then(storage::load))
        .unwrap_or_else(|| SAMPLE.to_string());
    let rich = bool_option(&options, "richText").unwrap_or(true);
    let on_file = function_option(&options, "onFile");
    let base = string_option(&options, "baseUrl")
        .map(|base| absolute(&host, &base))
        .transpose()?;

    // Every edit splices lines back together with LF. A document written with
    // CRLF would come back with every line changed, so it is edited as LF and
    // handed back the way it came.
    let crlf = Rc::new(Cell::new(line_ends::is_crlf(&value)));
    let value = line_ends::to_lf(value);

    add_style(&host)?;
    // Added rather than assigned: the page may well have dressed the element
    // it is handing over, and those classes are not the editor's to discard.
    host.class_list().add_1(CLASS)?;

    // The signal is made inside the mount so that it belongs to the same
    // reactive owner as everything reading it; the view runs before this
    // returns, so it is there to take afterwards.
    let carried: Rc<RefCell<Option<RwSignal<String>>>> = Rc::new(RefCell::new(None));
    let taken = Rc::clone(&carried);

    let mounted = leptos::mount::mount_to(host.clone(), move || {
        let source = RwSignal::new(value.clone());
        *taken.borrow_mut() = Some(source);

        view! {
            <app::App
                source=source
                autosave=autosave.clone()
                rich=rich
                on_file=on_file.clone()
                base=base.clone()
            />
        }
    });

    let source = carried
        .borrow_mut()
        .take()
        .ok_or_else(|| JsValue::from_str("the editor did not start"))?;

    Ok(Editor {
        source,
        crlf,
        mounted: Some(Box::new(mounted)),
        host,
    })
}

/// Finds the element to mount into, whether named or handed over.
fn resolve(target: &JsValue) -> Result<HtmlElement, JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("no document to mount into"))?;

    if let Some(selector) = target.as_string() {
        return document
            .query_selector(&selector)
            .ok()
            .flatten()
            .and_then(|element| element.dyn_into::<HtmlElement>().ok())
            .ok_or_else(|| JsValue::from_str(&format!("nothing matches {selector}")));
    }

    target
        .clone()
        .dyn_into::<HtmlElement>()
        .map_err(|_| JsValue::from_str("mount target must be an element or a selector"))
}

/// Puts the editor's styling on the page, once.
fn add_style(host: &HtmlElement) -> Result<(), JsValue> {
    let document = host
        .owner_document()
        .ok_or_else(|| JsValue::from_str("the mount target is not on a page"))?;

    style::adopt(&document, STYLE, STYLE_ID)
}

/// `base` made absolute against the page. The preview's own address is
/// `about:srcdoc`, which a relative base could not be resolved against.
fn absolute(host: &HtmlElement, base: &str) -> Result<String, JsValue> {
    let page = host
        .owner_document()
        .and_then(|document| document.base_uri().ok().flatten())
        .ok_or_else(|| JsValue::from_str("the mount target is not on a page"))?;

    web_sys::Url::new_with_base(base, &page)
        .map(|url| url.href())
        .map_err(|_| JsValue::from_str(&format!("baseUrl {base} is not a URL")))
}

fn string_option(options: &JsValue, name: &str) -> Option<String> {
    js_sys::Reflect::get(options, &JsValue::from_str(name))
        .ok()?
        .as_string()
        .filter(|value| !value.is_empty())
}

fn bool_option(options: &JsValue, name: &str) -> Option<bool> {
    js_sys::Reflect::get(options, &JsValue::from_str(name))
        .ok()?
        .as_bool()
}

fn function_option(options: &JsValue, name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(options, &JsValue::from_str(name))
        .ok()
        .filter(JsValue::is_function)
        .map(JsCast::unchecked_into)
}
