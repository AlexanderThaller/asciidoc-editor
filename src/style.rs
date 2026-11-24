//! Styling put on a document as a constructed stylesheet.
//!
//! A `<style>` element is inline style as far as a Content-Security-Policy is
//! concerned: a page that allows only `style-src 'self'` refuses it, and so
//! does the preview iframe, which inherits the page's policy. A stylesheet
//! built with `new CSSStyleSheet()` and adopted by the document is not
//! subject to `style-src` at all, so the editor works under such a policy
//! without the page having to hash or allow anything.
//!
//! Where constructed stylesheets are missing (Safari before 16.4) the
//! `<style>` element is still used.

use js_sys::{Array, Function, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::Document;

/// Puts `css` on `document`, once: a second call with the same `key` does
/// nothing.
pub fn adopt(document: &Document, css: &str, key: &str) -> Result<(), JsValue> {
    let marker = JsValue::from_str(key);
    if Reflect::has(document, &marker)? {
        return Ok(());
    }

    match constructed(document, css) {
        Some(sheet) => {
            let adopted = JsValue::from_str("adoptedStyleSheets");
            // Copied, then assigned: the list is not one to push onto in
            // every browser. Earlier sheets — the page's own — stay first.
            let sheets = Array::from(&Reflect::get(document, &adopted)?);
            sheets.push(&sheet);
            Reflect::set(document, &adopted, &sheets)?;
        }
        None => {
            let style = document.create_element("style")?;
            style.set_id(key);
            style.set_text_content(Some(css));
            document
                .head()
                .ok_or_else(|| JsValue::from_str("the document has no head"))?
                .append_child(&style)?;
        }
    }

    Reflect::set(document, &marker, &JsValue::TRUE)?;
    Ok(())
}

/// A stylesheet holding `css`, made in `document`'s own window.
///
/// The constructor has to be that window's: a document adopts only sheets
/// made for it, and the preview iframe is a window of its own. Looked up by
/// name and checked with `typeof` rather than `instanceof`, which fails for
/// anything belonging to another window.
fn constructed(document: &Document, css: &str) -> Option<JsValue> {
    if !Reflect::has(document, &JsValue::from_str("adoptedStyleSheets")).ok()? {
        return None;
    }

    let window = document.default_view()?;
    let constructor = Reflect::get(&window, &JsValue::from_str("CSSStyleSheet"))
        .ok()
        .filter(JsValue::is_function)?
        .unchecked_into::<Function>();

    let sheet = Reflect::construct(&constructor, &Array::new()).ok()?;
    let replace = Reflect::get(&sheet, &JsValue::from_str("replaceSync"))
        .ok()
        .filter(JsValue::is_function)?
        .unchecked_into::<Function>();
    replace.call1(&sheet, &JsValue::from_str(css)).ok()?;

    Some(sheet)
}
