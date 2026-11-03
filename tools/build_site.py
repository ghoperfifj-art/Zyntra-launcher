"""Builds the website's article pages and sitemap.

Each file in site-src/pages/ is one page: a few "key: value" lines, a line with "---",
then the page body in HTML. This script wraps it in the shared header, footer, meta tags
and structured data and writes site/<path>/index.html, then site/sitemap.xml.
The home page (site/index.html) and the English page (site/en/index.html) are written
by hand; their dates are set below.

Run from the project folder after changing a page:  python tools/build_site.py
"""

import html
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "site-src" / "pages"
OUT = ROOT / "site"

# Change this one line when the site gets its own domain (e.g. "https://zyntra.ru/").
BASE = "https://ghoperfifj-art.github.io/Zyntra-launcher-/"
RELEASES = "https://github.com/ghoperfifj-art/Zyntra-launcher-/releases/latest/download/"
REPO = "https://github.com/ghoperfifj-art/Zyntra-launcher-"

# Hand-written pages: (path, last change)
HAND_WRITTEN = [("", "2026-11-03"), ("en/", "2026-11-03")]

NAV = [("guides/", "Гайды"), ("tlauncher-alternative/", "Сравнение"), ("mac/", "Mac"), ("horizon/", "Horizon")]

FOOTER_LINKS = [
    ("tlauncher-alternative/", "Альтернатива TLauncher"),
    ("mac/", "Minecraft на Mac M1–M4"),
    ("horizon/", "Horizon 26.3"),
    ("mods/", "Как ставить моды"),
    ("shaders/", "Как ставить шейдеры"),
    ("guides/", "Все гайды"),
    ("changelog/", "Что нового"),
    ("en/", "English"),
]


def parse(path: Path) -> dict:
    text = path.read_text(encoding="utf-8")
    head, body = text.split("\n---\n", 1)
    page = {"body": body.strip()}
    for line in head.splitlines():
        if line.strip():
            key, value = line.split(":", 1)
            page[key.strip()] = value.strip()
    page.setdefault("related", "")
    return page


def faq_items(body: str) -> list:
    """Questions written as <details><summary>…</summary><p>…</p></details>."""
    strip = lambda s: html.unescape(re.sub(r"<[^>]+>", "", s)).strip()
    return [(strip(q), strip(a)) for q, a in re.findall(r"<summary>(.*?)</summary>\s*(.*?)\s*</details>", body, re.S)]


def json_ld(page: dict, url: str) -> str:
    crumbs = [("Zyntra Launcher", BASE)]
    if page.get("parent"):
        name, ppath = page["parent"].split("|")
        crumbs.append((name, BASE + ppath))
    crumbs.append((page["crumb"], url))
    blocks = [
        {
            "@context": "https://schema.org",
            "@type": page.get("type", "Article"),
            "headline": page["h1"],
            "description": page["description"],
            "url": url,
            "inLanguage": "ru",
            "dateModified": page["updated"],
            "datePublished": page.get("published", page["updated"]),
            "image": BASE + "img/og.png",
            "author": {"@type": "Organization", "name": "Zyntra", "url": BASE},
            "publisher": {"@type": "Organization", "name": "Zyntra", "url": BASE, "logo": {"@type": "ImageObject", "url": BASE + "img/logo.png"}},
            "mainEntityOfPage": url,
        },
        {
            "@context": "https://schema.org",
            "@type": "BreadcrumbList",
            "itemListElement": [{"@type": "ListItem", "position": i + 1, "name": n, "item": u} for i, (n, u) in enumerate(crumbs)],
        },
    ]
    faq = faq_items(page["body"])
    if faq:
        blocks.append({
            "@context": "https://schema.org",
            "@type": "FAQPage",
            "mainEntity": [{"@type": "Question", "name": q, "acceptedAnswer": {"@type": "Answer", "text": a}} for q, a in faq],
        })
    return "\n".join('<script type="application/ld+json">\n' + json.dumps(b, ensure_ascii=False, indent=1) + "\n</script>" for b in blocks)


