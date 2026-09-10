//! AsciiDoc -> HTML5, the only place the `asciidoc-html5` crate is touched.
//!
//! Only the in-memory entry points are used. `convert_file`/`load_file` compile
//! for wasm but hit `std::fs`, which does not exist in the browser.

use asciidoc_html5::{Options, ReferenceTime, SafeMode};

/// Body-only HTML for the preview pane, annotated with `data-source-line`.
fn preview_options() -> Options {
    pinned(Options::new().embedded(true).source_locations(true))
}

/// Pins the clock that drives `docdate`, `doctime` and their `local*` siblings.
///
/// Without this the renderer calls `SystemTime::now()`, which on
/// `wasm32-unknown-unknown` is not merely unavailable but *panics* — and a
/// panic aborts the whole wasm module, taking the editor down with it. The
/// browser has a perfectly good clock; hand it over instead.
fn pinned(options: Options) -> Options {
    let now = now();
    options.reference_time(now.clone()).input_mtime(now)
}

#[cfg(target_arch = "wasm32")]
fn now() -> ReferenceTime {
    let now = js_sys::Date::new_0();

    ReferenceTime::from_local(
        i64::from(now.get_full_year()),
        now.get_month() + 1, // JS months are 0-based.
        now.get_date(),
        now.get_hours(),
        now.get_minutes(),
        now.get_seconds(),
        // JS reports minutes *behind* UTC; the crate wants seconds ahead of it.
        -(now.get_timezone_offset() as i32) * 60,
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn now() -> ReferenceTime {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);

    ReferenceTime::from_unix_timestamp(secs)
}

/// Renders `src` for the preview iframe, alongside any parser warnings.
pub fn render(src: &str) -> (String, Vec<Warning>) {
    let doc = asciidoc_html5::load_with(src, &preview_options());
    let warnings = doc
        .warnings()
        .map(|w| Warning {
            line: w.source.line(),
            message: humanize(&format!("{:?}", w.warning)),
        })
        .collect();

    (
        asciidoc_html5::convert_document_with(&doc, &preview_options()),
        warnings,
    )
}

/// Renders a complete, self-contained HTML file for download.
///
/// `SafeMode::Secure` (the default) forces `linkcss`, which would leave the
/// downloaded file unstyled when opened from `file://`; `Safe` embeds the
/// stylesheet instead. There is no filesystem to protect in wasm, so the
/// relaxation costs nothing here.
pub fn render_standalone(src: &str) -> String {
    let opts = pinned(
        Options::new()
            .standalone(true)
            .safe_mode(SafeMode::Safe)
            .unset("linkcss"),
    );

    asciidoc_html5::convert_with(src, &opts)
}

/// Turns a `WarningType` debug string into something readable in a status bar:
/// `UnterminatedDelimitedBlock` -> `unterminated delimited block`.
///
/// The crate does not implement `Display` for its warning types, and the debug
/// form is close enough to prose to be worth reshaping rather than maintaining
/// a match arm per variant.
fn humanize(debug: &str) -> String {
    // Debug prints the full path, e.g. `WarningType::UnterminatedDelimitedBlock`.
    let debug = debug.rsplit("::").next().unwrap_or(debug);

    // Variants with payloads debug as `Name(..)`; only the name is reshaped.
    let (name, payload) = match debug.find('(') {
        Some(at) => (&debug[..at], &debug[at..]),
        None => (debug, ""),
    };

    let mut words = String::with_capacity(name.len() + 8);
    for (i, c) in name.char_indices() {
        if c.is_uppercase() && i > 0 {
            words.push(' ');
        }
        words.extend(c.to_lowercase());
    }
    words.push_str(payload);
    words
}

/// A parser warning, flattened for display.
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub line: usize,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_body_only_with_source_lines() {
        let (html, warnings) = render("= Title\n\nHello *world*.");

        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("data-source-line="));
        assert!(!html.contains("<html"), "embedded output must be body-only");
        assert!(warnings.is_empty());
    }

    /// Time-dependent attributes must resolve without touching `SystemTime`.
    #[test]
    fn resolves_date_attributes() {
        let (html, _) = render("= Title\n\nBuilt {docdate} at {doctime}.");

        assert!(!html.contains("{docdate}"), "docdate should resolve: {html}");
        assert!(!html.contains("{doctime}"), "doctime should resolve: {html}");
    }

    #[test]
    fn standalone_output_embeds_the_stylesheet() {
        let html = render_standalone("= Title\n\nBody.");

        assert!(html.contains("<html"));
        assert!(html.contains("<style>"), "export must not link the stylesheet");
    }

    #[test]
    fn humanizes_warning_names() {
        assert_eq!(
            humanize("WarningType::UnterminatedDelimitedBlock"),
            "unterminated delimited block"
        );
        assert_eq!(humanize("UnterminatedDelimitedBlock"), "unterminated delimited block");
        assert_eq!(humanize("MissingAttribute(\"x\")"), "missing attribute(\"x\")");
        assert_eq!(humanize("Empty"), "empty");
    }

    #[test]
    fn warnings_carry_a_line_number() {
        // An unterminated delimited block is a classic silent-failure mode.
        let (_, warnings) = render("= Title\n\n----\nnever closed\n");

        assert!(!warnings.is_empty(), "expected a warning");
        assert!(warnings[0].line > 0);
    }
}
