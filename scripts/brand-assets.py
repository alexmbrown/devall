"""Builds the /dev/all brand assets from the <D/A> mark artwork.

    python scripts/brand-assets.py
    cd apps/desktop && npx tauri icon src-tauri/icons/app-icon.png -o src-tauri/icons
    (then delete the generated icons/android and icons/ios folders — desktop only)

Traces the white <D/A> mark in scripts/brand/mark-source.jpg (sub-pixel contours, simplified to
polygons) and writes:
- apps/desktop/src/components/brandMark.ts  the mark as one even-odd SVG path plus the icon layout;
  the app draws the logo and repaints the window icon in the accent colour from it.
- apps/desktop/public/brand/icon.svg  the icon in the default colours (indigo tile, white mark).
- apps/desktop/src-tauri/icons/app-icon.png  1024px render of the same, for `tauri icon`.

Needs Pillow, numpy and scikit-image.
"""

import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw
from skimage import measure

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = ROOT / "apps" / "desktop"
SOURCE = ROOT / "scripts" / "brand" / "mark-source.jpg"
# Where the mark is in the source image, and the threshold between the white mark and its tile.
SOURCE_CROP = (595, 400, 1335, 790)
LEVEL, TOLERANCE = 164, 0.45
# Letter body (without the slash ends) in source pixels: centred vertically on the tile.
SOURCE_BODY_Y = (467, 650)

ACCENT, MARK_COLOUR = "#4f46e5", "#ffffff"  # tokens.css --accent (light)
SIZE, INSET, RADIUS = 1024, 64, 180  # icon canvas, tile inset, tile corner radius
MARK_WIDTH = 782  # mark width on the 896px tile (as in the source: ~87% of the tile)
# The A's crossbar stops short of the slash (like the D's stem stops short of the "<"): the gap and
# the rounding of its free end, in source pixels.
BAR_GAP, BAR_RADIUS = 16, 28

Polygon = list[tuple[float, float]]


def bbox(p: Polygon) -> tuple[float, float, float, float]:
    return min(x for x, _ in p), min(y for _, y in p), max(x for x, _ in p), max(y for _, y in p)


def row_gap(left: Polygon, right: Polygon) -> int:
    """Smallest horizontal gap between two shapes over the rows they share (whole pixels)."""
    x0 = int(min(bbox(left)[0], bbox(right)[0])) - 2
    y0 = int(min(bbox(left)[1], bbox(right)[1])) - 2
    w = int(max(bbox(left)[2], bbox(right)[2])) - x0 + 4
    h = int(max(bbox(left)[3], bbox(right)[3])) - y0 + 4

    def raster(p: Polygon) -> np.ndarray:
        m = Image.new("1", (w, h), 0)
        ImageDraw.Draw(m).polygon([(x - x0, y - y0) for x, y in p], fill=1)
        return np.asarray(m)

    a, b = raster(left), raster(right)
    gaps = [np.nonzero(b[r])[0].min() - np.nonzero(a[r])[0].max() for r in range(h) if a[r].any() and b[r].any()]
    return min(gaps)


def reshape(polys: list[Polygon]) -> list[Polygon]:
    """The source has a separate "/" between D and A. Drop it and let the A's long left leg be the
    slash: extend both legs of the A along their own lines up to where the slash's top was (the
    apex gets taller and narrower), then move the "A>" left so it sits as close to the D as the
    slash did."""
    by_left = sorted(polys, key=lambda p: bbox(p)[0])
    d = by_left[0]  # "<D" (leftmost)
    a = max(polys, key=lambda p: bbox(p)[2])  # "A>" (rightmost)
    hole = next(p for p in polys if p is not a and bbox(p)[0] > bbox(a)[0] and bbox(p)[2] < bbox(a)[2])  # the A's counter
    slash = next(p for p in polys if p is not d and p is not a and p is not hole)
    top = bbox(slash)[1]

    apex_y = bbox(a)[1]
    # The A's apex: points at the top within a stroke's width of the topmost one (the ">" reaches the
    # same height further right).
    first = min((i for i, (_, y) in enumerate(a) if y - apex_y < 3), key=lambda i: a[i][0])
    apex = [i for i, (x, y) in enumerate(a) if y - apex_y < 3 and x - a[first][0] < 60]
    left_i, right_i = min(apex, key=lambda i: a[i][0]), max(apex, key=lambda i: a[i][0])
    foot = min((p for p in a if p[1] > bbox(a)[3] - 3), key=lambda p: p[0])  # bottom of the left leg
    left_run = (a[left_i][0] - foot[0]) / (foot[1] - a[left_i][1])  # x per unit of rise, left leg
    nxt = a[(right_i + 1) % len(a)]  # next vertex down the right leg
    right_run = (nxt[0] - a[right_i][0]) / (nxt[1] - a[right_i][1])
    tall = list(a)
    for i in apex:
        x, y = a[i]
        tall[i] = (x + (y - top) * left_run, top) if a[i][0] <= (a[left_i][0] + a[right_i][0]) / 2 else (x - (y - top) * right_run, top)

    dx = row_gap(d, tall) - row_gap(d, slash)  # keep the D→slash spacing
    a_shapes = detach_crossbar([(x - dx, y) for x, y in tall], [(x - dx, y) for x, y in hole])
    return [d, *a_shapes]


