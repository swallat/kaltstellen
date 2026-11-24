# kaltstellen main window: web files

The main window (`kaltstellen gui`) is this page, shown in WebView2. The
files are embedded into the executable at compile time (`src/gui/assets.rs`);
there is no build step, no framework and no network access.

| File | Purpose |
| --- | --- |
| `index.html` | Markup: profile list, profile detail, dialogs. |
| `css/tokens.css` | Design tokens (colours, type, spacing) from `branding/README.md`, light and dark. |
| `css/app.css` | Layout and components, using the tokens only. |
| `css/settings.css` | The Settings view (M6): section list, rows, switches. |
| `js/app.js` | The view: renders the state, sends intents. |
| `js/bridge.js` | Messages to the Rust side (`docs/gui-bridge.md`). |
| `img/logo.svg` | The ice cube logo of the header (`branding/logos/09-eiswuerfel-warm-avatar.svg`, colours for the dark top bar). |
| `js/mock.js` | Mock backend with sample data for a normal browser. Not embedded. |

## Design without building Rust

Browsers do not load ES modules from `file://`, so serve the folder with
any static file server and open it, for example:

```powershell
cd ui
python -m http.server 8000     # or: npx serve, or any other static server
# then open http://localhost:8000/
```

Without the WebView2 bridge, `bridge.js` loads `mock.js`; all buttons work
on sample data in memory, nothing is saved. `?theme=dark` or `?theme=light`
forces a colour scheme; `?view=entry|add|confirm|new|compare` opens a dialog and
`?view=settings` the Settings view; `?state=update|locked|notriggers` shows
an update found, the update mode set by the environment, or no triggers.

## Rules the page must keep

- No inline `<script>`, no inline `style` attributes or `on…` handlers, no
  remote URLs: the Content-Security-Policy of the window forbids them, and a
  unit test checks the embedded files.
- Text from the state goes into the DOM as text (`textContent`), never as
  markup.
- The page only shows state and sends intents; every check that matters is
  repeated on the Rust side.
- A new file must be added to `src/gui/assets.rs` to be embedded.

## Checking the real window

`kaltstellen gui --screenshot out.png --view settings --theme dark` renders the
real window with the current configuration (use a test configuration via
`KALTSTELLEN_CONFIG_DIR` and `KALTSTELLEN_DATA_DIR`) and saves a PNG.
