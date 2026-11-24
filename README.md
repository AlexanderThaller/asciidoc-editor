# AsciiDoc editor

A WYSIWYG AsciiDoc editor for the web, written in Rust and compiled to
WebAssembly. You edit the rendered document — click a heading and type in it,
press Tab in a list to indent — and the AsciiDoc source stays the thing being
edited underneath.

Parsing and rendering are [`asciidoc-parser`] and [`asciidoc-html5`]; the
interface is [Leptos].

<!--
  Absolute, not relative: this README is copied into the npm package, and
  npmjs.com cannot resolve a path into the repository. And media., not raw.,
  because the screenshot is held in git-lfs — raw.githubusercontent.com hands
  back the pointer file rather than the image.
-->
![The editor showing a rendered AsciiDoc document — a title, a table of
contents, headings, a list, a code block and a table — with the formatting
toolbar above
it.](https://media.githubusercontent.com/media/AlexanderThaller/asciidoc-editor/main/docs/screenshot.png)

## Using it in a page

```sh
npm install asciidoc-editor
```

Or build it yourself, which is what `npm publish ./pkg` publishes:

```sh
./build-package.sh
```

Either way you get an ES module, its wasm, and TypeScript types — and nothing
else to serve, because the stylesheets are inside the wasm.

```html
<div id="editor" style="height: 34rem"></div>

<script type="module">
  import init, { mount } from "./pkg/asciidoc_editor.js";

  await init();
  const editor = mount("#editor", { value: "= Title\n\nWords.\n" });

  editor.onChange((document) => save(document));
</script>
```

The editor fills the element it is given, so give that element a height —
by id, class or anything else: the editor's own sizing (`height: 100%`, a
`min-height` of 20rem) is a default at zero specificity, so any rule of the
page's wins, and so do the element's margins, padding and border. One catch:
the editor's stylesheet is not in a cascade layer, and an unlayered rule beats
every layered one, so a height set inside `@layer` (Tailwind's
`@layer components`, say) loses to the default. Set it outside any layer.

Loading the module from a bundler works the same way. The wasm is fetched
relative to the module (`new URL(..., import.meta.url)`), which Vite, webpack 5
and esbuild all understand without configuration.

### TypeScript

The types ship with the package. wasm-bindgen writes a `[Symbol.dispose]()` on
its classes, so a project compiling the package's own declarations needs either
`"skipLibCheck": true` — which most templates set already — or `"lib"` at
`ESNext`.

### `mount(target, options?)`

`target` is a CSS selector or an element (`MountTarget`). The element keeps
whatever classes the page put on it.

