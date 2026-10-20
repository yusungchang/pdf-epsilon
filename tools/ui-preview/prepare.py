# Precomputes what the mocked bridge serves: thumbnails and full-width page renders from the
# app's own renderer process, for the original and for a Balanced copy made by the engine.
# Usage: python3 -I prepare.py <app binary> <inspect binary> <squeeze binary> <out.js> file.pdf...
import base64, json, os, struct, subprocess, sys, tempfile
app, inspect, squeeze, out = sys.argv[1:5]
pdfs = sys.argv[5:]

def render(pdf, pages, width):
    frames = subprocess.run([app, "--render-thumbnails", pdf, str(width)], input="".join(f"{p}\n" for p in pages).encode(),
                            capture_output=True, timeout=120).stdout
    got, at = {}, 0
    while at + 8 <= len(frames):
        page, n = struct.unpack(">II", frames[at:at + 8]); at += 8
        got[page] = base64.b64encode(frames[at:at + n]).decode() if n else None; at += n
    return got

thumbs, renders = {}, None
for i, pdf in enumerate(pdfs):
    name = os.path.basename(pdf)
    analysis = json.loads(subprocess.run([inspect, pdf, "--json"], capture_output=True).stdout)
    thumbs[name] = render(pdf, [p["number"] for p in analysis["pages"]][:40], 240)
    if i == 0:
        with tempfile.TemporaryDirectory() as d:
            small = os.path.join(d, "balanced.pdf")
            subprocess.run([squeeze, pdf, small, "balanced"], capture_output=True)
            pages = [1, 2, 3]
            renders = {"name": name, "original": render(pdf, pages, 1100), "compressed": render(small, pages, 1100),
                       "zoomed": render(pdf, pages, 2200), "zoomedCompressed": render(small, pages, 2200)}
with open(out, "w") as f:
    f.write("window.__THUMBS__ = " + json.dumps(thumbs) + ";\n")
    f.write("window.__RENDERS__ = " + json.dumps(renders) + ";\n")
