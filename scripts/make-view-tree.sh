#!/usr/bin/env bash
# Development: generates dummy media for vector-view tests and screenshots (never real files).
#   scripts/make-view-tree.sh   → $VECTOR_DEV/media (default /tmp/vector-dev/media)
# Needs ImageMagick, exiv2, ffmpeg and LibreOffice.
set -euo pipefail
DEV="${VECTOR_DEV:-${TMPDIR:-/tmp}/vector-dev}"
M="$DEV/media"
rm -rf "$M"; mkdir -p "$M"; cd "$M"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT

# images: generated gradients and noise, no photos
magick -size 2400x1600 gradient:'#5e81ac-#bf616a' -swirl 160 \
  -fill '#eceff4' -pointsize 90 -gravity center -annotate 0 'dummy 01' aurora.jpg
magick -size 1600x2400 plasma:'#a3be8c-#2e3440' -seed 7 -blur 0x3 tall-plasma.png
magick -seed 3 -size 3000x2000 plasma:fractal -modulate 100,40 -blur 0x1 big-fractal.webp
magick -size 640x640 xc:none -fill '#ebcb8b' -draw 'circle 320,320 320,60' \
  -fill '#2e3440' -pointsize 64 -gravity center -annotate 0 'alpha' transparent.png
magick -size 64x48 pattern:checkerboard -scale 800% pixel-art.png
# a JPEG stored sideways with an EXIF orientation tag (should show upright)
magick -size 1200x800 gradient:'#88c0d0-#4c566a' -fill white -pointsize 120 -gravity north \
  -annotate 0 'UP' -rotate -90 rotated-exif.jpg
exiv2 -M'set Exif.Image.Orientation Short 6' rotated-exif.jpg

# video and animation
ffmpeg -loglevel error -f lavfi -i testsrc2=size=1280x720:rate=30 -f lavfi -i sine=frequency=330 \
  -t 12 -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest clip.mp4
ffmpeg -loglevel error -f lavfi -i mandelbrot=size=480x320:rate=15 -t 3 spin.gif

# documents: made by LibreOffice from generated text
cat > "$work/report.html" <<'EOF'
<h1>Quarterly report (dummy)</h1>
<p>Generated test document. Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>
<h2>Numbers</h2><table border=1><tr><th>month</th><th>value</th></tr>
<tr><td>january</td><td>12</td></tr><tr><td>february</td><td>17</td></tr></table>
<p style="page-break-before:always">Second page of the dummy report.</p>
EOF
printf 'item,count,price\napples,12,0.5\npears,7,0.8\nplums,30,0.2\n' > "$work/budget.csv"
lo() { soffice -env:UserInstallation=file://$work/lo --headless "$@" >/dev/null 2>&1; }
lo --convert-to docx:"MS Word 2007 XML" --outdir . "$work/report.html"
lo --convert-to xlsx --outdir . "$work/budget.csv"
lo --convert-to pdf --outdir "$work" "$work/report.html"
cp "$work/report.pdf" manual.pdf
lo --infilter=impress_pdf_import --convert-to pptx --outdir . "$work/report.pdf" && mv report.pptx slides.pptx || true
touch notes.txt
ls -1 "$M"
