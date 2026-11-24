//! The editor's colours and fonts, as a page sets them.
//!
//! A page themes the editor with custom properties (`--ade-accent` and the
//! rest, listed in the README) on the element it mounts into or on any
//! ancestor. The editor's stylesheet never sets those itself: it reads each
//! one through `var(--ade-x, default)` into a private `--_ade-x`, so whatever
//! the page sets wins whatever its selector, and nothing the editor does can
//! shadow an inherited value.
//!
//! Custom properties do not cross into the preview iframe, which is a
//! document of its own. The preview's are therefore read off the editor's
//! element once resolved and written into a stylesheet of the preview's own
//! — see [`preview_sheet`].

/// Which set of defaults the editor's own chrome starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    /// Light or dark as the system prefers.
    Auto,
}

impl Theme {
    /// From the `theme` option. Without one the editor is dark, as it always
    /// has been.
    pub fn parse(name: Option<&str>) -> Result<Theme, String> {
        match name {
            None | Some("dark") => Ok(Theme::Dark),
            Some("light") => Ok(Theme::Light),
            Some("auto") => Ok(Theme::Auto),
            Some(other) => Err(format!(
                "theme: unknown theme \"{other}\"; known: dark, light, auto"
            )),
        }
    }

    /// The value of the `data-ade-theme` attribute the stylesheet keys on.
    pub fn attribute(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Auto => "auto",
        }
    }
}

/// What the preview takes from the page, by the name a page sets (with
/// `--ade-` in front). Each is resolved on the editor's element as
/// `--_ade-<name>`.
pub const PREVIEW_PROPERTIES: [&str; 17] = [
    "preview-bg",
    "preview-fg",
    "preview-strong",
    "preview-muted",
    "preview-heading",
    "preview-title",
    "preview-link",
    "preview-code",
    "preview-code-bg",
    "preview-surface",
    "preview-border",
    "preview-accent",
    "preview-font",
    "preview-heading-font",
    "preview-heading-weight",
    "preview-mono-font",
    "preview-scheme",
];

/// Where the resolved value of `--ade-<name>` is read from on the editor's
/// element.
pub fn resolved_name(name: &str) -> String {
    format!("--_ade-{name}")
}

/// A stylesheet for the preview document, setting each of `values` — pairs of
/// a name from [`PREVIEW_PROPERTIES`] and its resolved value — as
/// `--ade-<name>` on its root.
///
/// The values come from the page's own stylesheets, but they are still
/// pasted into CSS text: one that could close the declaration or the rule,
/// or start markup, is left out, and the preview falls back to its default
/// for it.
pub fn preview_sheet<'a>(values: impl IntoIterator<Item = (&'a str, String)>) -> String {
    let declarations: String = values
        .into_iter()
        .filter(|(name, _)| PREVIEW_PROPERTIES.contains(name))
        .filter_map(|(name, value)| {
            let value = value.trim();
            let safe = !value.is_empty()
                && !value.contains([';', '{', '}', '<', '>', '\\'])
                && !value.contains("/*");
            safe.then(|| format!("--ade-{name}:{value};"))
        })
        .collect();

    format!(":root{{{declarations}}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_parse_and_default_to_dark() {
        assert_eq!(Theme::parse(None), Ok(Theme::Dark));
        assert_eq!(Theme::parse(Some("light")), Ok(Theme::Light));
        assert_eq!(Theme::parse(Some("auto")), Ok(Theme::Auto));
        assert_eq!(Theme::parse(Some("dark")).map(Theme::attribute), Ok("dark"));

        let error = Theme::parse(Some("Solarized")).unwrap_err();
        assert!(error.contains("\"Solarized\""), "{error}");
    }

    #[test]
    fn the_preview_sheet_sets_the_public_names() {
        let sheet = preview_sheet([
            ("preview-bg", "rgb(30, 25, 27)".to_string()),
            (
                "preview-font",
                " ui-sans-serif, \"Segoe UI\", sans-serif ".to_string(),
            ),
        ]);

        assert_eq!(
            sheet,
            ":root{--ade-preview-bg:rgb(30, 25, 27);--ade-preview-font:ui-sans-serif, \"Segoe UI\", sans-serif;}"
        );
    }

    #[test]
    fn empty_and_unknown_values_are_left_out() {
        let sheet = preview_sheet([
            ("preview-bg", String::new()),
            ("preview-fg", "  ".to_string()),
            ("bg", "red".to_string()),
        ]);

        assert_eq!(sheet, ":root{}");
    }

    #[test]
    fn a_value_cannot_break_out_of_its_declaration() {
        for hostile in [
            "red;} body{display:none",
            "red} html{",
            "red</style><script>",
            "\\7d red",
            "red /* comment",
        ] {
            let sheet = preview_sheet([("preview-link", hostile.to_string())]);
            assert_eq!(sheet, ":root{}", "{hostile}");
        }
    }

    #[test]
    fn every_preview_property_is_used_by_the_preview_style() {
        let css = crate::app::preview_style();
        for name in PREVIEW_PROPERTIES {
            assert!(
                css.contains(&format!("var(--ade-{name},")),
                "--ade-{name} unused"
            );
        }
    }

    #[test]
    fn every_preview_property_is_resolved_by_the_editor_style() {
        for name in PREVIEW_PROPERTIES {
            let resolved = format!("{}: var(--ade-{name},", resolved_name(name));
            assert!(crate::STYLE.contains(&resolved), "{resolved} missing");
        }
    }
}
