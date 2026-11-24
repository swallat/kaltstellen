"""Draw the kaltstellen tray icons on the pixel grid and write PNG and ICO files.

Run from anywhere:  python branding/tray/make_icons.py

No dependencies beyond the Python standard library. Each size is drawn from
its own integer pixel geometry (not scaled down from one large image), so
straight edges land on pixel boundaries and stay sharp at 16 px.
"""
import pathlib
import struct
import zlib

HERE = pathlib.Path(__file__).resolve().parent
SIZES = (16, 20, 24, 32, 48, 256)
SS = 4  # supersampling per axis, for the diagonals and round corners

COLD = (0x15, 0x93, 0xAF)       # the ice cube
WARM = (0xE8, 0x78, 0x0C)       # the warm side: a profile is active
WARM_BRIGHT = (0xFF, 0x8A, 0x1F)
ERROR = (0xC8, 0x1E, 0x4A)
WHITE = (255, 255, 255)
DARK = (0x0E, 0x16, 0x20)

# The logo (A2, "Eiswuerfel") on its 32 grid. Points of the hexagon; the right
# face is the warm one.
HEX = [(16, 3.5), (26.8, 9.75), (26.8, 22.25), (16, 28.5), (5.2, 22.25), (5.2, 9.75)]
RIGHT = [(16, 16), (26.8, 9.75), (26.8, 22.25), (16, 28.5)]
EDGES = [((5.2, 9.75), (16, 16)), ((16, 16), (26.8, 9.75)), ((16, 16), (16, 28.5))]
BARS = ((12.2, 12.5, 21.0), (19.8, 12.5, 21.0))  # x, top, bottom of a pause bar's centre line

