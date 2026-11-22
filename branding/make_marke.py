"""Write the kaltstellen wordmark (SVG) and the GitHub social preview (SVG and PNG).

Run from anywhere:  python branding/make_marke.py

No dependencies beyond the Python standard library. The letters are drawn
from lines and circular arcs with one stroke width, so no font is involved.
"""
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "tray"))
from make_icons import png  # noqa: E402  (shared PNG writer)

COLD, ORANGE, ORANGE_BRIGHT = "#1593AF", "#E8780C", "#FF8A1F"
INK_LIGHT, INK_DARK, NIGHT = "#10202E", "#E8F0F7", "#0E1620"
STROKE = 3.5   # a little lighter than the logo (4 on its 32 grid), to keep a and e open
GAP = 7.0      # distance between the centre lines of neighbouring letters

# Letters as centre lines: ("L", x1, y1, x2, y2) or ("A", cx, cy, r, from, to)
# with angles in degrees, y pointing down, so a growing angle runs clockwise.
# Baseline 28, x-height 16, ascender 6. Third element of a letter: its width.
LETTERS = {
    "k": ([("L", 0, 6, 0, 28), ("L", 0, 22, 8, 16), ("L", 0, 22, 8, 28)], 8),
    "a": ([("A", 6, 22, 6, 0, 360), ("L", 12, 16, 12, 28)], 12),
    "l": ([("L", 0, 6, 0, 28)], 0),
    "t": ([("L", 4, 9, 4, 23), ("A", 9, 23, 5, 180, 90), ("L", 0, 16, 9, 16)], 9),
    "s": ([("A", 3.2, 19, 3, -30, -270), ("A", 3.2, 25, 3, -90, 150)], 6.4),
    "e": ([("L", 0, 22, 12, 22), ("A", 6, 22, 6, 0, -315)], 12),
    "n": ([("L", 0, 16, 0, 28), ("A", 5.5, 21.5, 5.5, 180, 360), ("L", 11, 21.5, 11, 28)], 11),
}
# The logo (A2, ice cube) on its 32 grid.
HEX = [(16, 3.5), (26.8, 9.75), (26.8, 22.25), (16, 28.5), (5.2, 22.25), (5.2, 9.75)]
RIGHT = [(16, 16), (26.8, 9.75), (26.8, 22.25), (16, 28.5)]
BARS = [(12.2, 12.5, 21.0), (19.8, 12.5, 21.0)]
BAR_W = 3.2


def word(text="kaltstellen", warm_k=True):
    """Lay the letters out. Returns (primitives with role, total width)."""
    out, x = [], 0.0
    for i, ch in enumerate(text):
        prims, w = LETTERS[ch]
        for j, p in enumerate(prims):
            role = "ink"
            if i == 0 and ch == "k" and warm_k and j == 1:
                # the first k echoes the logo: its upper arm is warm and set off from the stem
                p, role = ("L", 3.7, 19.2, 8, 16), "warm"
            q = (p[0], p[1] + x, p[2], p[3] + x, p[4]) if p[0] == "L" else (p[0], p[1] + x, p[2], p[3], p[4], p[5])
            out.append((q, role))
        x += w + GAP
    return out, x - GAP


def pt(cx, cy, r, deg):
    return cx + r * math.cos(math.radians(deg)), cy + r * math.sin(math.radians(deg))


def svg_path(p):
    if p[0] == "L":
        return f"M{p[1]:g} {p[2]:g}L{p[3]:g} {p[4]:g}"
    _, cx, cy, r, a0, a1 = p
    if abs(a1 - a0) >= 360:
        return f"M{cx + r:g} {cy:g}A{r:g} {r:g} 0 1 1 {cx - r:g} {cy:g}A{r:g} {r:g} 0 1 1 {cx + r:g} {cy:g}"
    (x0, y0), (x1, y1) = pt(cx, cy, r, a0), pt(cx, cy, r, a1)
    return f"M{x0:.2f} {y0:.2f}A{r:g} {r:g} 0 {int(abs(a1 - a0) > 180)} {int(a1 > a0)} {x1:.2f} {y1:.2f}"


