//! The editor on a page of its own, for working on a document directly.
//!
//! The same editor a page can embed — see `lib.rs` — mounted on the body and
//! told to keep what is written in local storage.

use asciidoc_editor::mount;
use wasm_bindgen::JsValue;

fn main() {
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &options,
        &JsValue::from_str("autosave"),
        &JsValue::from_str("asciidoc-editor.source"),
    );

    match mount(&JsValue::from_str("body"), Some(options)) {
        // The page is the editor and outlives it; dropping the handle would
        // take the editor straight back off again.
        Ok(editor) => std::mem::forget(editor),
        Err(error) => web_sys::console::error_1(&error),
    }
}
