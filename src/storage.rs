//! Autosave to `localStorage`, plus file import and export.

use wasm_bindgen::{JsCast, prelude::Closure};
use web_sys::{Blob, BlobPropertyBag, File, FileReader, HtmlAnchorElement, Url, js_sys::Array};

/// Reads the autosaved document, if there is one.
///
/// Storage is unavailable in some privacy modes; a failure there just means
/// starting from the sample document.
pub fn load(key: &str) -> Option<String> {
    let stored = web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?;
    stored.filter(|saved| !saved.is_empty())
}

pub fn save(key: &str, source: &str) {
    if let Some(Ok(Some(storage))) = web_sys::window().map(|window| window.local_storage()) {
        let _ = storage.set_item(key, source);
    }
}

/// Triggers a browser download of `content`.
pub fn download(file_name: &str, mime: &str, content: &str) -> Option<()> {
    let document = web_sys::window()?.document()?;

    let parts = Array::new();
    parts.push(&content.into());
    let properties = BlobPropertyBag::new();
    properties.set_type(mime);
    let blob = Blob::new_with_str_sequence_and_options(&parts, &properties).ok()?;

    let url = Url::create_object_url_with_blob(&blob).ok()?;
    let anchor: HtmlAnchorElement = document.create_element("a").ok()?.dyn_into().ok()?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    anchor.click();

    // The object URL would pin the blob for the lifetime of the document.
    let _ = Url::revoke_object_url(&url);
    Some(())
}

/// Reads a picked `.adoc` file, handing the text to `on_load`.
pub fn read_file(file: &File, on_load: impl Fn(String) + 'static) -> Option<()> {
    read(file, on_load, FileReader::read_as_text)
}

/// Reads a picked file as a `data:` URL, for embedding it in the document.
pub fn read_data_url(file: &File, on_load: impl Fn(String) + 'static) -> Option<()> {
    read(file, on_load, FileReader::read_as_data_url)
}

fn read(
    file: &File,
    on_load: impl Fn(String) + 'static,
    start: fn(&FileReader, &Blob) -> Result<(), wasm_bindgen::JsValue>,
) -> Option<()> {
    let reader = FileReader::new().ok()?;
    let handle = reader.clone();

    let onload = Closure::<dyn FnMut()>::new(move || {
        if let Some(text) = handle.result().ok().and_then(|value| value.as_string()) {
            on_load(text);
        }
    });
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    // The closure must outlive this call; the reader fires it exactly once.
    onload.forget();

    start(&reader, file).ok()
}
