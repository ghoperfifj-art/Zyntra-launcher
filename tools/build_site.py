"""Builds the website's article pages, sitemap and llms.txt.

Each file in site-src/pages/ is one page: a few "key: value" lines, a line with "---",
then the page body in HTML. This script wraps it in the shared header, footer, meta tags
and structured data and writes site/<path>/index.html, then site/sitemap.xml and
site/llms.txt + site/llms-full.txt (the site in plain text for AI search).
The home page (site/index.html) and the English page (site/en/index.html) are written
by hand; this script only keeps their version and date current.

The current release is kept in site-src/release.json.

    python tools/build_site.py             after changing a page
    python tools/build_site.py --release   when a version is released (the Release workflow
                                           runs it): takes the version from Cargo.toml and
                                           today's date, adds the version to «Что нового»
                                           from its commits, and dates the home pages
"""

import datetime
import html
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "site-src" / "pages"
OUT = ROOT / "site"
RELEASE_FILE = ROOT / "site-src" / "release.json"

# Change this one line when the site gets its own domain (e.g. "https://zyntra.ru/").
BASE = "https://ghoperfifj-art.github.io/Zyntra-launcher-/"
RELEASES = "https://github.com/ghoperfifj-art/Zyntra-launcher-/releases/latest/download/"
REPO = "https://github.com/ghoperfifj-art/Zyntra-launcher-"

# Hand-written pages, dated by the release.
HAND_WRITTEN = [("", "ru"), ("en/", "en")]

MONTHS_RU = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]
MONTHS_EN = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]


def date_ru(iso: str) -> str:
    y, m, d = iso.split("-")
    return f"{int(d)} {MONTHS_RU[int(m) - 1]} {y}"


def date_en(iso: str) -> str:
    y, m, d = iso.split("-")
    return f"{MONTHS_EN[int(m) - 1]} {int(d)}, {y}"

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
    updated_ru = date_ru(updated)

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


def git(*args) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8").stdout.strip()


def release_notes(version: str) -> list:
    """What changed in the launcher since the previous tag: the commits' first lines,
    without the website's own, the repository's housekeeping and the version bump
    (subjects starting «Сайт», «Репозиторий», «README», «Версия»)."""
    tag = f"v{version}"
    target = tag if git("tag", "-l", tag) else "HEAD"
    previous = git("describe", "--tags", "--abbrev=0", f"{target}^")
    span = f"{previous}..{target}" if previous else target
    notes = []
    for subject in git("log", "--format=%s", span).splitlines():
        if subject.startswith(("Сайт", "Версия", "Репозиторий", "README", "Merge")) or not subject.strip():
            continue
        notes.append(subject.strip().rstrip("."))
    return notes or ["Исправления и улучшения"]


def add_to_changelog(release: dict):
    """Adds the release to «Что нового» (if it isn't there yet) and dates the page."""
    path = SRC / "changelog.html"
    text = path.read_text(encoding="utf-8")
    head, body = text.split("\n---\n", 1)
    head = re.sub(r"^updated: .*$", f"updated: {release['date']}", head, flags=re.M)
    if f"<h2>{release['version']} " not in body:
        items = "\n".join(f"  <li>{html.escape(n)}.</li>" for n in release_notes(release["version"]))
        body = f"<h2>{release['version']} — {date_ru(release['date'])}</h2>\n<ul>\n{items}\n</ul>\n\n" + body
        print(f"«Что нового»: добавлена версия {release['version']}")
    path.write_text(head + "\n---\n" + body, encoding="utf-8", newline="\n")


def date_home_pages(release: dict):
    """Version and date on the hand-written home pages: the visible line and their JSON-LD."""
    for path, lang in HAND_WRITTEN:
        file = OUT / path / "index.html"
        text = file.read_text(encoding="utf-8")
        shown = f"Версия {release['version']} · {date_ru(release['date'])}" if lang == "ru" else f"Version {release['version']} · {date_en(release['date'])}"
        text = re.sub(r"(<span data-release>).*?(</span>)", rf"\g<1>{shown}\g<2>", text)
        text = re.sub(r'("softwareVersion": )"[^"]*"', rf'\g<1>"{release["version"]}"', text)
        text = re.sub(r'("dateModified": )"[^"]*"', rf'\g<1>"{release["date"]}"', text)
        file.write_text(text, encoding="utf-8", newline="")


def to_text(fragment: str) -> str:
    """Page HTML as plain Markdown-like text for llms-full.txt."""
    t = fragment
    t = re.sub(r"<script.*?</script>", "", t, flags=re.S)
    t = re.sub(r"<h2[^>]*>(.*?)</h2>", r"\n## \1\n", t, flags=re.S)
    t = re.sub(r"<h3[^>]*>(.*?)</h3>", r"\n### \1\n", t, flags=re.S)
    t = re.sub(r"<summary>(.*?)</summary>", r"\n### \1\n", t, flags=re.S)
    t = t.replace('<span class="sub">', " — ")
    t = re.sub(r"<li[^>]*>", "- ", t)
    t = re.sub(r"</t[hd]>\s*<t[hd][^>]*>", " | ", t)
    t = re.sub(r"<a [^>]*href=\"(http[^\"]+)\"[^>]*>(.*?)</a>", r"\2 (\1)", t, flags=re.S)
    t = re.sub(r"<(p|div|tr|ul|ol|table|details|figure)[^>]*>", "\n", t)
    t = re.sub(r"<[^>]+>", "", t)
    t = html.unescape(t)
    t = re.sub(r"[ \t]+", " ", t)
    t = re.sub(r"\n\s*\n\s*\n+", "\n\n", t)
    return "\n".join(line.strip() for line in t.splitlines()).strip()


