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

The editor fills the element it is given, so give that element a height.

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
it. Two things a strict `style-src` still takes away: `style="…"` attributes in
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