# Tray: the cube without a tile. States differ in colour and shape:
# neutral = cold cube with pause bars, aktiv = the right face turns warm,
# fehler = red cube with an exclamation mark instead of the bars.
STATES = ("neutral", "aktiv", "fehler")
# Per size: scale of the logo grid (1.0 = as drawn on 32) and bar width in px.
HINTS = {
    16: dict(scale=0.62, bar=2.0),
    20: dict(scale=0.78, bar=2.4),
    24: dict(scale=0.93, bar=2.8),
    32: dict(scale=1.2, bar=3.6),
    48: dict(scale=1.8, bar=5.2),
    256: dict(scale=9.6, bar=30.0),
    # only used by the program icon
    40: dict(scale=0.9, bar=3.2),
    64: dict(scale=1.45, bar=4.6),
}
APP_SIZES = (16, 20, 24, 32, 40, 48, 64, 256)
APP_TILE, APP_COLD = (0x12, 0x32, 0x4B), (0x2B, 0xB3, 0xD1)
APP_SCALE = {16: .5, 20: .62, 24: .74, 32: .95, 40: 1.2, 48: 1.4, 64: 1.85, 256: 7.4}
APP_RADIUS = {16: 3, 20: 4, 24: 5, 32: 7, 40: 9, 48: 10, 64: 14, 256: 56}
APP_MARGIN = {16: 0, 20: 0, 24: 1, 32: 1, 40: 2, 48: 2, 64: 3, 256: 8}


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def rrect(x0, y0, x1, y1, r):
    """Rounded rectangle as an inside test."""
    def inside(x, y):
        if not (x0 <= x <= x1 and y0 <= y <= y1):
            return False
        cx = min(max(x, x0 + r), x1 - r)
        cy = min(max(y, y0 + r), y1 - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
    return inside


def capsule(ax, ay, bx, by, r):
    """Line segment with round caps as an inside test."""
    dx, dy = bx - ax, by - ay
    ll = dx * dx + dy * dy

    def inside(x, y):
        t = ((x - ax) * dx + (y - ay) * dy) / ll if ll else 0.0
        t = min(1.0, max(0.0, t))
        return (x - ax - t * dx) ** 2 + (y - ay - t * dy) ** 2 <= r * r
    return inside


def polygon(pts):
    """Convex polygon (clockwise on screen) as an inside test."""
    def inside(x, y):
        for (ax, ay), (bx, by) in zip(pts, pts[1:] + pts[:1]):
            if (bx - ax) * (y - ay) - (by - ay) * (x - ax) < 0:
                return False
        return True
    return inside


def cube(n, scale, bar, cold, warm, bars_col, edges, error=False, ox=0.0, oy=0.0):
    """The logo as shapes for an n px canvas: grid point (16, 16) lands on the centre.
    warm=None draws the cube all cold; error=True swaps the pause bars for an exclamation mark."""
    c = n / 2

    def T(p):
        return (c + ox + (p[0] - 16) * scale, c + oy + (p[1] - 16.25) * scale)
    out = [(polygon([T(p) for p in HEX]), cold)]
    if warm:
        out.append((polygon([T(p) for p in RIGHT]), warm))
    if edges:
        pale = mix(cold, WHITE, 0.4)
        for a, b in EDGES:
            (ax, ay), (bx, by) = T(a), T(b)
            out.append((capsule(ax, ay, bx, by, max(0.5, 0.6 * scale)), pale))
    r = bar / 2
    if error:
        x, y0 = T((16, 0))[0], T((0, 10.5))[1]
        y1, yd = T((0, 18.0))[1], T((0, 23.0))[1]
        out.append((capsule(x, y0 + r, x, y1 - r, r), bars_col))
        out.append((capsule(x, yd, x, yd, r * 1.05), bars_col))
    else:
        for bx, top, bot in BARS:
            x, y0 = T((bx, top))
            y1 = T((bx, bot))[1]
            out.append((capsule(x, y0 + r, x, y1 - r, r), bars_col))
    return out


def shapes(state, n):
    """The shapes of one tray icon, back to front, as (inside test, colour)."""
    h = HINTS[n]
    if state == "fehler":
        return cube(n, h["scale"], h["bar"], ERROR, None, WHITE, n >= 32, error=True)
    return cube(n, h["scale"], h["bar"], COLD, WARM_BRIGHT if state == "aktiv" else None, DARK, n >= 32)


def app_shapes(n):
    """The program icon: the logo on a dark tile."""
    m = APP_MARGIN[n]
    out = [(rrect(m, m, n - m, n - m, APP_RADIUS[n]), APP_TILE)]
    out += cube(n, APP_SCALE[n], max(2.0, 3.6 * APP_SCALE[n]), APP_COLD, WARM_BRIGHT, DARK, n >= 40)
    return out


def render(state, n):
    """Return n*n RGBA tuples, row by row. State "app" is the program icon."""
    sh = app_shapes(n) if state == "app" else shapes(state, n)
    px = []
    for y in range(n):
        for x in range(n):
            r = g = b = a = 0
            for j in range(SS):
                for i in range(SS):
                    sx, sy = x + (i + 0.5) / SS, y + (j + 0.5) / SS
                    col = None
                    for inside, c in sh:
                        if inside(sx, sy):
                            col = c
                    if col:
                        r += col[0]; g += col[1]; b += col[2]; a += 1
            px.append((round(r / a), round(g / a), round(b / a), round(255 * a / (SS * SS))) if a else (0, 0, 0, 0))
    return px


def png(px, w, h):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    raw = b"".join(b"\x00" + bytes(c for p in px[y * w:(y + 1) * w] for c in p) for y in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def ico(images):
    """images: list of (size, png bytes). PNG-compressed entries, Windows Vista and later."""
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for n, data in images:
        entries += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        blobs += data
    return head + entries + blobs


def preview(rendered, bands=(("neutral", "aktiv", "fehler"), ("neutral", "aktiv", "fehler"))):
    """Contact sheet: every state enlarged 10x at 16 px, then all sizes at 1:1, on a light and a dark taskbar.
    """
    zoom, pad = 10, 12
    col_w = 16 * zoom + pad
    row_h = 16 * zoom + pad + max(SIZES[:-1]) + pad
    w = pad + len(bands[0]) * (col_w + sum(s + pad for s in SIZES[:-1]))
    h = 2 * (row_h + pad)
    canvas = [[(0xF3, 0xF3, 0xF3)] * w for _ in range(h // 2)] + [[(0x1C, 0x1C, 0x1C)] * w for _ in range(h // 2)]

    def blit(px, n, ox, oy, scale=1):
        for y in range(n):
            for x in range(n):
                r, g, b, a = px[y * n + x]
                if not a:
                    continue
                for yy in range(oy + y * scale, oy + (y + 1) * scale):
                    row = canvas[yy]
                    for xx in range(ox + x * scale, ox + (x + 1) * scale):
                        br, bg, bb = row[xx]
                        row[xx] = ((r * a + br * (255 - a)) // 255, (g * a + bg * (255 - a)) // 255, (b * a + bb * (255 - a)) // 255)

    for band in range(2):
        oy = band * (h // 2) + pad
        ox = pad
        for col, state in enumerate(bands[band]):
            src = rendered[state]
            blit(src[16], 16, ox, oy, zoom)
            x = ox + col_w
            for s in SIZES[:-1]:
                blit(src[s], s, x, oy)
                x += s + pad
            blit(src[24], 24, ox + col_w, oy + 64, 4)
            ox = x
    return png([p + (255,) for row in canvas for p in row], w, h)


def main():
    (HERE / "png").mkdir(exist_ok=True)
    (HERE / "ico").mkdir(exist_ok=True)
    rendered = {}
    for state in STATES:
        rendered[state] = {n: render(state, n) for n in SIZES}
        images = []
        for n in SIZES:
            data = png(rendered[state][n], n, n)
            (HERE / "png" / f"kaltstellen-{state}-{n}.png").write_bytes(data)
            images.append((n, data))
        (HERE / "ico" / f"kaltstellen-{state}.ico").write_bytes(ico(images))
    (HERE.parent / "vorschau" / "tray-pixel.png").write_bytes(preview(rendered))
    app = HERE.parent / "app"
    (app / "png").mkdir(parents=True, exist_ok=True)
    app_px = {n: render("app", n) for n in APP_SIZES}
    images = []
    for n in APP_SIZES:
        data = png(app_px[n], n, n)
        (app / "png" / f"kaltstellen-{n}.png").write_bytes(data)
        images.append((n, data))
    (app / "kaltstellen.ico").write_bytes(ico(images))
    rendered["app"] = app_px
    (HERE.parent / "vorschau" / "app-pixel.png").write_bytes(preview(rendered, (("app",), ("app",))))


if __name__ == "__main__":
    main()
