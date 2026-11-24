//! Which of the toolbar's buttons a page wants, and the separators between
//! them.
//!
//! The page names buttons by stable ids — or whole groups of them — in the
//! `toolbar` option of `mount`: `show` keeps only those, `hide` takes those
//! away. Everything here is plain data, so it is tested without a browser.

/// One thing on the toolbar a page can show or hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// The "AsciiDoc" label.
    Brand,
    /// The Rich text / Source switch.
    Mode,
    Bold,
    Italic,
    Link,
    /// Inline monospace.
    Code,
    BulletList,
    NumberedList,
    Admonition,
    /// Back to an ordinary paragraph.
    Body,
    Quote,
    CodeBlock,
    /// The three heading buttons.
    Headings,
    Undo,
    Redo,
    Table,
    Rule,
    Image,
    /// Open a file from disk.
    Open,
    /// Download the document as `.adoc`.
    SaveAdoc,
    /// Download the rendering as `.html`.
    ExportHtml,
}

/// The runs of buttons that separators stand between, in toolbar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// The label and the mode switch, which no separator divides.
    Lead,
    Format,
    Lists,
    Blocks,
    History,
    Insert,
    /// Pushed to the far end of the bar.
    File,
}

impl Group {
    /// The groups in the order they sit on the bar.
    const ALL: [Group; 7] = [
        Group::Lead,
        Group::Format,
        Group::Lists,
        Group::Blocks,
        Group::History,
        Group::Insert,
        Group::File,
    ];

    /// What a page calls the group, if it can name it as a whole.
    fn id(self) -> Option<&'static str> {
        match self {
            Group::Lead => None,
            Group::Format => Some("format"),
            Group::Lists => Some("lists"),
            Group::Blocks => Some("blocks"),
            Group::History => Some("history"),
            Group::Insert => Some("insert"),
            Group::File => Some("file"),
        }
    }

    fn items(self) -> &'static [Item] {
        match self {
            Group::Lead => &[Item::Brand, Item::Mode],
            Group::Format => &[Item::Bold, Item::Italic, Item::Link, Item::Code],
            Group::Lists => &[Item::BulletList, Item::Admonition, Item::NumberedList],
            Group::Blocks => &[Item::Body, Item::Quote, Item::CodeBlock, Item::Headings],
            Group::History => &[Item::Undo, Item::Redo],
            Group::Insert => &[Item::Table, Item::Rule, Item::Image],
            Group::File => &[Item::Open, Item::SaveAdoc, Item::ExportHtml],
        }
    }

    /// Only there in rich text mode: they act on the block being edited.
    fn rich_only(self) -> bool {
        matches!(self, Group::Format | Group::Lists | Group::Blocks)
    }
}

impl Item {
    const ALL: [Item; 21] = [
        Item::Brand,
        Item::Mode,
        Item::Bold,
        Item::Italic,
        Item::Link,
        Item::Code,
        Item::BulletList,
        Item::NumberedList,
        Item::Admonition,
        Item::Body,
        Item::Quote,
        Item::CodeBlock,
        Item::Headings,
        Item::Undo,
        Item::Redo,
        Item::Table,
        Item::Rule,
        Item::Image,
        Item::Open,
        Item::SaveAdoc,
        Item::ExportHtml,
    ];

    /// The id a page names the item by. These are part of the API: never
    /// rename one.
    pub fn id(self) -> &'static str {
        match self {
            Item::Brand => "brand",
            Item::Mode => "mode",
            Item::Bold => "bold",
            Item::Italic => "italic",
            Item::Link => "link",
            Item::Code => "code",
            Item::BulletList => "bulletList",
            Item::NumberedList => "numberedList",
            Item::Admonition => "admonition",
            Item::Body => "body",
            Item::Quote => "quote",
            Item::CodeBlock => "codeBlock",
            Item::Headings => "headings",
            Item::Undo => "undo",
            Item::Redo => "redo",
            Item::Table => "table",
            Item::Rule => "rule",
            Item::Image => "image",
            Item::Open => "open",
            Item::SaveAdoc => "saveAdoc",
            Item::ExportHtml => "exportHtml",
        }
    }

    fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// The items a name stands for: one item, or every item of a group.
fn expand(name: &str) -> Result<u32, String> {
    if let Some(item) = Item::ALL.iter().find(|item| item.id() == name) {
        return Ok(item.bit());
    }

    Group::ALL
        .iter()
        .find(|group| group.id() == Some(name))
        .map(|group| group.items().iter().fold(0, |bits, item| bits | item.bit()))
        .ok_or_else(|| {
            let known: Vec<&str> = Group::ALL
                .iter()
                .filter_map(|group| group.id())
                .chain(Item::ALL.iter().map(|item| item.id()))
                .collect();
            format!(
                "toolbar: unknown item \"{name}\"; known: {}",
                known.join(", ")
            )
        })
}

/// The buttons a page asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toolbar {
    shown: u32,
}

impl Default for Toolbar {
    /// Every button.
    fn default() -> Self {
        Toolbar {
            shown: Item::ALL.iter().fold(0, |bits, item| bits | item.bit()),
        }
    }
}

impl Toolbar {
    /// `show`, when given, keeps only the items it names; `hide` then takes
    /// away the items it names. Either may name groups. A name that is
    /// neither is refused rather than ignored: a typo would otherwise
    /// quietly leave a button on the page.
    pub fn new<S: AsRef<str>>(show: Option<&[S]>, hide: &[S]) -> Result<Toolbar, String> {
        let mut shown = match show {
            Some(names) => names.iter().try_fold(0, |bits, name| {
                Ok::<_, String>(bits | expand(name.as_ref())?)
            })?,
            None => Toolbar::default().shown,
        };
        for name in hide {
            shown &= !expand(name.as_ref())?;
        }
        Ok(Toolbar { shown })
    }