def detach_crossbar(a: Polygon, hole: Polygon, ss: int = 8) -> list[Polygon]:
    """Cuts the A's crossbar loose from the slash (its left leg): the bar keeps its join with the
    right leg, ends `BAR_GAP` short of the slash with an edge parallel to it, and its free end is
    rounded on top — the way the D's stem ends short of the "<". Works on a raster (`ss`× the
    source) and traces the result again."""
    top = min(hole, key=lambda p: p[1])
    bl, br = min(hole, key=lambda p: p[0]), max(hole, key=lambda p: p[0])
    bar_top = max(y for _, y in hole)
    left_inner = lambda y: bl[0] + (bar_top - y) * (top[0] - bl[0]) / (bar_top - top[1])  # noqa: E731
    right_inner = lambda y: br[0] - (bar_top - y) * (br[0] - top[0]) / (bar_top - top[1])  # noqa: E731
    bar_bottom = min(y for x, y in a if bar_top + 20 < y < bar_top + 45 and left_inner(y) - 3 < x < right_inner(y) + 3)

    x0, y0 = bbox(a)[0] - 5, bbox(a)[1] - 5
    w, h = int((bbox(a)[2] - x0 + 10) * ss), int((bbox(a)[3] - y0 + 10) * ss)

    def raster(p: Polygon) -> np.ndarray:
        m = Image.new("1", (w, h), 0)
        ImageDraw.Draw(m).polygon([((x - x0) * ss, (y - y0) * ss) for x, y in p], fill=1)
        return np.asarray(m)

    shape = raster(a) ^ raster(hole)
    t, b = bar_top - 0.6, bar_bottom + 0.6
    shape &= ~raster([(left_inner(t), t), (right_inner(t), t), (right_inner(b), b), (left_inner(b), b)])  # old bar
    end = lambda y: left_inner(y) + BAR_GAP  # noqa: E731
    bar = fillet([(end(bar_top), bar_top), (right_inner(bar_top) + 3, bar_top), (right_inner(bar_bottom) + 3, bar_bottom), (end(bar_bottom), bar_bottom)], [BAR_RADIUS, 0.01, 0.01, 0.01])
    shape |= raster(bar)
    contours = [c for c in measure.find_contours(shape.astype(float), 0.5) if len(c) >= 40]
    return [[(x / ss + x0, y / ss + y0) for y, x in measure.approximate_polygon(c, tolerance=TOLERANCE * ss)] for c in contours]


def fillet(corners: Polygon, radii: list[float], segments: int = 10) -> Polygon:
    """A polygon through `corners` with each corner rounded by its radius (arcs as short segments)."""
    out: Polygon = []
    n = len(corners)
    for i in range(n):
        p, prev, nxt = np.array(corners[i]), np.array(corners[i - 1]), np.array(corners[(i + 1) % n])
        u, v = (prev - p) / np.linalg.norm(prev - p), (nxt - p) / np.linalg.norm(nxt - p)
        angle = math.acos(float(np.clip(u @ v, -1, 1)))
        start, end = p + u * radii[i] / math.tan(angle / 2), p + v * radii[i] / math.tan(angle / 2)
        centre = p + (u + v) / np.linalg.norm(u + v) * radii[i] / math.sin(angle / 2)
        a0, a1 = math.atan2(*(start - centre)[::-1]), math.atan2(*(end - centre)[::-1])
        sweep = (a1 - a0 + math.pi) % (2 * math.pi) - math.pi
        out += [tuple(centre + radii[i] * np.array([math.cos(a0 + sweep * k / segments), math.sin(a0 + sweep * k / segments)])) for k in range(segments + 1)]
    return out


