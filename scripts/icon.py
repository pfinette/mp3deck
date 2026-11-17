"""MP3deck's icon: a cream deck with stereo level meters on its screen and a play key with its lamp lit.

Drawn at 4x on Apple's 1024 icon grid (824 pt squircle at 100 pt inset), then downsampled.
Usage: uv run --with pillow --with numpy python scripts/icon.py assets/icon.png
"""

import math
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

S = 4
N = 1024 * S

FACE_TOP = (0xEC, 0xE9, 0xD7)
FACE_BOTTOM = (0xD8, 0xD4, 0xBE)
GROOVE = (0xC9, 0xC5, 0xAE)
INK = (0x1B, 0x1B, 0x1A)
SCREEN = (0x1A, 0x1B, 0x1D)
SCREEN_EDGE = (0x0E, 0x0E, 0x0F)
SEG_OFF = (0x34, 0x36, 0x3B)
SCREEN_TEXT = (0xE6, 0xE4, 0xDE)
AMBER = (0xFF, 0xB0, 0x20)


def px(v):
    return int(round(v * S))


def squircle_mask(size, inset, exponent=5.0):
    """Superellipse close to Apple's continuous-corner icon shape."""
    half = (size - 2 * inset) / 2
    c = size / 2
    ys, xs = np.mgrid[0:size, 0:size].astype(np.float32) + 0.5
    d = np.abs((xs - c) / half) ** exponent + np.abs((ys - c) / half) ** exponent
    return (d <= 1.0).astype(np.float32)


def vertical_gradient(size, top, bottom):
    """Float gradient with dither, so the 8-bit result has no bands."""
    t = np.linspace(0.0, 1.0, size, dtype=np.float32)[:, None, None]
    rgb = (1 - t) * np.array(top, np.float32) + t * np.array(bottom, np.float32)
    rgb = np.broadcast_to(rgb, (size, size, 3)).copy()
    rgb += np.random.default_rng(7).uniform(-0.5, 0.5, rgb.shape).astype(np.float32)
    return np.clip(np.round(rgb), 0, 255).astype(np.uint8)


def main(out):
    mask = squircle_mask(N, px(100))

    # Faceplate with a light top edge, as if lit from above.
    face = Image.fromarray(vertical_gradient(N, FACE_TOP, FACE_BOTTOM), "RGB")
    draw = ImageDraw.Draw(face)

    # Recessed screen with left and right level meters.
    screen = (px(196), px(206), px(828), px(560))
    draw.rounded_rectangle(screen, radius=px(40), fill=SCREEN_EDGE)
    inner = (screen[0] + px(5), screen[1] + px(7), screen[2] - px(5), screen[3] - px(4))
    draw.rounded_rectangle(inner, radius=px(36), fill=SCREEN)
    draw.line([(screen[0] + px(40), screen[3] + px(3)), (screen[2] - px(40), screen[3] + px(3))], fill=(0xF7, 0xF6, 0xF2), width=px(3))

    seg_w, gap, seg_h = 18, 10, 64
    step = seg_w + gap
    total = 19
    meters = [(15, 17), (12, 14)]
    row_gap = 46
    block_h = len(meters) * seg_h + (len(meters) - 1) * row_gap
    x0 = 512 - (total * step - gap) / 2
    y0 = (206 + 560) / 2 - block_h / 2
    for r, (lit, peak) in enumerate(meters):
        y = y0 + r * (seg_h + row_gap)
        for k in range(total):
            x = x0 + k * step
            color = AMBER if k < lit else SCREEN_TEXT if k == peak else SEG_OFF
            draw.rectangle([px(x), px(y), px(x + seg_w) - 1, px(y + seg_h) - 1], fill=color)

    # The play key: ink outline, printed triangle, amber lamp.
    key = (392, 640, 632, 812)
    draw.rounded_rectangle([px(v) for v in key], radius=px(22), outline=INK, width=px(12))
    cx, cy = (key[0] + key[2]) / 2 + 6, (key[1] + key[3]) / 2
    tri = [(cx - 34, cy - 40), (cx + 40, cy), (cx - 34, cy + 40)]
    draw.polygon([(px(x), px(y)) for x, y in tri], fill=INK)
    lamp = (key[2] - 30, key[1] + 30)
    r = 13
    draw.ellipse([px(lamp[0] - r), px(lamp[1] - r), px(lamp[0] + r), px(lamp[1] + r)], fill=AMBER)

    # Soft shadow under the device, per Apple's icon template.
    alpha = Image.fromarray((mask * 255).astype(np.uint8), "L")
    shadow_alpha = alpha.filter(ImageFilter.GaussianBlur(px(14))).point(lambda v: int(v * 0.32))
    shadow = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 255), (0, px(10)), shadow_alpha)

    device = face.convert("RGBA")
    device.putalpha(alpha)

    # Thin dark rim so the light faceplate holds its edge on white backgrounds.
    rim = alpha.filter(ImageFilter.MinFilter(px(2) * 2 + 1))
    edge = Image.fromarray(np.clip(np.asarray(alpha, np.int16) - np.asarray(rim, np.int16), 0, 255).astype(np.uint8), "L")
    rim_layer = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    rim_layer.paste((0x8E, 0x8A, 0x82, 255), (0, 0), edge.point(lambda v: int(v * 0.55)))

    icon = Image.alpha_composite(shadow, device)
    icon = Image.alpha_composite(icon, rim_layer)
    icon.resize((1024, 1024), Image.LANCZOS).save(out)


if __name__ == "__main__":
    main(sys.argv[1])
