from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SIZE = 256
SEA = "#003000"
PAPER = "#F0F0E6"
SEAL = "#C9131B"
image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)

# A printed-paper seal on a deep-sea field: eight separated archive segments.
draw.rounded_rectangle((6, 6, 250, 250), radius=48, fill=SEA)
draw.rounded_rectangle((15, 15, 241, 241), radius=40, outline="#527052", width=2)
for index in range(8):
    start = -90 + index * 45 + 3
    end = -90 + (index + 1) * 45 - 3
    draw.pieslice((38, 38, 218, 218), start=start, end=end, fill=PAPER)
    draw.pieslice((66, 66, 190, 190), start=0, end=360, fill=SEA)

draw.ellipse((78, 78, 178, 178), fill=SEAL)
try:
    font = ImageFont.truetype("simkai.ttf", 66)
except OSError:
    font = ImageFont.load_default(size=66)
draw.text((128, 125), "八", font=font, fill=PAPER, anchor="mm")

output = Path(__file__).resolve().parents[1] / "src-tauri" / "icons" / "icon.ico"
output.parent.mkdir(parents=True, exist_ok=True)
image.save(output, format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print(output)

source = Path(__file__).resolve().parents[1] / "assets" / "icon-source.png"
source.parent.mkdir(parents=True, exist_ok=True)
image.resize((1024, 1024), Image.Resampling.LANCZOS).save(source)
print(source)
