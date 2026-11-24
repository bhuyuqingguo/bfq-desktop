from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 256
image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)

# Deep-teal application tile with a compact archive/lock mark.
draw.rounded_rectangle((8, 8, 248, 248), radius=52, fill="#123F43")
draw.rounded_rectangle((16, 16, 240, 240), radius=44, fill="#16565A")

# Archive outline.
draw.rounded_rectangle((54, 48, 202, 208), radius=18, outline="#F4F8F7", width=12)
draw.line((54, 94, 202, 94), fill="#F4F8F7", width=12)
draw.line((98, 94, 98, 208), fill="#F4F8F7", width=12)

# Small lock in the lower-right compartment.
draw.arc((126, 116, 178, 168), start=180, end=360, fill="#8AD0C6", width=10)
draw.rounded_rectangle((120, 140, 184, 190), radius=10, fill="#8AD0C6")
draw.ellipse((147, 154, 157, 164), fill="#16565A")
draw.line((152, 162, 152, 174), fill="#16565A", width=5)

output = Path(__file__).resolve().parents[1] / "src-tauri" / "icons" / "icon.ico"
output.parent.mkdir(parents=True, exist_ok=True)
image.save(output, format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print(output)

source = Path(__file__).resolve().parents[1] / "assets" / "icon-source.png"
source.parent.mkdir(parents=True, exist_ok=True)
image.resize((1024, 1024), Image.Resampling.LANCZOS).save(source)
print(source)
