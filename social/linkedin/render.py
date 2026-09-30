"""Render carousel.html to nexspec-carousel.pdf and png/slide-NN.png using Edge headless.
Usage: python render.py   (Windows, Edge installed)"""
import pathlib, re, subprocess, time

HERE = pathlib.Path(__file__).parent.resolve()
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
html = (HERE / "carousel.html").read_text(encoding="utf-8")
head, *secs = html.split('<section class="slide">')
secs = [re.sub(r"</body>.*", "", s, flags=re.S) for s in secs]

def edge(*args, out):
    # headless Edge can return before the file is fully written: poll for it
    before = out.stat().st_mtime if out.exists() else 0
    subprocess.run([EDGE, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    "--virtual-time-budget=10000", *args], capture_output=True, timeout=120)
    for _ in range(60):
        if out.exists() and out.stat().st_mtime > before and out.stat().st_size > 0:
            time.sleep(1)
            return
        time.sleep(1)
    raise RuntimeError(f"failed to render {out}")

pdf = HERE / "nexspec-carousel.pdf"
edge("--no-pdf-header-footer", f"--print-to-pdf={pdf}", (HERE / "carousel.html").as_uri(), out=pdf)

(HERE / "png").mkdir(exist_ok=True)
for i, body in enumerate(secs, 1):
    tmp = HERE / f".slide-{i}.tmp.html"
    tmp.write_text(head + '<section class="slide">' + body + "</body></html>", encoding="utf-8")
    png = HERE / "png" / f"slide-{i:02d}.png"
    edge("--window-size=1080,1350", f"--screenshot={png}", tmp.as_uri(), out=png)
    tmp.unlink()
print("ok")