def word_svg(prims, ink, warm):
    groups = {"ink": [], "warm": []}
    for p, role in prims:
        groups[role].append(svg_path(p))
    s = f'<g fill="none" stroke-width="{STROKE:g}" stroke-linecap="round" stroke-linejoin="round">'
    s += f'<path d="{"".join(groups["ink"])}" stroke="{ink}"/>'
    if groups["warm"]:
        s += f'<path d="{"".join(groups["warm"])}" stroke="{warm}"/>'
    return s + "</g>"


def _pts(p):
    return " ".join(f"{x:g} {y:g}" for x, y in p)


def logo_svg(warm=ORANGE):
    return (f'<path d="M{_pts(HEX)}Z" fill="{COLD}"/>'
            f'<path d="M{_pts(RIGHT)}Z" fill="{warm}"/>'
            '<path d="M5.2 9.75 16 16l10.8-6.25M16 16v12.5" fill="none" stroke="#fff" stroke-opacity=".35" stroke-width="1.2" stroke-linejoin="round"/>'
            f'<path d="M12.2 12.5v8.5M19.8 12.5v8.5" fill="none" stroke="{NIGHT}" stroke-width="{BAR_W:g}" stroke-linecap="round"/>')


def write_wordmarks():
    out = HERE / "wortmarke"
    out.mkdir(exist_ok=True)
    for name, ink, warm in (("hell", INK_LIGHT, ORANGE), ("dunkel", INK_DARK, ORANGE_BRIGHT)):
        prims, w = word(warm_k=False)
        (out / f"wortmarke-{name}.svg").write_text(
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="-4 2 {w + 8:g} 30" role="img" aria-label="kaltstellen">\n'
            f'  {word_svg(prims, ink, warm)}\n</svg>\n', encoding="utf-8", newline="\n")
        prims, w = word(warm_k=False)
        (out / f"logo-wortmarke-{name}.svg").write_text(
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w + 40:g} 32" role="img" aria-label="kaltstellen">\n'
            f'  {logo_svg()}\n  <g transform="translate(34 -1)">{word_svg(prims, ink, warm)}</g>\n</svg>\n',
            encoding="utf-8", newline="\n")


# ---------- social preview ----------
W, H = 1280, 640
LOGO_SCALE, WORD_SCALE, SPACE = 8.0, 4.2, 56.0


def layout():
    prims, w = word(warm_k=False)
    total = 26.8 * LOGO_SCALE - 5.2 * LOGO_SCALE + SPACE + w * WORD_SCALE   # logo ink spans x 5.2..26.8 of its grid
    lx = (W - total) / 2 - 5.2 * LOGO_SCALE
    ly = H / 2 - 16 * LOGO_SCALE
    wx = lx + 26.8 * LOGO_SCALE + SPACE
    wy = H / 2 - 17 * WORD_SCALE   # letters span y 4..30, centre 17
    return prims, lx, ly, wx, wy


def social_svg():
    prims, lx, ly, wx, wy = layout()
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="kaltstellen">\n'
            f'  <rect width="{W}" height="{H}" fill="{NIGHT}"/>\n'
            f'  <g transform="translate({lx:.1f} {ly:.1f}) scale({LOGO_SCALE:g})">{logo_svg(ORANGE_BRIGHT)}</g>\n'
            f'  <g transform="translate({wx:.1f} {wy:.1f}) scale({WORD_SCALE:g})">{word_svg(prims, INK_DARK, ORANGE_BRIGHT)}</g>\n</svg>\n')


def rgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def social_png():
    """Rasterise the same picture: every stroke is a chain of round-capped segments."""
    bg = rgb(NIGHT)
    px = [[bg] * W for _ in range(H)]

    def seg(ax, ay, bx, by, r, col):
        dx, dy = bx - ax, by - ay
        ll = dx * dx + dy * dy
        for y in range(max(0, int(min(ay, by) - r - 1)), min(H, int(max(ay, by) + r + 2))):
            row = px[y]
            for x in range(max(0, int(min(ax, bx) - r - 1)), min(W, int(max(ax, bx) + r + 2))):
                qx, qy = x + 0.5 - ax, y + 0.5 - ay
                t = min(1.0, max(0.0, (qx * dx + qy * dy) / ll)) if ll else 0.0
                cov = r + 0.5 - math.hypot(qx - t * dx, qy - t * dy)   # one-pixel-wide soft edge
                if cov >= 1:
                    row[x] = col
                elif cov > 0:
                    o = row[x]
                    if o != col:
                        row[x] = tuple(round(o[i] + (col[i] - o[i]) * cov) for i in range(3))

    def fill_poly(pts, col):
        xs, ys = [p[0] for p in pts], [p[1] for p in pts]
        for y in range(int(min(ys)), int(max(ys)) + 1):
            for x in range(int(min(xs)), int(max(xs)) + 1):
                n = 0
                for j in range(4):
                    for i in range(4):
                        sx, sy = x + (i + .5) / 4, y + (j + .5) / 4
                        if all((bx - ax) * (sy - ay) - (by - ay) * (sx - ax) >= 0
                               for (ax, ay), (bx, by) in zip(pts, pts[1:] + pts[:1])):
                            n += 1
                if n:
                    o = px[y][x]
                    px[y][x] = tuple(round(o[k] + (col[k] - o[k]) * n / 16) for k in range(3))

    prims, lx, ly, wx, wy = layout()
    cold, orange, ink = rgb(COLD), rgb(ORANGE_BRIGHT), rgb(INK_DARK)
    fill_poly([(lx + x * LOGO_SCALE, ly + y * LOGO_SCALE) for x, y in HEX], cold)
    fill_poly([(lx + x * LOGO_SCALE, ly + y * LOGO_SCALE) for x, y in RIGHT], orange)
    pale = tuple(round(cold[i] + (255 - cold[i]) * 0.35) for i in range(3))
    for (x1, y1), (x2, y2) in (((5.2, 9.75), (16, 16)), ((16, 16), (26.8, 9.75)), ((16, 16), (16, 28.5))):
        d = math.hypot(x2 - x1, y2 - y1)
        ux, uy = (x2 - x1) / d * 0.6, (y2 - y1) / d * 0.6   # keep the round caps inside the outline
        a, b = (x1 + (ux if (x1, y1) != (16, 16) else 0), y1 + (uy if (x1, y1) != (16, 16) else 0)), (x2 - (ux if (x2, y2) != (16, 16) else 0), y2 - (uy if (x2, y2) != (16, 16) else 0))
        seg(lx + a[0] * LOGO_SCALE, ly + a[1] * LOGO_SCALE, lx + b[0] * LOGO_SCALE, ly + b[1] * LOGO_SCALE, 0.6 * LOGO_SCALE, pale)
    for bx, top, bot in BARS:
        seg(lx + bx * LOGO_SCALE, ly + (top + BAR_W / 2) * LOGO_SCALE, lx + bx * LOGO_SCALE,
            ly + (bot - BAR_W / 2) * LOGO_SCALE, BAR_W * LOGO_SCALE / 2, bg)
    r = STROKE * WORD_SCALE / 2
    for p, _ in prims:
        if p[0] == "L":
            pts = [(p[1], p[2]), (p[3], p[4])]
        else:
            _, cx, cy, rad, a0, a1 = p
            n = max(2, int(abs(a1 - a0) / 3))
            pts = [pt(cx, cy, rad, a0 + (a1 - a0) * i / n) for i in range(n + 1)]
        for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
            seg(wx + x1 * WORD_SCALE, wy + y1 * WORD_SCALE, wx + x2 * WORD_SCALE, wy + y2 * WORD_SCALE, r, ink)
    return png([p + (255,) for row in px for p in row], W, H)


def main():
    write_wordmarks()
    out = HERE / "social"
    out.mkdir(exist_ok=True)
    (out / "social-preview.svg").write_text(social_svg(), encoding="utf-8", newline="\n")
    (out / "social-preview.png").write_bytes(social_png())


if __name__ == "__main__":
    main()
