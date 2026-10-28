# X Studio plugins

A plugin adds a page to the sidebar or commands to the book editor. It is a folder with a `manifest.json` and a web page, installed from a `.zip`
(**Plugins → Install from ZIP…**).

The page runs in a sandboxed frame: it has no network access of its own, can't see the app,
and can't navigate anywhere. Everything it does — HTTPS requests, stored tokens, the reader —
goes through `window.xstudio`, and X Studio checks each call against the manifest. The user
sees the permissions before installing.

## Layout

```
my-plugin/
├─ manifest.json
├─ index.html        ← the page (manifest "entry")
├─ main.js
└─ icon.svg          ← optional (manifest "icon")
```

Pack it with `node scripts/build-plugin.mjs my-plugin` (→ `release/plugins/<id>-<version>.zip`)
or any ZIP tool — the files may sit at the archive root or in one top-level folder.

## manifest.json

```json
{
  "id": "dev.example.my-plugin",
  "name": "My plugin",
  "version": "1.0.0",
  "description": "One line for the Plugins page.",
  "author": "You",
  "apiVersion": 1,
  "entry": "index.html",
  "icon": "icon.svg",
  "permissions": {
    "network": ["api.example.com", "*.cdn.example.com"],
    "authWindow": ["login.example.com"],
    "secrets": true,
    "device": ["read", "write"]
  },
  "contributes": {
    "section": { "title": "My plugin" },
    "bookActions": [{ "id": "fix-metadata", "title": "Fix metadata", "formats": ["epub"] }],
    "settings": true
  }
}
```

- `id` — lowercase letters, digits, dots, dashes; also the install folder. Installing a ZIP
  with the same id updates the plugin and keeps its settings.
- `apiVersion` — `1`.
- At least one of `section`, `bookActions`.

### Permissions

| Permission | Allows |
| --- | --- |
| `network` | `xstudio.http.fetch` to these hosts, HTTPS only. `example.com` is exact, `*.example.com` is any subdomain, `*` is any site (the install dialog warns about it). Redirects are followed by the app and checked hop by hop. Images (`<img>`) may load from the same hosts. |
| `authWindow` | `xstudio.auth.signIn` may open a sign-in window on these hosts. |
| `secrets` | Store tokens with `xstudio.secrets`. They are **write-only**: the plugin can't read them back, only use them as `{{secret:NAME}}` in request headers. |
| `device` | `read` — names of the books on the reader. `write` — send books to the reader. |

Plugin data lives in the app's data folder (`plugin-data/<id>/`, owner-only files) and is
removed with the plugin unless the user keeps it.

## The page

```html
<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <link rel="stylesheet" href="/_sdk/xstudio.css" />   <!-- the app's look, optional -->
    <script src="/_sdk/xstudio.js"></script>             <!-- the SDK, first -->
    <script src="main.js" defer></script>
  </head>
  <body><div id="app"></div></body>
</html>
```

Use classic scripts (or bundle your code into one): the page has an opaque origin, so ES module
imports depend on CORS for the plugin scheme, which not every WebView grants.

Forms can't be submitted (the frame has no `allow-forms`): handle Return in `keydown`
instead of `<form onsubmit>`.

The page's CSP allows scripts, styles and fonts from the plugin itself (inline too), images
from the plugin, `data:`, `blob:` and the `network` hosts — and nothing else: no `fetch`,
`XMLHttpRequest`, WebSocket, workers, frames or forms.

`xstudio.css` gives the app's colours (kept in sync with the app's light/dark theme), buttons,
inputs, `.seg`, `.chips`, `.xs-grid` and `.xs-cover`.

## API (`window.xstudio`)

Everything returns a Promise; errors are rejections with a message.

```js
const init = await xstudio.ready;        // {manifest, view: "section" | "settings", theme}
xstudio.on("view", (view) => …);         // the app shows the page as its section or its settings
xstudio.on("theme", (theme) => …);       // colours changed (applied for you)

// HTTPS to permissions.network hosts
const r = await xstudio.http.fetch("https://api.example.com/v1/books", {
  method: "POST",                         // GET (default), POST, PUT, PATCH, DELETE, HEAD
  headers: { Authorization: "Bearer {{secret:token}}" },
  json: { query: "…" },                   // or body: "text" | Uint8Array
});
r.status; r.ok; r.headers; r.url;         // headers are lower-case
r.json(); r.text(); r.bytes();            // sync, the body is already here (up to 100 MB)

await xstudio.storage.set("lastQuery", "…");   // JSON values, up to 1 MB in all
await xstudio.storage.get("lastQuery");
await xstudio.storage.delete("lastQuery");
await xstudio.storage.keys();

await xstudio.secrets.set("token", value);     // needs "secrets"
await xstudio.secrets.has("token");            // → true/false
await xstudio.secrets.delete("token");

// Opens url in a sign-in window; when it redirects to a URL with ?param= or #param=,
// the value is stored as secret `secret` and the window closes. Rejects if the user closes it.
await xstudio.auth.signIn({ url: "https://login.example.com/authorize?…", secret: "token", param: "access_token" });

await xstudio.device.sendBook("Author. Title.epub", bytes);   // needs device "write"; goes to /Books
await xstudio.device.books();                                 // needs device "read" → ["a.epub", …]

// EPUB clean-up for store downloads: metadata into the OPF, a cover page, XHTML fixed,
// embedded fonts dropped. → {data, fileName}
await xstudio.epub.prepare(bytes, {
  title, authors: [], annotation, genres: [], publishers: [],
  series: [{ title, positionLabel }], source: "https://…", originalYear, rights,
}, coverBytes);

await xstudio.files.saveAs("Book.epub", bytes);   // the user picks where; false if cancelled
await xstudio.ui.toast("Done");                    // kind: "info" (default) | "error"
await xstudio.ui.confirm("Remove it?", { okLabel: "Remove" });   // → true/false
```

### A book action

Declared in `contributes.bookActions`; a button in the book editor for matching `formats`.
The app reads the book from the reader, calls the action, and writes back what it returns.

```js
xstudio.registerBookAction("fix-metadata", async ({ name, path, data }) => {
  const fixed = await doSomething(data);   // Uint8Array
  return { data: fixed };                  // or null to leave the book as it is
});
```

## Updating and removing

Install a newer ZIP with the same `id` to update. **Plugins** has Turn off / Turn on and
Remove (with or without the plugin's settings and tokens).
