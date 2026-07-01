Convert a Markdown file to a styled PDF and a stitched full-page PNG, installing any missing dependencies automatically.

You are converting the Markdown file at path: $ARGUMENTS

Follow these steps exactly. Do not ask the user to install anything or run anything manually — handle it all yourself.

## 0. Resolve input and output paths

- If `$ARGUMENTS` is empty, ask the user which Markdown file to convert (or stop with a clear error).
- Resolve the input path to an absolute path and confirm it exists (`test -f`). If it doesn't, stop and report the error.
- Determine the output directory:
  - If `.sbxw-artifacts/` exists in the current working directory, use it.
  - Otherwise, use the same directory as the source file.
- Base name for outputs = the source file's stem (e.g. `ARCHITECTURE.md` → `ARCHITECTURE`).

## 1. Check and install dependencies (only if missing)

Check each of these before installing anything — do not reinstall what's already present:

```bash
python3 -c "import weasyprint" 2>/dev/null && echo weasyprint:ok || echo weasyprint:missing
python3 -c "import markdown" 2>/dev/null && echo markdown:ok || echo markdown:missing
python3 -c "import PIL" 2>/dev/null && echo pillow:ok || echo pillow:missing
which pdftoppm >/dev/null 2>&1 && echo poppler:ok || echo poppler:missing
ldconfig -p | grep -q libpango-1.0.so && echo pango:ok || echo pango:missing
```

For anything missing, install silently:

```bash
sudo apt-get update -qq && sudo apt-get install -y -qq \
  poppler-utils libpango-1.0-0 libpangoft2-1.0-0 libgobject-2.0-0 libglib2.0-0
pip3 install --break-system-packages -q weasyprint markdown pillow
```

Only run the parts that are actually needed based on the checks above — skip apt/pip entirely if everything is already `ok`.

## 2. Convert Markdown to styled HTML

Write a small Python script (e.g. to the scratchpad dir) that reads the source Markdown, converts it with the `markdown` library using extensions `['extra', 'tables', 'fenced_code', 'toc', 'codehilite']`, and wraps it in a styled HTML document. Use CSS along these lines (GitHub-like, A4-friendly, and note `overflow-x: hidden` — NOT `auto`, because WeasyPrint ignores `auto` and warns):

```python
import markdown, sys, pathlib

src = pathlib.Path(sys.argv[1])
dst = pathlib.Path(sys.argv[2])

html_body = markdown.markdown(
    src.read_text(encoding="utf-8"),
    extensions=["extra", "tables", "fenced_code", "toc", "codehilite"],
)

CSS = """
@page { size: A4; margin: 2cm; }
body {
  font-family: -apple-system, "Segoe UI", Helvetica, Arial, sans-serif;
  font-size: 11pt;
  line-height: 1.55;
  color: #1f2328;
  overflow-x: hidden;
}
h1, h2, h3, h4 { font-weight: 600; margin-top: 1.4em; margin-bottom: 0.6em; }
h1 { font-size: 1.9em; border-bottom: 1px solid #d0d7de; padding-bottom: 0.3em; }
h2 { font-size: 1.5em; border-bottom: 1px solid #d0d7de; padding-bottom: 0.3em; }
code { background: #f6f8fa; padding: 0.15em 0.35em; border-radius: 4px; font-family: "SFMono-Regular", Consolas, Menlo, monospace; font-size: 0.9em; }
pre { background: #f6f8fa; padding: 1em; border-radius: 6px; overflow-x: hidden; white-space: pre-wrap; word-wrap: break-word; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 1em 0; }
th, td { border: 1px solid #d0d7de; padding: 0.5em 0.8em; }
th { background: #f6f8fa; }
blockquote { border-left: 4px solid #d0d7de; margin: 1em 0; padding: 0 1em; color: #57606a; }
img { max-width: 100%; }
a { color: #0969da; text-decoration: none; }
"""

dst.write_text(
    f"<!doctype html><html><head><meta charset='utf-8'><style>{CSS}</style></head>"
    f"<body>{html_body}</body></html>",
    encoding="utf-8",
)
```

Run it: `python3 <script>.py <source.md> <scratchpad>/<basename>.html`

## 3. Render HTML to PDF

```bash
weasyprint "<scratchpad>/<basename>.html" "<output_dir>/<basename>.pdf"
```

## 4. Rasterize PDF pages to PNG

```bash
pdftoppm -r 150 -png "<output_dir>/<basename>.pdf" "<scratchpad>/<basename>-page"
```

This produces one PNG per page (e.g. `<basename>-page-1.png`, `<basename>-page-2.png`, ...).

## 5. Stitch pages into a single full-height PNG

```bash
python3 -c "
from PIL import Image
import glob

files = sorted(glob.glob('<scratchpad>/<basename>-page-*.png'))
images = [Image.open(f) for f in files]
width = max(im.width for im in images)
total_height = sum(im.height for im in images)
stitched = Image.new('RGB', (width, total_height), 'white')
y = 0
for im in images:
    stitched.paste(im, (0, y))
    y += im.height
stitched.save('<output_dir>/<basename>.png')
"
```

## 6. Report results

Print the final absolute paths of the generated PDF and stitched PNG, e.g.:

```
PDF: <output_dir>/<basename>.pdf
PNG: <output_dir>/<basename>.png
```

Clean up intermediate files in the scratchpad (per-page PNGs, the HTML) but leave the PDF/PNG in the output directory. Do not print large amounts of raw command output — just confirm success and show the two final paths.
