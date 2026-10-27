"""Draws the Kynex app icon from the mark's geometry (docs/design/branding.md) and
writes kynex.ico (Windows) and PNGs (Linux, 16-512). Needs Pillow:

    python assets/icons/make-icons.py
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).parent
# The dark mark, lamp lit amber: the static app icon.
HOUSING, RIM, KNOB, POINTER, LAMP = "#0f0e0c", "#33312e", "#77becf", "#22201e", "#f2af48"
SS = 8  # supersampling factor


def draw(size: int) -> Image.Image:
    s = size * SS
    u = s / 100
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    detailed = size >= 20  # below that the rim and pointer don't read
    d.rounded_rectangle((0, 0, s - 1, s - 1), radius=20 * u, fill=HOUSING)
    if detailed:
        w = max(1.5 * u, SS)  # at least one device pixel
        d.rounded_rectangle((w / 2, w / 2, s - 1 - w / 2, s - 1 - w / 2), radius=19.25 * u, outline=RIM, width=round(w))
    d.ellipse(((50 - 31) * u, (54 - 31) * u, (50 + 31) * u, (54 + 31) * u), fill=KNOB)
    if detailed:
        # The pointer: a 6x21 capsule from y 27 to 48, rotated 38 degrees about (50, 54).
        a = math.radians(38)
        cx, cy = 50, 54
        def rot(x, y):
            dx, dy = x - cx, y - cy
            return ((cx + dx * math.cos(a) - dy * math.sin(a)) * u, (cy + dx * math.sin(a) + dy * math.cos(a)) * u)
        (x1, y1), (x2, y2) = rot(50, 30), rot(50, 45)
        d.line((x1, y1, x2, y2), fill=POINTER, width=round(6 * u))
        for x, y in ((x1, y1), (x2, y2)):
            d.ellipse((x - 3 * u, y - 3 * u, x + 3 * u, y + 3 * u), fill=POINTER)
    d.ellipse(((21 - 6) * u, (20 - 6) * u, (21 + 6) * u, (20 + 6) * u), fill=LAMP)
    return im.resize((size, size), Image.LANCZOS)


ico_sizes = [16, 20, 24, 32, 40, 48, 64, 256]
images = [draw(n) for n in ico_sizes]
images[-1].save(OUT / "kynex.ico", sizes=[(n, n) for n in ico_sizes], append_images=images[:-1])
for n in [16, 32, 48, 64, 128, 256, 512]:
    draw(n).save(OUT / f"kynex-{n}.png")
print("wrote", OUT / "kynex.ico")
