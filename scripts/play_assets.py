# /// script
# dependencies = ["pillow", "fonttools", "brotli"]
# ///
"""Google Play listing graphics: app/play/listing/{icon-512,feature-1024x500,phone-N}.png.

  uv run scripts/play_assets.py

The icon comes from site/icon.svg (square: Play rounds it). The phone screenshots come from
app/play/screens (1080x2280 app screens), framed at 1080x1920 (9:16) with a caption, because Play wants
the long side at most twice the short one. Those screens are the app's golden screens (app/test/screens,
`flutter test test/screens_test.dart --update-goldens`) and, for the WebView tabs, the server's pages
captured at the phone's size (393x749 CSS px at 2.75x) pasted above the frame-library/frame-map goldens.
"""
import io
import re
import subprocess
import tempfile
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "app/play/listing"
GREEN, WHITE, PALE = (47, 107, 79), (255, 255, 255), (214, 232, 222)
SHOTS = [  # app/play/screens/<name>.png (1080x2280 app screens), caption; Play shows at most 8
    ("home", "440,000 articles in your pocket"),
    ("meaning", "Search by meaning, not just keywords"),
    ("ask", "Offline AI chat, backed by facts"),
    ("map", "Offline maps with place search"),
    ("chat", "Chat with people nearby, no internet"),
    ("firstaid", "First aid, step by step"),
    ("books", "Classic books to read offline"),
    ("games", "22 games, no internet needed"),
]


def font(weight: str, size: int) -> ImageFont.FreeTypeFont:
    f = TTFont(ROOT / f"site/fonts/Inter-{weight}.woff2")
    f.flavor = None
    buf = io.BytesIO()
    f.save(buf)
    buf.seek(0)
    return ImageFont.truetype(buf, size)


def svg_png(size: int, square: bool) -> Image.Image:
    svg = (ROOT / "site/icon.svg").read_text()
    if square:
        svg = svg.replace(' rx="9"', "")
    with tempfile.NamedTemporaryFile(suffix=".svg") as t:
        t.write(svg.encode())
        t.flush()
        png = subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), t.name],
                             check=True, capture_output=True).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def house(size: int) -> Image.Image:
    """The icon's white house alone, on transparent."""
    svg = re.sub(r'<rect width="32"[^>]*/>', "", (ROOT / "site/icon.svg").read_text())
    with tempfile.NamedTemporaryFile(suffix=".svg") as t:
        t.write(svg.encode())
        t.flush()
        png = subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), t.name],
                             check=True, capture_output=True).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def wrap(d: ImageDraw.ImageDraw, text: str, f, width: int) -> list[str]:
    lines, cur = [], ""
    for w in text.split():
        t = (cur + " " + w).strip()
        if d.textlength(t, font=f) <= width or not cur:
            cur = t
        else:
            lines.append(cur)
            cur = w
    return lines + [cur]


def balanced(d: ImageDraw.ImageDraw, text: str, f, width: int) -> list[str]:
    """One line if it fits, else two lines of about equal width (no lone word on the second line)."""
    if d.textlength(text, font=f) <= width:
        return [text]
    w = text.split()
    splits = [(" ".join(w[:i]), " ".join(w[i:])) for i in range(1, len(w))]
    return list(min(splits, key=lambda s: max(d.textlength(x, font=f) for x in s)))


def rounded(im: Image.Image, r: int) -> Image.Image:
    m = Image.new("L", im.size, 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, *im.size), r, fill=255)
    out = im.convert("RGBA")
    out.putalpha(m)
    return out


def phone(src: Path, caption: str) -> Image.Image:
    W, H = 1080, 1920
    im = Image.new("RGB", (W, H), GREEN)
    d = ImageDraw.Draw(im)
    f = font("Bold", 64)
    lines = balanced(d, caption, f, W - 160)
    y = 90 + (2 - len(lines)) * 40  # room for two lines on every shot, so the phones line up
    for line in lines:
        d.text((W // 2, y), line, font=f, fill=WHITE, anchor="ma")
        y += 80
    top = 90 + 2 * 80 + 50
    shot = Image.open(src).convert("RGB")
    h = H - top - 0
    w = round(shot.width * h / shot.height)
    shot = rounded(shot.resize((w, h), Image.LANCZOS), 44)
    x = (W - w) // 2
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((x, top + 10, x + w, top + h + 60), 44, fill=(0, 0, 0, 90))
    im.paste(shadow.filter(ImageFilter.GaussianBlur(24)), (0, 0), shadow.filter(ImageFilter.GaussianBlur(24)))
    im.paste(shot, (x, top), shot)
    return im


def feature() -> Image.Image:
    W, H = 1024, 500
    im = Image.new("RGB", (W, H), GREEN)
    d = ImageDraw.Draw(im)
    h = house(300)
    im.paste(h, (60, 90), h)
    d.text((400, 120), "Bothy", font=font("Bold", 120), fill=WHITE)
    y = 270
    for line in wrap(d, "The offline library: how-to, first aid, maps and books", font("SemiBold", 38), 600):
        d.text((404, y), line, font=font("SemiBold", 38), fill=PALE)
        y += 50
    return im


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    svg_png(512, square=True).convert("RGB").save(OUT / "icon-512.png")
    feature().save(OUT / "feature-1024x500.png")
    for i, (name, cap) in enumerate(SHOTS, 1):
        phone(ROOT / f"app/play/screens/{name}.png", cap).save(OUT / f"phone-{i}.png")
    for p in sorted(OUT.glob("*.png")):
        print(p.relative_to(ROOT), Image.open(p).size, f"{p.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
