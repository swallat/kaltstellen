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

WHITE = (255, 255, 255)
DARK = (0x0E, 0x16, 0x20)
RAY_STRENGTH = 0.65  # how far the crystal rays go from tile colour towards glyph colour
# Per state: tile colour, glyph colour. "aktiv" and "fehler" differ in hue
# (orange against crimson), in brightness (2.4 : 1) and in glyph colour and shape.
STATES = {
    "neutral": ((0x2A, 0x8F, 0xD6), WHITE),  # nothing on ice
    "aktiv": ((0xFF, 0x8A, 0x1F), DARK),     # a profile is active
    "fehler": ((0xC8, 0x1E, 0x4A), WHITE),   # error
}
# Per size: tile margin, tile corner radius, stroke width, top of the K.
HINTS = {
    16: dict(margin=0, radius=3, stroke=2, top=3),
    20: dict(margin=0, radius=4, stroke=2, top=4),
    24: dict(margin=1, radius=5, stroke=3, top=5),
    32: dict(margin=1, radius=7, stroke=4, top=7),
    48: dict(margin=2, radius=10, stroke=6, top=10),
    256: dict(margin=8, radius=56, stroke=32, top=56),
    # only used by the program icon
    40: dict(margin=2, radius=9, stroke=5, top=9),
    64: dict(margin=3, radius=14, stroke=8, top=14),
}
# Program icon (for the .exe, shortcuts, the window): the logo on a dark tile.
APP_SIZES = (16, 20, 24, 32, 40, 48, 64, 256)
# From 32 px up the program icon has room for a slimmer, taller K than the tray tiles.
APP_HINTS = {
    32: dict(stroke=3, top=6), 40: dict(stroke=4, top=8), 48: dict(stroke=5, top=9),
    64: dict(stroke=6, top=12), 256: dict(stroke=24, top=46),
}
APP_TILE, APP_K, APP_WARM = (0x12, 0x32, 0x4B), (0xEA, 0xF4, 0xFB), (0xFF, 0x8A, 0x1F)


def app_shapes(n):
    """The program icon: light K with a warm upper arm and pale crystal rays on a dark tile."""
    h = dict(HINTS[n], **APP_HINTS.get(n, {}))
    m, w, top = h["margin"], h["stroke"], h["top"]
    out = [(rrect(m, m, n - m, n - m, h["radius"]), APP_TILE)]
    mid, bottom = n / 2, n - top
    reach = mid - top - w / 2
    ray, ray_r = round(reach * 0.7), max(0.6, w * 0.3)
    x0 = round((n - (ray + ray_r + reach + w / 2)) / 2 + ray + ray_r - w / 2)
    sx = x0 + w / 2
    pale = tuple(round(t + (g - t) * 0.5) for t, g in zip(APP_TILE, (0x2A, 0x8F, 0xD6)))
    out.append((capsule(sx, mid, sx - ray, mid - ray, ray_r), pale))
    out.append((capsule(sx, mid, sx - ray, mid + ray, ray_r), pale))
    # Below 40 px the warm arm stays attached; a gap would leave only a dot.
    gap = w + max(1.0, w * 0.35) if n >= 40 else 0.0
    out.append((capsule(sx + gap, mid - gap, sx + reach, mid - reach, w / 2), APP_WARM))
    out.append((capsule(sx, mid, sx + reach, mid + reach, w / 2), APP_K))
    out.append((rrect(x0, top, x0 + w, bottom, w * 0.3), APP_K))
    return out


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


def shapes(state, n):
    """The shapes of one icon, back to front, as (inside test, colour)."""
    h = HINTS[n]
    m, w, top = h["margin"], h["stroke"], h["top"]
    tile, WHITE = STATES[state]  # WHITE: this state's glyph colour
    out = [(rrect(m, m, n - m, n - m, h["radius"]), tile)]
    mid, bottom = n / 2, n - top
    if state == "fehler":
        # Exclamation mark instead of the K: a shape of its own, not only a colour.
        bw = w + (w % 2)
        x0 = mid - bw / 2
        split = bottom - bw - max(1, round(n / 12))
        out.append((rrect(x0, top, x0 + bw, split, bw * 0.3), WHITE))
        out.append((rrect(x0, bottom - bw, x0 + bw, bottom, bw / 2 if n >= 32 else bw * 0.3), WHITE))
        return out
    reach = mid - top - w / 2           # how far the arms run, at 45 degrees
    # The other half of the ice crystal: two short, thin, paler rays left of the stem.
    ray, ray_r = round(reach * 0.7), max(0.6, w * 0.3)
    width = ray + ray_r + reach + w / 2
    x0 = round((n - width) / 2 + ray + ray_r - w / 2)   # centre crystal and K in the tile
    sx = x0 + w / 2
    pale = tuple(round(t + (g - t) * RAY_STRENGTH) for t, g in zip(tile, WHITE))
    out.append((capsule(sx, mid, sx - ray, mid - ray, ray_r), pale))
    out.append((capsule(sx, mid, sx - ray, mid + ray, ray_r), pale))
    out.append((rrect(x0, top, x0 + w, bottom, w * 0.3), WHITE))
    out.append((capsule(sx, mid, sx + reach, mid + reach, w / 2), WHITE))
    # Active: the warm arm is set off from the stem by a gap. Below 32 px the
    # gap would leave only a dot, so there the colour alone carries the state.
    gap = w * 0.5 + max(1.0, n / 16) + w / 2 if state.startswith("aktiv") and n >= 32 else 0.0
    out.append((capsule(sx + gap, mid - gap, sx + reach, mid - reach, w / 2), WHITE))
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
