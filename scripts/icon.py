"""MP3deck's icon: a waveform on the flat ground, played in ink, the rest dim, cut by the orange playhead.

Drawn at 4x on Apple's 1024 icon grid (824 pt squircle at 100 pt inset), then downsampled.
Usage: uv run --with pillow --with numpy python scripts/icon.py assets/icon.png
"""

import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

S = 4
N = 1024 * S

GROUND = (0xEF, 0xEE, 0xE9)
INK = (0x12, 0x12, 0x12)
DIM = (0xC9, 0xC7, 0xBE)
LIVE = (0xEE, 0x4F, 0x17)
RIM = (0xB8, 0xB6, 0xAE)

# Bar heights as fractions of the tallest: a build, a break, a drop.
LEVELS = [0.34, 0.52, 0.46, 0.70, 0.62, 0.88, 1.00, 0.80, 0.30, 0.44, 0.92, 0.84, 0.66]
PLAYED = 7


def px(v):
    return int(round(v * S))


def squircle_mask(size, inset, exponent=5.0):
    """Superellipse close to Apple's continuous-corner icon shape."""
    half = (size - 2 * inset) / 2
    c = size / 2
    ys, xs = np.mgrid[0:size, 0:size].astype(np.float32) + 0.5
    d = np.abs((xs - c) / half) ** exponent + np.abs((ys - c) / half) ** exponent
    return (d <= 1.0).astype(np.float32)


def main(out):
    mask = squircle_mask(N, px(100))
    face = Image.new("RGB", (N, N), GROUND)
    draw = ImageDraw.Draw(face)

    # Bars centred on the icon, mirrored about the middle line, as on the deck.
    bar, gap = 34, 18
    tallest = 430
    left = 512 - (len(LEVELS) * bar + (len(LEVELS) - 1) * gap) / 2
    for k, level in enumerate(LEVELS):
        x = left + k * (bar + gap)
        h = tallest * level
        color = INK if k < PLAYED else DIM
        draw.rectangle([px(x), px(512 - h / 2), px(x + bar) - 1, px(512 + h / 2) - 1], fill=color)

    # The playhead, in the gap after the last played bar, a little taller than the tallest bar.
    x = left + PLAYED * (bar + gap) - gap / 2
    draw.rectangle([px(x - 9), px(512 - tallest / 2 - 50), px(x + 9) - 1, px(512 + tallest / 2 + 50) - 1], fill=LIVE)

    # Soft shadow under the tile, per Apple's icon template.
    alpha = Image.fromarray((mask * 255).astype(np.uint8), "L")
    shadow_alpha = alpha.filter(ImageFilter.GaussianBlur(px(14))).point(lambda v: int(v * 0.32))
    shadow = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 255), (0, px(10)), shadow_alpha)

    tile = face.convert("RGBA")
    tile.putalpha(alpha)

    # Thin darker rim so the light tile holds its edge on white.
    rim = alpha.filter(ImageFilter.MinFilter(px(2) * 2 + 1))
    edge = Image.fromarray(np.clip(np.asarray(alpha, np.int16) - np.asarray(rim, np.int16), 0, 255).astype(np.uint8), "L")
    rim_layer = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    rim_layer.paste((*RIM, 255), (0, 0), edge.point(lambda v: int(v * 0.6)))

    icon = Image.alpha_composite(shadow, tile)
    icon = Image.alpha_composite(icon, rim_layer)
    icon.resize((1024, 1024), Image.LANCZOS).save(out)


if __name__ == "__main__":
    main(sys.argv[1])
