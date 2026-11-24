import init, { mount } from "../pkg/asciidoc_editor.js";

await init();

const editor = mount("#editor", {
  value:
    "= Themed\n\nThis editor takes its colours from the page around it.\n\n" +
    "== A section\n\n* A list item with a https://example.com[link]\n* `code` too\n\n" +
    "NOTE: An admonition.\n\n[source,sh]\n----\necho hello\n----\n",
  theme: "auto",
  toolbar: { hide: ["brand", "file"] },
});

// Kept on the window so it is not dropped, which would unmount it.
window.editor = editor;
