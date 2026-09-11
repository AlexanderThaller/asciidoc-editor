# AsciiDoc editor

A WYSIWYG AsciiDoc editor for the web, written in Rust and compiled to
WebAssembly. You edit the rendered document — click a heading and type in it,
press Tab in a list to indent — and the AsciiDoc source stays the thing being
edited underneath.

Parsing and rendering are [`asciidoc-parser`] and [`asciidoc-html5`]; the
interface is [Leptos].

## Using it in a page

Build the package:

```sh
./build-package.sh
```

That writes `pkg/`: an ES module, its wasm, and TypeScript types. Serve those
four files next to your page — there is nothing else to serve, because the
stylesheets are inside the wasm.

```html
<div id="editor" style="height: 34rem"></div>

<script type="module">
  import init, { mount } from "./pkg/asciidoc_editor.js";

  await init();
  const editor = mount("#editor", { value: "= Title\n\nWords.\n" });

  editor.onChange((document) => save(document));
</script>
```

The editor fills the element it is given, so give that element a height.

### `mount(target, options?)`

`target` is a CSS selector or an element. The element keeps whatever classes
the page put on it.

| Option | Default | |
|---|---|---|
| `value` | the sample document | the document to open with |
| `autosave` | none | a `localStorage` key to keep the document under |
| `richText` | `true` | start in rich text rather than source |

With `autosave` set and no `value`, the editor opens whatever was last saved
under that key.

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

## Developing

```sh
trunk serve      # the editor on its own at http://localhost:8080
cargo test       # the source-editing and parsing logic
```

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
