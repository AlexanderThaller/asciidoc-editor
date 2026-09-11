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
//! the wasm, so there is nothing for the page to serve alongside.

mod app;
mod highlight;
mod inline;
mod list;
mod render;
mod source;
mod storage;
mod sync;
mod table;
mod wysiwyg;

use std::{any::Any, cell::RefCell, rc::Rc};

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

/// A mounted editor.
///
/// Hold on to it: letting it go takes the editor off the page, which is what
/// [`Editor::destroy`] does deliberately.
#[wasm_bindgen]
pub struct Editor {
    source: RwSignal<String>,
    /// Kept so that unmounting can happen on demand rather than never.
    mounted: Option<Box<dyn Any>>,
    /// Kept so that destroying gives the element back as it was found.
    host: HtmlElement,
}

#[wasm_bindgen]
impl Editor {
    /// The document, as AsciiDoc.
    pub fn value(&self) -> String {
        self.source.get_untracked()
    }

    /// Replaces the document.
    #[wasm_bindgen(js_name = setValue)]
    pub fn set_value(&self, source: String) {
        self.source.set(source);
    }

    /// Calls `callback` with the document whenever it changes.
    #[wasm_bindgen(js_name = onChange)]
    pub fn on_change(&self, callback: js_sys::Function) {
        let source = self.source;

        Effect::new(move |_| {
            let document = source.get();
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
/// `localStorage` key to keep the document under, and `richText` to start in
/// rich text rather than source (the default).
#[wasm_bindgen]
pub fn mount(target: &JsValue, options: Option<js_sys::Object>) -> Result<Editor, JsValue> {
    console_error_panic_hook::set_once();

    let host = resolve(target)?;
    let options = options.map(JsValue::from).unwrap_or(JsValue::UNDEFINED);

    let autosave = string_option(&options, "autosave");
    let value = string_option(&options, "value")
        .or_else(|| autosave.as_deref().and_then(storage::load))
        .unwrap_or_else(|| SAMPLE.to_string());
    let rich = bool_option(&options, "richText").unwrap_or(true);

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

        view! { <app::App source=source autosave=autosave.clone() rich=rich /> }
    });

    let source = carried
        .borrow_mut()
        .take()
        .ok_or_else(|| JsValue::from_str("the editor did not start"))?;

    Ok(Editor {
        source,
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

    if document.get_element_by_id(STYLE_ID).is_some() {
        return Ok(());
    }

    let style = document.create_element("style")?;
    style.set_id(STYLE_ID);
    style.set_text_content(Some(STYLE));

    document
        .head()
        .ok_or_else(|| JsValue::from_str("the page has no head"))?
        .append_child(&style)?;

    Ok(())
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
