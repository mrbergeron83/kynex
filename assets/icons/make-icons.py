"""Draws the Kynex app icon from the mark's geometry (docs/design/branding.md) and
writes kynex.ico (Windows) and PNGs (Linux, 16-512). Needs Pillow:

    python assets/icons/make-icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).parent
# The dark mark, caret in petrol: the static app icon.
HOUSING, RIM, CARET, CHEVRON = "#0f0e0c", "#33312e", "#77becf", "#e5e2df"
SS = 8  # supersampling factor
STROKE = 12


def draw(size: int) -> Image.Image:
    s = size * SS
    u = s / 100
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    detailed = size >= 20  # below that the rim doesn't read
    d.rounded_rectangle((0, 0, s - 1, s - 1), radius=20 * u, fill=HOUSING)
    if detailed:
        w = max(1.5 * u, SS)  # at least one device pixel
        d.rounded_rectangle((w / 2, w / 2, s - 1 - w / 2, s - 1 - w / 2), radius=19.25 * u, outline=RIM, width=round(w))

    def stroke(points, fill):
        """A polyline with round caps and joins."""
        pts = [(x * u, y * u) for x, y in points]
        d.line(pts, fill=fill, width=round(STROKE * u))
        r = STROKE * u / 2
        for x, y in pts:
            d.ellipse((x - r, y - r, x + r, y + r), fill=fill)

    stroke([(68, 26), (48, 50), (68, 74)], CHEVRON)  # the K's arms: a prompt chevron
    stroke([(32, 26), (32, 74)], CARET)  # its stem: the text caret
    return im.resize((size, size), Image.LANCZOS)


ico_sizes = [16, 20, 24, 32, 40, 48, 64, 256]
images = [draw(n) for n in ico_sizes]
images[-1].save(OUT / "kynex.ico", sizes=[(n, n) for n in ico_sizes], append_images=images[:-1])
for n in [16, 32, 48, 64, 128, 256, 512]:
    draw(n).save(OUT / f"kynex-{n}.png")
print("wrote", OUT / "kynex.ico")