def write_llms(pages: dict, release: dict):
    """llms.txt (llmstxt.org): what Zyntra is and where everything is; llms-full.txt: all text."""
    guides = [p for p in pages.values() if p["path"] not in ("changelog/",)]
    lines = [
        "# Zyntra Launcher",
        "",
        "> Zyntra Launcher — бесплатный лаунчер Minecraft с открытым кодом (MIT) на Rust для Windows 10/11 и macOS на Apple Silicon (M1–M4). "
        "Все версии Minecraft, игра с офлайн-профилем без аккаунта Microsoft, моды и шейдеры из каталога Modrinth, скины с 3D-просмотром "
        "и Horizon — Minecraft 26.3, переписанный на Rust, с OptiFine-настройками. Без рекламы.",
        "",
        "Zyntra Launcher is a free, open-source (MIT) Minecraft launcher written in Rust for Windows and Apple Silicon Macs: every Minecraft version, "
        "offline profile, one-click Modrinth mods and shaders, 3D skins and Horizon (Minecraft 26.3 rewritten in Rust). No ads.",
        "",
        "Ключевые факты:",
        f"- Последняя версия: {release['version']} ({release['date']})",
        "- Цена: бесплатно, без рекламы и платных функций",
        "- Системы: Windows 10 и 11 (x64); macOS 11+ на Apple Silicon (M1, M2, M3, M4)",
        "- Размер установщика: около 6 МБ",
        "- Вход: офлайн-профиль с любым ником, аккаунт Microsoft не нужен",
        "- Версии: релизы, снапшоты, Beta, Alpha, Fabric; версии Forge, NeoForge и OptiFine из папки TLauncher",
        "- Моды и шейдеры: каталог Modrinth, зависимости ставятся автоматически",
        "- Память: подбирается автоматически — половина ОЗУ, от 2 до 8 ГБ",
        f"- Исходный код: {REPO} (лицензия MIT)",
        "- Неофициальный проект, не связан с Mojang Studios и Microsoft",
        "",
        "## Скачать",
        f"- [Установщик для Windows]({RELEASES}ZyntraSetup-Windows.exe): ZyntraSetup-Windows.exe, последняя версия",
        f"- [Приложение для Mac]({RELEASES}ZyntraLauncher-macOS.dmg): ZyntraLauncher-macOS.dmg, Apple Silicon",
        f"- [Все версии]({REPO}/releases): история релизов на GitHub",
        "",
        "## Страницы",
        f"- [Главная]({BASE}): что умеет лаунчер, скачивание, частые вопросы",
        f"- [English]({BASE}en/): the home page in English",
    ]
    lines += [f"- [{p['crumb']}]({BASE}{p['path']}): {p['description']}" for p in guides]
    lines += ["", "## Optional", f"- [Что нового]({BASE}changelog/): история версий", f"- [Полный текст сайта]({BASE}llms-full.txt)", ""]
    (OUT / "llms.txt").write_text("\n".join(lines), encoding="utf-8", newline="\n")

    full = ["# Zyntra Launcher — полный текст сайта", "", f"Источник: {BASE} · версия {release['version']} · {release['date']}", ""]
    home = (OUT / "index.html").read_text(encoding="utf-8")
    full += ["", f"# Главная ({BASE})", "", to_text(home.split("<main>", 1)[1].split("</main>", 1)[0])]
    for p in pages.values():
        full += ["", "", f"# {p['h1']} ({BASE}{p['path']})", "", f"Обновлено: {p['updated']}", "", to_text(p["lead"]), "", to_text(p["body"].replace("{up}", BASE).replace("{releases}", RELEASES).replace("{repo}", REPO))]
    (OUT / "llms-full.txt").write_text("\n".join(full) + "\n", encoding="utf-8", newline="\n")
    print("site/llms.txt, site/llms-full.txt")


def main():
    release = json.loads(RELEASE_FILE.read_text(encoding="utf-8"))
    if "--release" in sys.argv:
        version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
        release = {"version": version, "date": datetime.date.today().isoformat()}
        RELEASE_FILE.write_text(json.dumps(release, indent=2) + "\n", encoding="utf-8", newline="\n")
        add_to_changelog(release)
        print(f"Релиз {version} от {release['date']}")
    date_home_pages(release)

    pages = {}
    for f in sorted(SRC.glob("*.html")):
        page = parse(f)
        pages[page["path"]] = page
    for page in pages.values():
        out = OUT / page["path"] / "index.html"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(render(page, pages), encoding="utf-8", newline="\n")
        print("site/" + page["path"])
    write_llms(pages, release)

    urls = [(path, release["date"]) for path, _ in HAND_WRITTEN] + [(p["path"], p["updated"]) for p in pages.values()]
    sitemap = ['<?xml version="1.0" encoding="UTF-8"?>', '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">']
    for path, date in urls:
        sitemap.append(f"  <url>\n    <loc>{BASE}{path}</loc>\n    <lastmod>{date}</lastmod>\n  </url>")
    sitemap.append("</urlset>\n")
    (OUT / "sitemap.xml").write_text("\n".join(sitemap), encoding="utf-8", newline="\n")
    print(f"site/sitemap.xml ({len(urls)} pages)")


if __name__ == "__main__":
    main()