def straighten(p: Polygon, slack: float = 1.6) -> Polygon:
    """Snaps edges that are almost vertical or horizontal (JPEG noise in the trace) to exact ones."""
    q = list(p)
    for _ in range(2):
        for i in range(len(q)):
            (x1, y1), (x2, y2) = q[i], q[(i + 1) % len(q)]
            if abs(x2 - x1) <= slack and abs(y2 - y1) > 8:
                m = (x1 + x2) / 2
                q[i], q[(i + 1) % len(q)] = (m, y1), (m, y2)
            elif abs(y2 - y1) <= slack and abs(x2 - x1) > 8:
                m = (y1 + y2) / 2
                q[i], q[(i + 1) % len(q)] = (x1, m), (x2, m)
    return q


def trace() -> tuple[list[Polygon], tuple[float, float], float]:
    """The mark's outlines (outer shapes and holes) relative to its bounding box, its size, and its top in the source."""
    x0, y0, x1, y1 = SOURCE_CROP
    gray = np.asarray(Image.open(SOURCE).convert("L"), dtype=float)[y0:y1, x0:x1]
    contours = [c for c in measure.find_contours(gray, LEVEL) if len(c) >= 40]
    polys = reshape([straighten([(float(x) + x0, float(y) + y0) for y, x in measure.approximate_polygon(c, tolerance=TOLERANCE)]) for c in contours])
    min_x = min(x for p in polys for x, _ in p)
    min_y = min(y for p in polys for _, y in p)
    max_x = max(x for p in polys for x, _ in p)
    max_y = max(y for p in polys for _, y in p)
    rel = [[(x - min_x, y - min_y) for x, y in p] for p in polys]
    return rel, (max_x - min_x, max_y - min_y), min_y


def svg_path(polys: list[Polygon]) -> str:
    return " ".join("M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in p) + " Z" for p in polys)


def main() -> None:
    polys, (w, h), top = trace()
    scale = MARK_WIDTH / w
    body_mid = ((SOURCE_BODY_Y[0] + SOURCE_BODY_Y[1]) / 2 - top) * scale
    mark_x, mark_y = (SIZE - MARK_WIDTH) / 2, SIZE / 2 - body_mid
    path = svg_path(polys)

    ts = DESKTOP / "src" / "components" / "brandMark.ts"
    ts.write_text(
        "// Generated by scripts/brand-assets.py from scripts/brand/mark-source.jpg — don't edit by hand.\n\n"
        "/** The <D/A> mark: one even-odd path (outer shapes and holes) in a `width` × `height` box. */\n"
        f"export const MARK = {{ width: {w:.1f}, height: {h:.1f}, path: '{path}' }};\n\n"
        "/** The app icon on a `size` canvas: the rounded tile, and the mark's offset and scale on it. */\n"
        f"export const ICON = {{ size: {SIZE}, inset: {INSET}, radius: {RADIUS}, markX: {mark_x:.1f}, markY: {mark_y:.1f}, markScale: {scale:.4f} }};\n",
        encoding="utf-8",
        newline="\n",
    )

    tile = f'<rect x="{INSET}" y="{INSET}" width="{SIZE - 2 * INSET}" height="{SIZE - 2 * INSET}" rx="{RADIUS}" fill="{ACCENT}"/>'
    mark = f'<path transform="translate({mark_x:.1f} {mark_y:.1f}) scale({scale:.4f})" fill="{MARK_COLOUR}" fill-rule="evenodd" d="{path}"/>'
    (DESKTOP / "public" / "brand" / "icon.svg").write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}">{tile}{mark}</svg>\n', encoding="utf-8", newline="\n"
    )

    # 4x supersampled raster of the same, for the platform icon files.
    ss = 4
    big = SIZE * ss
    tile_mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle((INSET * ss, INSET * ss, (SIZE - INSET) * ss, (SIZE - INSET) * ss), radius=RADIUS * ss, fill=255)
    mark_mask = np.zeros((big, big), dtype=bool)
    for p in polys:  # even-odd: every outline toggles coverage
        layer = Image.new("1", (big, big), 0)
        ImageDraw.Draw(layer).polygon([((mark_x + x * scale) * ss, (mark_y + y * scale) * ss) for x, y in p], fill=1)
        mark_mask ^= np.asarray(layer)
    icon = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    icon.paste(Image.new("RGBA", (big, big), ACCENT), (0, 0), tile_mask)
    icon.paste(Image.new("RGBA", (big, big), MARK_COLOUR), (0, 0), Image.fromarray(mark_mask))
    icon.resize((SIZE, SIZE), Image.LANCZOS).save(DESKTOP / "src-tauri" / "icons" / "app-icon.png")
    print(f"mark {w:.0f}x{h:.0f} ({sum(len(p) for p in polys)} points); wrote brandMark.ts, icon.svg, app-icon.png")


if __name__ == "__main__":
    main()