def render(page: dict, pages: dict) -> str:
    path = page["path"]
    up = "../" * path.count("/")
    url = BASE + path
    esc = html.escape

    crumbs = f'<a href="{up}">Главная</a><span>›</span>'
    if page.get("parent"):
        name, ppath = page["parent"].split("|")
        crumbs += f'<a href="{up}{ppath}">{esc(name)}</a><span>›</span>'
    crumbs += esc(page["crumb"])

    related = ""
    if page["related"]:
        cards = []
        for rel in [r.strip() for r in page["related"].split(",") if r.strip()]:
            other = pages[rel]
            cards.append(f'<a class="card" href="{up}{rel}"><h3>{esc(other["crumb"])}</h3><p>{esc(other["description"])}</p></a>')
        related = f'<section class="related"><h2>Читайте также</h2><div class="cards">{"".join(cards)}</div></section>'

    nav = "\n      ".join(f'<a href="{up}{p}">{n}</a>' for p, n in NAV)
    foot = "\n        ".join(f'<a href="{up}{p}">{n}</a>' for p, n in FOOTER_LINKS)
    body = page["body"].replace("{up}", up).replace("{releases}", RELEASES).replace("{repo}", REPO)
    updated = page["updated"]
    y, m, d = updated.split("-")
    months = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]
    updated_ru = f"{int(d)} {months[int(m) - 1]} {y}"

    return f"""<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{esc(page["title"])}</title>
<meta name="description" content="{esc(page["description"])}">
<link rel="canonical" href="{url}">
<meta property="og:type" content="article">
<meta property="og:site_name" content="Zyntra Launcher">
<meta property="og:locale" content="ru_RU">
<meta property="og:url" content="{url}">
<meta property="og:title" content="{esc(page["title"])}">
<meta property="og:description" content="{esc(page["description"])}">
<meta property="og:image" content="{BASE}img/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="article:modified_time" content="{updated}">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="{BASE}img/og.png">
<meta name="theme-color" content="#0d0a14">
<link rel="icon" type="image/png" href="{up}img/favicon.png">
<link rel="preload" href="{up}fonts/inter-cyrillic.woff2" as="font" type="font/woff2" crossorigin>
<link rel="preload" href="{up}fonts/unbounded-cyrillic.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="{up}style.css">
{json_ld(page, url)}
</head>
<body>

<header>
  <div class="wrap nav">
    <a class="brand" href="{up}">
      <img src="{up}img/logo.png" width="38" height="38" alt="Zyntra Launcher">
      <span><b>ZYNTRA</b><small>LAUNCHER</small></span>
    </a>
    <nav class="links">
      {nav}
    </nav>
    <a class="btn small primary" href="{up}#download">Скачать</a>
  </div>
</header>

<main class="wrap">
  <nav class="crumbs" aria-label="Навигация">{crumbs}</nav>
  <div class="page-head">
    <h1>{esc(page["h1"])}</h1>
    <p class="lead">{page["lead"]}</p>
    <time class="updated" datetime="{updated}">Обновлено {updated_ru}</time>
  </div>
  <article class="article">
{body}
  </article>
  {related}
</main>

<footer>
  <div class="wrap foot">
    <div>
      <b style="color:var(--text)">Zyntra Launcher</b> · <a href="{REPO}">GitHub</a><br>
      Неофициальный проект, не связан с Mojang Studios и Microsoft.<br>Minecraft — торговая марка Mojang и Microsoft.
    </div>
    <nav class="foot-links" aria-label="Разделы сайта">
        {foot}
    </nav>
  </div>
</footer>
</body>
</html>
"""


def main():
    pages = {}
    for f in sorted(SRC.glob("*.html")):
        page = parse(f)
        pages[page["path"]] = page
    for page in pages.values():
        out = OUT / page["path"] / "index.html"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(render(page, pages), encoding="utf-8", newline="\n")
        print("site/" + page["path"])

    urls = HAND_WRITTEN + [(p["path"], p["updated"]) for p in pages.values()]
    sitemap = ['<?xml version="1.0" encoding="UTF-8"?>', '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">']
    for path, date in urls:
        sitemap.append(f"  <url>\n    <loc>{BASE}{path}</loc>\n    <lastmod>{date}</lastmod>\n  </url>")
    sitemap.append("</urlset>\n")
    (OUT / "sitemap.xml").write_text("\n".join(sitemap), encoding="utf-8", newline="\n")
    print(f"site/sitemap.xml ({len(urls)} pages)")


if __name__ == "__main__":
    main()