    pub fn shows(self, item: Item) -> bool {
        self.shown & item.bit() != 0
    }

    /// Whether anything of `group` is on the bar, in rich text mode or not.
    pub fn shows_group(self, group: Group, rich: bool) -> bool {
        (rich || !group.rich_only()) && group.items().iter().any(|item| self.shows(*item))
    }

    /// Whether a separator stands before `group`: only between two groups
    /// that both have something showing, so hiding a group — or everything
    /// before it — leaves no stray line behind. The file group sits apart,
    /// at the far end, and has none.
    pub fn separator_before(self, group: Group, rich: bool) -> bool {
        if matches!(group, Group::Lead | Group::File) || !self.shows_group(group, rich) {
            return false;
        }

        Group::ALL
            .iter()
            .take_while(|earlier| **earlier != group)
            .any(|earlier| self.shows_group(*earlier, rich))
    }

    /// Whether the bar has anything on it at all; with nothing, it is left
    /// out rather than drawn as an empty strip.
    pub fn is_empty(self, rich: bool) -> bool {
        !Group::ALL
            .iter()
            .any(|group| self.shows_group(*group, rich))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolbar(show: Option<&[&str]>, hide: &[&str]) -> Toolbar {
        Toolbar::new(show, hide).unwrap()
    }

    #[test]
    fn everything_shows_by_default() {
        let bar = Toolbar::default();
        assert!(Item::ALL.iter().all(|item| bar.shows(*item)));
        assert_eq!(toolbar(None, &[]), bar);
    }

    #[test]
    fn every_item_has_its_own_bit_and_id() {
        for (index, item) in Item::ALL.iter().enumerate() {
            assert_eq!(*item as usize, index, "{item:?} out of order");
            assert_eq!(expand(item.id()), Ok(item.bit()));
        }
        assert!(Item::ALL.len() < 32);
    }

    #[test]
    fn every_item_is_in_exactly_one_group() {
        for item in Item::ALL {
            let groups = Group::ALL
                .iter()
                .filter(|group| group.items().contains(&item))
                .count();
            assert_eq!(groups, 1, "{item:?}");
        }
    }

    #[test]
    fn hide_takes_items_away() {
        let bar = toolbar(None, &["open", "saveAdoc", "exportHtml", "brand"]);
        assert!(!bar.shows(Item::Open));
        assert!(!bar.shows(Item::SaveAdoc));
        assert!(!bar.shows(Item::ExportHtml));
        assert!(!bar.shows(Item::Brand));
        assert!(bar.shows(Item::Bold));
        assert!(bar.shows(Item::Mode));
    }

    #[test]
    fn a_group_stands_for_its_items() {
        assert_eq!(
            toolbar(None, &["file"]),
            toolbar(None, &["open", "saveAdoc", "exportHtml"])
        );

        let bar = toolbar(Some(&["format", "undo"]), &[]);
        assert!(bar.shows(Item::Bold) && bar.shows(Item::Link) && bar.shows(Item::Code));
        assert!(bar.shows(Item::Undo));
        assert!(!bar.shows(Item::Redo));
        assert!(!bar.shows(Item::Mode));
    }

    #[test]
    fn hide_applies_after_show() {
        let bar = toolbar(Some(&["format"]), &["link"]);
        assert!(bar.shows(Item::Bold));
        assert!(!bar.shows(Item::Link));
    }

    #[test]
    fn unknown_names_are_refused() {
        let error = Toolbar::new(None, &["bolt"]).unwrap_err();
        assert!(error.contains("\"bolt\""), "{error}");
        assert!(error.contains("saveAdoc"), "lists the known ids: {error}");
        assert!(
            Toolbar::new(Some(&["Bold"]), &[]).is_err(),
            "ids are case-sensitive"
        );
    }

    #[test]
    fn separators_stand_only_between_visible_groups() {
        let all = Toolbar::default();
        assert!(!all.separator_before(Group::Lead, true));
        assert!(all.separator_before(Group::Format, true));
        assert!(all.separator_before(Group::Insert, true));
        assert!(
            !all.separator_before(Group::File, true),
            "the file group sits apart"
        );

        // Nothing before the formatting buttons: no line at the very start.
        let bar = toolbar(None, &["brand", "mode"]);
        assert!(!bar.separator_before(Group::Format, true));
        assert!(bar.separator_before(Group::Lists, true));

        // A hidden group takes its separator with it.
        let bar = toolbar(None, &["lists"]);
        assert!(!bar.separator_before(Group::Lists, true));
        assert!(bar.separator_before(Group::Blocks, true));
    }

    #[test]
    fn source_mode_drops_the_rich_text_groups() {
        // In source mode the history group follows the mode switch directly.
        let all = Toolbar::default();
        assert!(all.separator_before(Group::History, false));
        assert!(!all.separator_before(Group::Format, false));

        // With no mode switch, nothing comes before history in source mode.
        let bar = toolbar(None, &["brand", "mode"]);
        assert!(!bar.separator_before(Group::History, false));
        assert!(bar.separator_before(Group::History, true));
    }

    #[test]
    fn an_empty_bar_is_known_to_be_empty() {
        assert!(toolbar(Some(&[]), &[]).is_empty(true));
        assert!(!Toolbar::default().is_empty(false));

        let rich_only = toolbar(Some(&["format"]), &[]);
        assert!(!rich_only.is_empty(true));
        assert!(rich_only.is_empty(false));
    }
}