| Option | Default | |
|---|---|---|
| `value` | the sample document | the document to open with |
| `autosave` | none | a `localStorage` key to keep the document under |
| `richText` | `true` | start in rich text rather than source |
| `onFile` | none | takes pasted, dropped and picked files; answers with the AsciiDoc to put in for each (`FileCallback`) |
| `baseUrl` | the page's address | what relative image and link targets in the rendering resolve against |
| `toolbar` | every button | which toolbar buttons to show (`{ show?, hide? }`, see [Toolbar](#toolbar)) |
| `theme` | `"dark"` | `"dark"`, `"light"`, or `"auto"` to follow `prefers-color-scheme`: the defaults the colours start from (see [Theming](#theming)) |

`mount` throws on an unknown toolbar id or theme, rather than quietly leaving
a button the page meant to hide.

With `autosave` set and no `value`, the editor opens whatever was last saved
under that key.

A document written with CRLF line ends comes back with them: the editor works
on LF, and `value()` and `onChange` hand the CRLF back, so editing one block
does not change every line.

#### Files

Without `onFile`, a picture that is pasted, dropped or picked in the image
panel is embedded in the document as a `data:` URI, up to 2 MB. With it, every
file goes to the page instead — all of the files of a paste or drop, one call
each, in order — and whatever the page answers goes in as a block of its own,
below the block being edited:

```js
mount("#editor", {
  value,
  baseUrl: "/files/",
  onFile: async (file) => {
    const body = new FormData();
    body.append("file", file);
    const response = await fetch("/upload", { method: "POST", body });
    if (!response.ok) throw new Error(await response.text());
    const { path } = await response.json();
    return file.type.startsWith("image/")
      ? `image::${path}[${file.name}]`
      : `link:${path}[${file.name}]`;
  },
});
```

The answer may be a string or a promise of one. Nothing, or an empty string,
puts nothing in; a throw or a rejection puts nothing in either, and its message
is shown in the status bar. Picked from the panel while it is changing an
image, an answer of `image::target[...]` points that image at the new target.

#### Relative targets

The rendering lives in an iframe whose address is `about:srcdoc`, so a
relative target such as `image::assets/a.png[]` resolves against the page.
`baseUrl` — absolute, or relative to the page — gives the iframe a `<base>`
instead: with `baseUrl: "/files/"` that image loads from `/files/assets/a.png`.
The document itself is not changed.

### Toolbar

Every button has a stable id; `show` keeps only the ids it names, and `hide`
then takes away the ones it names. Either can name a whole group.

```js
// A page that saves the document itself has no use for the file buttons.
mount("#editor", { toolbar: { hide: ["brand", "file"] } });

// Only formatting, and undo.
mount("#editor", { toolbar: { show: ["format", "history"] } });
```

| Group | Ids | |
|---|---|---|
| | `brand` | the "AsciiDoc" label |
| | `mode` | the Rich text / Source switch |
| `format` | `bold`, `italic`, `link`, `code` | inline formatting (rich text only) |
| `lists` | `bulletList`, `admonition`, `numberedList` | lists, and the note admonition (rich text only) |
| `blocks` | `body`, `quote`, `codeBlock`, `headings` | the kind of block; `headings` is all three heading buttons (rich text only) |
| `history` | `undo`, `redo` | |
| `insert` | `table`, `rule`, `image` | insert a table, a thematic break, an image |
| `file` | `open`, `saveAdoc`, `exportHtml` | open a file, download `.adoc`, download `.html` |

The separators follow what is left: none at either end, none around a group
that is gone. With nothing left the toolbar row is not drawn at all. Hiding a
button does not take its shortcut away (ctrl+B, ctrl+I, ctrl+E, ctrl+Z), nor
the bar below it that acts on the block being edited. Hiding `mode` leaves the
editor in the mode `richText` chose.

### Theming

The editor's colours, fonts and radius are custom properties a page sets —
on the element it mounts into, or on any ancestor — in a stylesheet of its own:

```css
#editor {
  --ade-bg: #fff;
  --ade-accent: #a9434b;
  --ade-preview-link: #a9434b;
}

@media (prefers-color-scheme: dark) {
  #editor {
    --ade-bg: #1e191b;
    --ade-accent: #e08a79;
    --ade-preview-bg: #272022;
    --ade-preview-fg: #f3e9e5;
    --ade-preview-scheme: dark;
  }
}
```

The editor never sets an `--ade-*` property itself, so whatever the page sets
wins, whatever its selector. Anything left unset falls back to the editor's
own look, which `theme` picks: `"dark"` (the default, as the editor has always
looked), `"light"`, or `"auto"` to follow `prefers-color-scheme`. The theme
only decides the fallbacks of the editor's chrome; the preview falls back to
Asciidoctor's look, a white page, in every theme.

The editor's chrome — toolbars, source pane, status bar, panels:

| Property | Dark | Light | |
|---|---|---|---|
| `--ade-bg` | `#1b1f24` | `#ffffff` | the source pane, and behind everything |
| `--ade-bg-raised` | `#22272e` | `#f6f8fa` | toolbar, status bar, panels |
| `--ade-bg-context` | `#1e242c` | `#eef1f4` | the bar for the block being edited |
| `--ade-border` | `#343b45` | `#d0d7de` | borders and separators |
| `--ade-fg` | `#d5dae1` | `#1f2328` | text |
| `--ade-fg-dim` | `#8b95a3` | `#656d76` | hints, labels, the status bar |
| `--ade-fg-hover` | `#fff` | `#000` | a button's text under the pointer |
| `--ade-accent` | `#7fb4ff` | `#0969da` | active buttons, focus, the caret |
| `--ade-accent-fg` | `#10151c` | `#fff` | text on the accent |
| `--ade-button-bg` | `#2b313a` | `#fff` | buttons and chips |
| `--ade-input-bg` | `#10151c` | `#fff` | the panels' text fields |
| `--ade-danger` | `#ff9d9d` | `#cf222e` | delete buttons |
| `--ade-danger-border` | `#5b3138` | `#f3b8bd` | |
| `--ade-danger-bg` | `#4a262c` | `#ffebe9` | a delete button under the pointer |
| `--ade-warn` | `#e6c07b` | `#9a6700` | warnings in the status bar |
| `--ade-selection` | `rgba(127,180,255,.28)` | `rgba(9,105,218,.2)` | selected source |
| `--ade-shadow` | `rgba(0,0,0,.35)` | `rgba(31,35,40,.15)` | under the panels |
| `--ade-syntax-heading`, `-delim`, `-verbatim`, `-attr`, `-title`, `-admonition`, `-marker`, `-bold`, `-italic`, `-code` | | | the source highlighting |

Not themed by light or dark:

| Property | Default | |
|---|---|---|
| `--ade-radius` | `5px` | buttons and fields |
| `--ade-font` | `system-ui, -apple-system, "Segoe UI", sans-serif` | the interface |
| `--ade-font-size` | `13px` | the interface; smaller text scales with it |
| `--ade-mono-font` | `ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace` | the source pane |
| `--ade-mono-size` | `13.5px` | the source pane |
| `--ade-mono-line` | `1.6` | the source pane's line height |
| `--ade-pad` | `1rem` | the source pane's padding |

The rendering, in the preview iframe. Custom properties do not cross into an
iframe, so the editor reads these off its element, resolved, and writes them
into a stylesheet of the preview's own: on load, at every render, and when the
system switches between light and dark. A value that could break out of its
declaration (`;`, `{`, `}`, `<`, `>`, `\`, `/*`) is left out.

| Property | Default (Asciidoctor's) | |
|---|---|---|
| `--ade-preview-bg` | `#fff` | the page |
| `--ade-preview-fg` | `rgba(0,0,0,.8)` | text, table cells |
| `--ade-preview-strong` | `rgba(0,0,0,.85)` | the document title, quotes, a lead paragraph |
| `--ade-preview-muted` | `rgba(0,0,0,.6)` | admonition text, attributions |
| `--ade-preview-heading` | `#ba3925` | section headings |
| `--ade-preview-title` | `#7a2518` | block titles, the table of contents' title |
| `--ade-preview-link` | `#2156a5` | links |
| `--ade-preview-code` | `rgba(0,0,0,.9)` | code text |
| `--ade-preview-code-bg` | `#f7f7f8` | code and listing blocks |
| `--ade-preview-surface` | `#f8f8f7` | table heads and stripes, sidebars, the table of contents |
| `--ade-preview-border` | `#dddddf` | rules, table and block borders |
| `--ade-preview-accent` | `#7fb4ff` | the editable block under the pointer, in focus, and where a new one would go |
| `--ade-preview-font` | `"Noto Serif", "DejaVu Serif", serif` | text |
| `--ade-preview-heading-font` | `"Open Sans", "DejaVu Sans", sans-serif` | headings |
| `--ade-preview-heading-weight` | `300` | headings |
| `--ade-preview-mono-font` | `"Droid Sans Mono", "DejaVu Sans Mono", monospace` | code |
| `--ade-preview-scheme` | `light` | the preview's `color-scheme`: its scrollbars and form controls |

A page that changes these by other means than a media query (a theme switch of
its own, say) has them picked up by the preview at the next render.

`demo/themed.html` is a page doing this: a height on a class, colours for light
and dark, and no file buttons — under the strict policy below.

### Content Security Policy

The editor runs under a strict policy. It has no inline scripts or event
handlers, and its stylesheets are constructed (`new CSSStyleSheet()`, adopted
by the page and by the preview iframe), which `style-src` does not cover — so
no `'unsafe-inline'`, hash or nonce is needed for them. What it does need:

| Directive | Needs | For |
|---|---|---|
| `script-src` | `'self' 'wasm-unsafe-eval'` | the module, and compiling its wasm |
| `connect-src` | `'self'` | fetching the wasm (`init()` does it with `fetch`) |
| `style-src` | nothing more than `'self'`, or even `'none'` | |
| `img-src` | `'self'`; `data:` for embedded images | images in the document |
| `base-uri` | `'self'` | only with `baseUrl` (the iframe's `<base>`) |

For example, with the package and the images on the page's own origin:

```
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self';
style-src 'self'; img-src 'self' data:; base-uri 'self'
```

The preview iframe is `srcdoc`, so it inherits the page's policy and needs no
`frame-src`; `frame-ancestors 'none'` and `X-Frame-Options: DENY` do not stop
it. Theming needs nothing more either: the page sets the `--ade-*` properties in
a stylesheet of its own, which `style-src 'self'` allows, and the editor
carries the preview's in through another constructed stylesheet. Two things a
strict `style-src` still takes away: `style="…"` attributes in
the rendering (column widths of a table with `cols`, for one), and the
fallback for browsers without constructed stylesheets (Safari before 16.4),
where the editor puts `<style>` elements on the page instead.

### The handle

| | |
|---|---|
| `editor.value()` | the document, as AsciiDoc |
| `editor.setValue(source)` | replace the document |
| `editor.onChange(callback)` | called with the document whenever it changes |
| `editor.destroy()` | take the editor off the page |

Hold on to the handle. Dropping it unmounts the editor, which is what
`destroy()` does deliberately; after it the element is as it was found, and can
be mounted into again.

`demo/index.html` is a page doing all of this. Serve the repository root and
open `/demo/`:

```sh
python3 -m http.server 8000
```

### Publishing

Releases go out from CI, which needs no token and no second factor: npm trusts
`.github/workflows/release.yml` by name and the runner proves it is that
workflow with an OIDC token.

```sh
git tag v0.1.1 && git push origin v0.1.1
```

The workflow refuses to publish if the tag and the version in `Cargo.toml`
disagree.

Setting that trust up is a one-off, and it has an awkward first step: npm will
only configure a trusted publisher for a package that already exists, so the
very first release has to be published by hand.

1. `./build-package.sh && npm publish ./pkg` — the leading `./` matters, since
   `npm publish pkg` reads `pkg` as the name of a package on the registry and
   tries to publish that one. Publishing by hand needs 2FA on the account: npm
   now accepts only a security key, added at
   `npmjs.com/settings/<user>/tfa`.
2. On the package's settings page at npmjs.com, add a trusted publisher: this
   repository, workflow `release.yml`.
3. Every release after that is the tag above.

The trusted publisher is a record against the package, not against the
repository, and `npm publish` fails with "package not found" when it is
missing — whatever the package's own state. `npm trust list asciidoc-editor`
shows it; `npm trust github asciidoc-editor --file release.yml --repo
AlexanderThaller/asciidoc-editor --allow-publish` creates it. The publish
grant is not implied, and the environment must be left unset, because the
workflow declares none.

## Licence

MIT. See `LICENSE`.

## Developing

Every tool this needs is in the flake, pinned by `flake.lock` — the same
compiler, generator and optimiser here as in CI:

```sh
nix develop
trunk serve      # the editor on its own at http://localhost:8080
cargo test       # the source-editing and parsing logic
```

With direnv, `direnv allow` enters it for you. Without Nix it still builds from
whatever is on PATH, so long as `wasm-bindgen` matches the version in
`Cargo.lock` — `build-package.sh` refuses to run when it does not, because a
mismatch yields a module the browser rejects at load.

That pin runs the other way too: `wasm-bindgen` is held at an exact version in
`Cargo.toml` because it has to match the CLI nixpkgs provides. Moving one means
moving both.

`trunk serve` runs `src/main.rs`, which is the same library mounted onto a bare
page.

### How it works

The AsciiDoc source is the only state. It is rendered to HTML with source
locations turned on, which puts a `data-source-line` on every block, and that
HTML goes into an iframe. Blocks in the iframe are made `contenteditable`; when
one changes, its DOM is written back out as AsciiDoc and spliced over the lines
it came from.

A block is only made editable if writing its DOM back reproduces its source
exactly. Anything that fails that check is left alone and can still be edited
in source mode — better a block that refuses to be edited in place than one
that quietly rewrites itself.

The block being edited is never re-rendered, or the caret would be lost; the
line numbers of the blocks below it are shifted instead.

### What stays source-only

Cell specifiers (`2+|`, `a|`, `^|`), explicitly numbered ordered lists, image
sizing attributes, and anything else that would not survive the round trip.
Strikethrough is not implemented.

[`asciidoc-parser`]: https://crates.io/crates/asciidoc-parser
[`asciidoc-html5`]: https://crates.io/crates/asciidoc-html5
[Leptos]: https://leptos.dev
