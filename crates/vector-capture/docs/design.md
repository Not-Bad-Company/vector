# Vector Capture

Screenshots, colour picking, screen recording and text from the screen, in one
resident tool. Replaces Omarchy's `omarchy-capture-*` scripts, `hyprpicker`
and `satty`-on-enter flow.

## Shape

- `vector-capture` is a light client: it forwards its words over D-Bus to
  `vector-capture-daemon` (run by `vector-capture.service`), or becomes the
  daemon when none runs. The daemon keeps GTK, the overlay and the OCR models
  warm, so a key press reaches a frozen overlay in about 50-75 ms.
- The screen is frozen by grabbing the output once (grim, raw PPM) at the key
  press; the overlay draws that image and every crop comes from it, so the
  overlay never ends up in the picture.

## Commands and keys

| Command | Key | What |
|---|---|---|
| `shot region` | PRINT, SUPER+S | drag a region; copies the PNG and saves it |
| `shot window` | SUPER+SHIFT+S | click a window |
| `shot output` | | the whole screen |
| `shot ... --view` | | also opens it in Vector View |
| `pick [hex\|rgb\|hsl]` | SUPER+PRINT, SUPER+CTRL+P | colour under the cursor with a magnifier; h/r/l switch format |
| `record region\|output [--audio=desktop,mic]` | (not bound yet) | gpu-screen-recorder; the same command stops it |
| `ocr` | SUPER+CTRL+PRINT | text of a selection to the clipboard |
| `ocr FILE` | | prints the text of an image file (used by Vector View) |

Screenshots go where Omarchy put them (`$OMARCHY_SCREENSHOT_DIR`, else the XDG
pictures dir).

## Text recognition

Local and offline: PaddleOCR PP-OCRv5 (mobile detection + Latin recognition,
about 21 MB, downloaded once to `~/.local/share/vector-capture/models`) run
through ONNX Runtime via `oar-ocr`. Reads Dutch and other Latin-script
accents correctly, about 0.2 s warm. tesseract is the fallback when the models
are missing. No cloud service, no API key.

## Testing

On the AI workspace only: `aiws nest` for the overlay and clipboard,
synthetic images from `aiws make` or ImageMagick-rendered text for OCR.
`VECTOR_CAPTURE_TEST=1` runs a separate test instance; `VECTOR_CAPTURE_RECORDER`
swaps in a stand-in recorder.
