#!/usr/bin/env python3
"""
从 source.png 生成 icon.png（1024x1024 苹果风格圆角白底 logo）。

源图本身已带白色画布，所以无需再 padding。仅做：
  1) 缩放到 1024x1024
  2) 应用圆角 mask：四角变透明（alpha=0），便于 macOS Dock 混合

输出后用 `pnpm tauri icon assets/icon.png` 生成全套 app icon。
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).parent
SOURCE_PNG = ROOT / "source.png"
ICON_PNG = ROOT / "icon.png"

SIZE = 1024
# macOS Big Sur+ app icon 圆角约为画布的 22.37%（squircle 近似）
CORNER_RATIO = 0.2237


def main():
    src = Image.open(SOURCE_PNG).convert("RGBA")
    print(f"源图: {src.size} 模式={src.mode}")

    # 居中裁切到正方形（保险起见，源图理论上已经是正方形）
    w, h = src.size
    if w != h:
        side = min(w, h)
        left = (w - side) // 2
        top = (h - side) // 2
        src = src.crop((left, top, left + side, top + side))

    # 缩放到目标尺寸
    src = src.resize((SIZE, SIZE), Image.LANCZOS)

    # 圆角 mask：rounded rectangle 内为 255（保留），外为 0（透明）
    radius = int(SIZE * CORNER_RATIO)
    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, SIZE - 1, SIZE - 1), radius=radius, fill=255
    )

    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(src, (0, 0), mask)
    out.save(ICON_PNG, "PNG")
    print(f"✓ 圆角白底 icon: {ICON_PNG}  ({SIZE}x{SIZE}, 圆角半径={radius}px)")


if __name__ == "__main__":
    main()
