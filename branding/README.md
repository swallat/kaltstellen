# kaltstellen brand kit

Direction: variant A, two colors in the symbol. Mark: a program window with icicles and one drop. Frozen, not gone: the drop says the ice melts again.
Wordmark: lowercase `kaltstellen`, drawn from lines and circular arcs (stroke 10, round caps), no font file.

## Colors
The symbol uses three colors on any surface: window, ice, and an orange accent for the title-bar dots and the drop.
| Name | HEX | Use |
|---|---|---|
| Indigo | `#3322B8` | window on light surfaces |
| Violet | `#7A66FF` | window on dark surfaces |
| Mint | `#7FF7DC` | icicles, drop, title-bar dots on dark; on light use `#0FA88C` |
| Night | `#0F0C33` | dark surface, app-icon tile |
| Orange | `#FF7A1A` (light: `#E8650A`) | title-bar dots and drop in the symbol; UI primary action; error window in the tray |
| Ink / Paper | `#1A1260` / `#ECEBFF` | wordmark on light / on dark |

UI semantics: Mint = frozen / active, Orange = action, stop and error, Indigo/Violet = brand, selection, Start. Rose (`#FF6B8A` dark, `#C21F4B` light) = delete and protected only. State is never shown by color alone: every action has an icon (pause, stop, play, dash) and the tray states differ in shape.

## Tray icon (one file per state for light and dark taskbar)
The window color tells the state, the icicles are always mint, the title-bar dots are orange. Icicles appear in every state. All colors are mid-tones that pass 3:1 on both `#F3F3F3` and `#202020`.
| State | Window | Icicles | Shape |
|---|---|---|---|
| neutral | Gray `#7D8499` (3.4 / 4.4), orange dots `#FF7A1A` | Mint `#0A9A82` (3.2 / 4.6) | short stubs, nothing paused |
| active | Violet `#6A55F5` (4.5 / 3.3), orange dots | Mint `#0A9A82` | long icicles, programs frozen or stopped |
| error | Orange `#E65F00` (3.2 / 4.7), dots Violet | Mint `#0A9A82` | smaller window, two icicles, Violet `#6A55F5` warning triangle with "!" |
Pixel-hinted masters for 16, 20 and 24 px (`tray-*-16/20/24.svg`); 32 px is the 16 master doubled, 48 px the 24 master doubled. `.ico` per state holds 16, 20, 24, 32, 48.

## Files
- `logo/`: symbol (light, dark, black, white), horizontal and stacked lockups.
- `wortmarke/`: wordmark in indigo, paper, white, black.
- `tray/`: `svg/` pixel-hinted masters, `png/`, `ico/` per state (used by `build.rs`).
- `app/`: `kaltstellen.ico` (16 to 256, used by `build.rs`), SVG, PNGs. 16 to 32 px use a simplified glyph.
- `social/social-preview.png` (1280 x 640).
- `archive/`: the previous ice cube branding, kept for history.
- UI tokens live in `ui/css/tokens.css`, the header logo in `ui/img/logo.svg`.

## Usage rules
- Clear space: at least the height of the window's title bar (about 1/6 of the symbol height) on all sides.
- Minimum size: symbol 24 px (full detail), 16 px only as the tray or app-icon glyph; lockup at least 120 px wide.
- Light surfaces: Indigo window, Mint `#0FA88C` ice, Orange `#E8650A` dots and drop. Dark surfaces: Violet window, Mint `#7FF7DC` ice, Orange `#FF7A1A` dots and drop. The pane is always transparent.
- One color: use `symbol-schwarz`, `symbol-weiss` or the wordmark in one color. Without the drop below 24 px.
- Do not: recolor the drop to red, add shadows or gradients, rotate or crack the window, put the symbol on a busy photo, redraw the wordmark with a font.

## Contrast (WCAG 2.1, computed)
Dark UI: text 15.9, muted 7.9, violet text 8.6, mint 14.0, orange text 9.2, orange button 7.4, rose 6.7. Light UI: text 17.6, muted 6.9, indigo 10.2, mint text 5.5, orange text 5.2, orange button 7.4, rose 5.8. `--faint` (3.7 dark, 3.3 light) is for disabled and decorative text only.

## Honest limits
- No trademark search, no reverse image search. A text search found no freeze tool with a window-and-icicle mark; the visual check against others was not possible.
- Tray icons were checked in a browser on simulated taskbars, not on a real Windows taskbar. At 20 and 24 px the error badge cuts a small notch into the window; check on a 125% and 150% display.
- The wordmark is stroke based. Expand strokes to outlines before sending it to a printer.
- Everything is drawn from primitives. The social preview PNG uses the system font stack (Segoe UI, falls back to Inter or system-ui); the PNG is the deliverable, the font is not shipped. Release under the repository license (MIT or Apache-2.0).
