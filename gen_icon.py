import math
from PIL import Image, ImageDraw, ImageFont

# ── 超采样设置 ──────────────────────────────────────────
TARGET_SIZE = 1024
SCALE = 4  # 放大 4 倍绘制，最后缩小以消除锯齿
SIZE = TARGET_SIZE * SCALE
CENTER = SIZE // 2

# ── 颜色 ────────────────────────────────────────────────
BG_COLOR  = (245, 248, 255)
NAVY      = ( 26,  48, 104)
TEAL      = ( 46, 203, 165)

# ── 放大后的比例参数 (整体变大) ─────────────────────────
STROKE_W = 16 * SCALE     # 描边宽度 (微调加粗)
FILL_W   = 56 * SCALE     # 薄荷绿主线宽 (显著加粗)
DOT_R    = 56 * SCALE     # 中心圆点半径 (显著加大)
GAP      = 46 * SCALE     # 线条间的空白间距

NAVY_W = FILL_W + STROKE_W * 2
TEAL_W = FILL_W

# Wi-Fi "原点" 下移，使放大的图标和文字在整个画布中居中对齐
WX   = CENTER
WY_D = 580 * SCALE

# 角度
ARC_START = 210
ARC_END   = 330

# 预先计算每条弧线的绝对中心半径
r0_outer = DOT_R + STROKE_W
r1_center = r0_outer + GAP + (NAVY_W / 2)
r2_center = r1_center + (NAVY_W / 2) + GAP + (NAVY_W / 2)
r3_center = r2_center + (NAVY_W / 2) + GAP + (NAVY_W / 2)

radii = [r1_center, r2_center, r3_center]

# ── 大画布绘制 ──────────────────────────────────────────
img  = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# ── 圆角矩形背景 ────────────────────────────────────────
RADIUS = 225 * SCALE
draw.rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=RADIUS,
                        fill=(*BG_COLOR, 255))

mask = Image.new("L", (SIZE, SIZE), 0)
mask_draw = ImageDraw.Draw(mask)
mask_draw.rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=RADIUS, fill=255)
img.putalpha(mask)
draw = ImageDraw.Draw(img)

# ── 1. Wi-Fi 圆点 ───────────────────────────────────────
draw.ellipse([WX - DOT_R - STROKE_W, WY_D - DOT_R - STROKE_W,
             WX + DOT_R + STROKE_W, WY_D + DOT_R + STROKE_W], fill=(*NAVY, 255))
draw.ellipse([WX - DOT_R, WY_D - DOT_R, WX + DOT_R, WY_D + DOT_R], fill=(*TEAL, 255))

# ── 2. 深蓝背景层 (轮廓 + 端点圆) ───────────────────────
for r in radii:
    r_outer = r + NAVY_W / 2
    bbox = [WX - r_outer, WY_D - r_outer, WX + r_outer, WY_D + r_outer]
    draw.arc(bbox, ARC_START, ARC_END, fill=(*NAVY, 255), width=int(NAVY_W))
    
    sx = WX + r * math.cos(math.radians(ARC_START))
    sy = WY_D + r * math.sin(math.radians(ARC_START))
    ex = WX + r * math.cos(math.radians(ARC_END))
    ey = WY_D + r * math.sin(math.radians(ARC_END))
    
    cr = NAVY_W / 2
    draw.ellipse([sx - cr, sy - cr, sx + cr, sy + cr], fill=(*NAVY, 255))
    draw.ellipse([ex - cr, ey - cr, ex + cr, ey + cr], fill=(*NAVY, 255))

# ── 3. 薄荷绿填充层 (内部线 + 端点圆) ───────────────────
for r in radii:
    r_outer = r + TEAL_W / 2
    bbox = [WX - r_outer, WY_D - r_outer, WX + r_outer, WY_D + r_outer]
    draw.arc(bbox, ARC_START, ARC_END, fill=(*TEAL, 255), width=int(TEAL_W))
    
    sx = WX + r * math.cos(math.radians(ARC_START))
    sy = WY_D + r * math.sin(math.radians(ARC_START))
    ex = WX + r * math.cos(math.radians(ARC_END))
    ey = WY_D + r * math.sin(math.radians(ARC_END))
    
    cr = TEAL_W / 2
    draw.ellipse([sx - cr, sy - cr, sx + cr, sy + cr], fill=(*TEAL, 255))
    draw.ellipse([ex - cr, ey - cr, ex + cr, ey + cr], fill=(*TEAL, 255))

# ── "SHU" 文字 ──────────────────────────────────────────
TEXT    = "SHU"
FONT_SZ = 260 * SCALE  

font = None
candidates = [
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSDisplay-Bold.otf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
]
for path in candidates:
    try:
        font = ImageFont.truetype(path, FONT_SZ)
        break
    except Exception:
        pass
if font is None:
    font = ImageFont.load_default()

bbox = draw.textbbox((0, 0), TEXT, font=font)
tw = bbox[2] - bbox[0]
th = bbox[3] - bbox[1]
tx = (SIZE - tw) // 2 - bbox[0]
ty = 730 * SCALE - bbox[1]  # 文字位置适配新的中心点

draw.text((tx, ty), TEXT, font=font, fill=(*NAVY, 255))

# ── 缩小并保存 (抗锯齿核心步) ───────────────────────────
final_img = img.resize((TARGET_SIZE, TARGET_SIZE), resample=Image.Resampling.LANCZOS)
final_img.save("icon.png")
print(f"icon.png saved ({TARGET_SIZE}x{TARGET_SIZE}) with anti-aliasing")