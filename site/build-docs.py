#!/usr/bin/env python3
"""Turn the repository's Markdown docs into pages on kaviri.dev.

The docs live next to the code in `docs/`, because that is where they get updated when the
code changes and a separate docs repository is a docs repository that goes stale. This script
is the one place that knows how to put them on the web, and it runs before a deploy rather
than at request time: a Worker that renders Markdown per request is a rendering cost on every
page view for content that changes when someone commits.

    python3 site/build-docs.py      # writes site/docs/

Everything it writes is generated, so `site/docs/` is not in the repository.
"""

import html
import os
import re
import sys

try:
    import markdown
except ImportError:  # pragma: no cover - the message is the whole value
    sys.exit("build-docs: needs the markdown package (pip install --user markdown)")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "site", "docs")

# Order matters: this is the order they appear in the index and in the footer of each page,
# and it is the order somebody reading all of them should read them in.
PAGES = [
    ("index", "README.md", "kaviri", "What it is, what a script looks like, and how to run one."),
    ("script-protocol", "docs/script-protocol.md", "Script protocol",
     "Every op, every field, and what each one does to the camera."),
    ("camera", "docs/camera.md", "The camera",
     "How a zoom and a pan are derived from interactions you never describe."),
    ("ci", "docs/ci.md", "In CI",
     "The GitHub Action, and filming an app the workflow starts itself."),
    ("troubleshooting", "docs/troubleshooting.md", "Troubleshooting",
     "What each error means, starting with the ones that produce a video rather than a failure."),
    ("agents", "AGENTS.md", "For agents",
     "The whole protocol for something that has been asked for a video and has a shell."),
    ("audio", "docs/audio.md", "Sound and captions",
     "Narration, a music bed that ducks under it, and captions timed to the words."),
    ("mcp", "docs/MCP.md", "MCP server",
     "kaviri mcp: the op protocol as tools, plus inspect, background takes and previews."),
]

# ---------------------------------------------------------------------------- shared chrome
#
# The header and footer every page shares. The landing page, the playground and the legal
# pages are hand-written HTML, so rather than keep five copies in step by hand, build() writes
# these blocks into each of them (and into every docs page) on every run. Change the nav here.

V = "4"  # bumped with a redesign, so the hour-long CSS/JS cache does not show the old look

MARK = ('<svg class="mark" viewBox="0 0 100 100" width="26" height="26" aria-hidden="true">'
        '<rect x="8" y="8" width="62" height="62" rx="13" fill="none" stroke="#3ddc97" stroke-width="6" opacity="0.5"/>'
        '<rect x="30" y="30" width="62" height="62" rx="13" fill="#3ddc97"/>'
        '<path d="M53 48L53 74L75 61Z" fill="#0e1013" stroke="#0e1013" stroke-width="3" stroke-linejoin="round"/></svg>')

GITHUB = "https://github.com/kaviridev/kaviri"
GH_ICON = ('<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 '
           '6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 '
           '1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 '
           '.67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 '
           '3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>')

HEADER = f"""<header class="site-head" data-nav="closed">
  <div class="wrap head-in">
    <a class="wordmark" href="/">{MARK}kaviri</a>
    <button class="nav-toggle" type="button" id="nav-toggle"
            aria-expanded="false" aria-controls="site-nav" aria-label="Menu">
      <span class="bars" aria-hidden="true"></span>
    </button>
    <nav id="site-nav" aria-label="Main">
      <div class="nav-links">
        <a href="/#features">Product</a>
        <a href="/docs/">Docs</a>
        <a href="/play/">Playground</a>
        <a href="/docs/mcp/">For agents</a>
        <a href="/#pricing">Pricing</a>
        <span class="nav-cta">
          <a class="btn btn-ghost btn-sm gh-stars" href="{GITHUB}">{GH_ICON}GitHub</a>
          <a class="btn btn-primary btn-sm" href="/#waitlist">Join the beta</a>
        </span>
      </div>
    </nav>
  </div>
</header>"""

FOOTER = f"""<footer class="site-foot">
  <div class="wrap foot-grid">
    <div class="foot-brand">
      <a class="wordmark" href="/">{MARK}kaviri</a>
      <p>The open-source demo recorder for web apps that runs in your CI.</p>
    </div>
    <div class="foot-col">
      <h4>Product</h4>
      <a href="/#features">Features</a>
      <a href="/play/">Playground</a>
      <a href="/#pricing">Pricing</a>
      <a href="/#waitlist">Hosted beta</a>
    </div>
    <div class="foot-col">
      <h4>Developers</h4>
      <a href="/docs/">Docs</a>
      <a href="/docs/ci/">CI guide</a>
      <a href="/docs/mcp/">MCP server</a>
      <a href="/llms.txt">llms.txt</a>
      <a href="{GITHUB}">GitHub</a>
    </div>
    <div class="foot-col">
      <h4>Company</h4>
      <a href="/company/">Company</a>
      <a href="/privacy/">Privacy</a>
      <a href="/terms/">Terms</a>
      <a href="mailto:hello@kaviri.dev">hello@kaviri.dev</a>
    </div>
  </div>
  <div class="wrap foot-base">
    <span>&copy; 2026 kaviri &middot; <a href="{GITHUB}/blob/main/LICENSE">Apache 2.0</a></span>
    <span><span class="pron">ka-VEE-ree</span>: ChiShona for twice, a second time.</span>
  </div>
</footer>"""

STATIC_PAGES = ["index.html", "play/index.html", "company/index.html", "privacy/index.html", "terms/index.html"]


def chrome(page: str) -> str:
    """Put the shared header, footer and asset links into one page, idempotently."""
    page = re.sub(r'<header class="site-head[^"]*"[^>]*>.*?</header>|<!--HEADER-->', lambda m: HEADER, page, count=1, flags=re.S)
    page = re.sub(r'<footer class="site-foot[^"]*">.*?</footer>|<!--FOOTER-->', lambda m: FOOTER, page, count=1, flags=re.S)
    page = re.sub(r'href="/(brand/tokens|style|docs)\.css(\?v=\w+)?"', lambda m: f'href="/{m.group(1)}.css?v={V}"', page)
    page = re.sub(r'href="play\.css(\?v=\w+)?"', f'href="play.css?v={V}"', page)
    page = re.sub(r'\s*<script src="/site\.js[^"]*"></script>', "", page)
    page = page.replace("</head>", f'<script src="/site.js?v={V}"></script>\n</head>', 1)
    page = re.sub(r'\s*<script src="/nav\.js[^"]*"></script>', "", page)
    page = page.replace("</body>", f'<script src="/nav.js?v={V}"></script>\n</body>', 1)
    page = re.sub(r'src="/(player|waitlist)\.js(\?v=\w+)?"', lambda m: f'src="/{m.group(1)}.js?v={V}"', page)
    return page


def sync_static():
    for rel in STATIC_PAGES:
        path = os.path.join(ROOT, "site", rel)
        before = open(path, encoding="utf-8").read()
        after = seo(rel, chrome(before))
        if after != before:
            open(path, "w", encoding="utf-8").write(after)
            print(f"  /{rel.replace('index.html', '')}  header and footer synced")


# ------------------------------------------------------------------------------- search
#
# What search engines and answer engines read. It is generated for the same reason the chrome
# is: the FAQ structured data is built from the FAQ on the page, so the two cannot disagree,
# and the sitemap is built from the page list, so a new doc is in it without anyone remembering.

SITE = "https://kaviri.dev"
VERSION = re.search(r'^version\s*=\s*"([^"]+)"', open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8").read(), re.M).group(1)

ORG = {"@type": "Organization", "@id": SITE + "/#org", "name": "kaviri", "url": SITE + "/",
       "logo": SITE + "/apple-touch-icon.png", "email": "hello@kaviri.dev",
       "sameAs": [GITHUB, "https://github.com/kaviridev"]}
WEBSITE = {"@type": "WebSite", "@id": SITE + "/#website", "name": "kaviri", "url": SITE + "/",
           "inLanguage": "en", "publisher": {"@id": SITE + "/#org"}}
SOFTWARE = {
    "@type": "SoftwareApplication", "@id": SITE + "/#software", "name": "kaviri",
    "alternateName": "kaviri recording browser",
    "description": "An open-source recording browser that films product demos from a script in "
                   "your repository, in CI, with the camera zooms derived from the interactions.",
    "url": SITE + "/", "applicationCategory": "DeveloperApplication",
    "applicationSubCategory": "Screen recording",
    "operatingSystem": "Linux, macOS", "softwareVersion": VERSION,
    "license": "https://www.apache.org/licenses/LICENSE-2.0",
    "isAccessibleForFree": True, "codeRepository": GITHUB, "downloadUrl": GITHUB,
    "screenshot": SITE + "/demo-poster.jpg",
    "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
    "publisher": {"@id": SITE + "/#org"},
}


def jsonld(nodes) -> str:
    import json
    # </ cannot appear inside a script element, so the one way to end it early is escaped.
    return json.dumps({"@context": "https://schema.org", "@graph": nodes},
                      ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")


def strip_tags(s: str) -> str:
    return html.unescape(re.sub(r"\s+", " ", re.sub(r"<[^>]+>", "", s))).strip()


def faq(page: str):
    return [{"@type": "Question", "name": strip_tags(q),
             "acceptedAnswer": {"@type": "Answer", "text": strip_tags(a)}}
            for q, a in re.findall(r"<summary>(.*?)</summary>\s*<p>(.*?)</p>", page, re.S)]


# Per static page: the structured data it carries, and the title and description for the
# social cards when the page does not already have its own.
def videos(page: str):
    """A VideoObject per <video> with a poster, so the take can surface in video results."""
    import subprocess
    out = []
    for tag in re.findall(r"<video\b[^>]*>", page):
        src, poster, label = (re.search(rf'{a}="([^"]*)"', tag) for a in ("src", "poster", "aria-label"))
        if not (src and poster and label):
            continue
        node = {"@type": "VideoObject", "name": strip_tags(label.group(1)).split(":")[0],
                "description": strip_tags(label.group(1)), "contentUrl": SITE + src.group(1),
                "thumbnailUrl": SITE + poster.group(1), "publisher": {"@id": SITE + "/#org"}}
        local = os.path.join(ROOT, "site", src.group(1).lstrip("/"))
        try:
            secs = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                                         "-of", "csv=p=0", local], capture_output=True, text=True).stdout)
            node["duration"] = f"PT{round(secs)}S"
            stamp = subprocess.run(["git", "log", "-1", "--format=%cI", "--", local], cwd=ROOT,
                                   capture_output=True, text=True).stdout.strip()
            if not stamp:  # not committed yet: the file's own time is the honest answer
                import datetime
                stamp = datetime.datetime.fromtimestamp(os.path.getmtime(local), datetime.timezone.utc).isoformat(timespec="seconds")
            node["uploadDate"] = stamp
        except (OSError, ValueError):
            pass
        out.append(node)
    return out


def static_ld(rel: str, page: str):
    url = SITE + "/" + rel.replace("index.html", "")
    if rel == "index.html":
        return [ORG, WEBSITE, SOFTWARE,
                {"@type": "FAQPage", "@id": url + "#faq", "url": url, "mainEntity": faq(page)}] + videos(page)
    name = strip_tags(re.search(r"<title>(.*?)</title>", page, re.S).group(1))
    node = {"@type": "WebPage", "@id": url, "url": url, "name": name,
            "isPartOf": {"@id": SITE + "/#website"}, "publisher": {"@id": SITE + "/#org"}}
    if rel.startswith("play/"):
        node["@type"] = ["WebPage", "WebApplication"]
        node.update({"applicationCategory": "DeveloperApplication", "browserRequirements": "Requires JavaScript",
                     "operatingSystem": "Any", "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"}})
    if rel.startswith("company/"):
        node["@type"] = "AboutPage"
    return [ORG, node]


def seo(rel: str, page: str) -> str:
    """Canonical, social cards and structured data for one hand-written page, idempotently."""
    url = SITE + "/" + rel.replace("index.html", "")
    title = re.search(r"<title>(.*?)</title>", page, re.S).group(1)
    m = re.search(r'<meta name="description" content="([^"]*)"', page)
    desc = m.group(1) if m else ""
    head = []
    if 'rel="canonical"' not in page:
        head.append(f'<link rel="canonical" href="{url}">')
    if 'property="og:title"' not in page:
        head += [f'<meta property="og:type" content="website">',
                 f'<meta property="og:site_name" content="kaviri">',
                 f'<meta property="og:url" content="{url}">',
                 f'<meta property="og:title" content="{title}">',
                 f'<meta property="og:description" content="{desc}">',
                 f'<meta property="og:image" content="{SITE}/og.png">',
                 '<meta name="twitter:card" content="summary_large_image">',
                 f'<meta name="twitter:image" content="{SITE}/og.png">']
    if 'rel="manifest"' not in page:
        head.append('<link rel="manifest" href="/site.webmanifest">')
    if head:
        page = re.sub(r'(<meta name="description"[^>]*>|<title>.*?</title>)',
                      lambda m: m.group(1) + "\n" + "\n".join(head), page, count=1, flags=re.S)
    block = f'<script type="application/ld+json">{jsonld(static_ld(rel, page))}</script>'
    if 'type="application/ld+json"' in page:
        page = re.sub(r'<script type="application/ld\+json">.*?</script>', lambda m: block, page, count=1, flags=re.S)
    else:
        page = page.replace("</head>", block + "\n</head>", 1)
    return page


def write_search_files():
    """sitemap.xml and robots.txt, from the same page lists the build already walks."""
    import datetime, subprocess
    def lastmod(path):
        try:
            out = subprocess.run(["git", "log", "-1", "--format=%cs", "--", path], cwd=ROOT,
                                 capture_output=True, text=True).stdout.strip()
        except OSError:
            out = ""
        return out or datetime.date.today().isoformat()
    urls = [(SITE + "/" + rel.replace("index.html", ""), lastmod("site/" + rel)) for rel in STATIC_PAGES]
    urls += [(SITE + ("/docs/" if slug == "index" else f"/docs/{slug}/"), lastmod(path))
             for slug, path, _, _ in PAGES if os.path.exists(os.path.join(ROOT, path))]
    body = "".join(f"  <url><loc>{u}</loc><lastmod>{d}</lastmod></url>\n" for u, d in urls)
    open(os.path.join(ROOT, "site", "sitemap.xml"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' + body + "</urlset>\n")
    # Every crawler, search and AI alike, is welcome: the docs are written to be quoted, and an
    # answer engine that cannot read them answers from a stale guess instead.
    open(os.path.join(ROOT, "site", "robots.txt"), "w", encoding="utf-8").write(
        "User-agent: *\nAllow: /\nDisallow: /admin\nDisallow: /api/\nDisallow: /unsubscribe\n\n"
        f"Sitemap: {SITE}/sitemap.xml\n")
    print(f"  sitemap.xml  {len(urls)} urls; robots.txt")


SHELL = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} &middot; kaviri docs</title>
<meta name="description" content="{blurb}">
<link rel="canonical" href="{url}">
<meta property="og:type" content="article">
<meta property="og:site_name" content="kaviri">
<meta property="og:url" content="{url}">
<meta property="og:title" content="{title} &middot; kaviri docs">
<meta property="og:description" content="{blurb}">
<meta property="og:image" content="{site}/og.png">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="{site}/og.png">
<script type="application/ld+json">{ld}</script>
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
<link rel="stylesheet" href="/brand/tokens.css">
<link rel="stylesheet" href="/style.css">
<link rel="stylesheet" href="/docs.css">
</head>
<body>
<header class="site-head wrap">
  <a class="wordmark" href="/"><svg class="mark" viewBox="0 0 100 100" width="26" height="26" aria-hidden="true"><rect x="8" y="8" width="62" height="62" rx="13" fill="none" stroke="#3ddc97" stroke-width="6" opacity="0.5"/><rect x="30" y="30" width="62" height="62" rx="13" fill="#3ddc97"/><path d="M53 48L53 74L75 61Z" fill="#0e1013" stroke="#0e1013" stroke-width="3" stroke-linejoin="round"/></svg>kaviri</a>
  <nav>
    <a href="/docs/">Docs</a>
    <a href="/play/">Playground</a>
    <a href="https://github.com/kaviridev/kaviri">GitHub</a>
  </nav>
</header>
<div class="wrap docs-shell">
  <nav class="docs-side" aria-label="Documentation">
    <p class="eyebrow">Docs</p>
    <ul>{side}</ul>
    <p class="eyebrow" style="margin-top:var(--k-space-3)">Plain text</p>
    <ul>
      <li><a href="/llms.txt">llms.txt</a></li>
      <li><a href="/llms-full.txt">llms-full.txt</a></li>
    </ul>
  </nav>
  <main class="docs-main">
{body}
    <hr class="rule">
    <p class="muted docs-foot">
      This page is generated from <code>{source}</code> in the repository, so it cannot drift
      from the code it documents without somebody noticing.
    </p>
  </main>
</div>
<footer class="site-foot wrap">
  <p class="pron">kaviri &middot; ka-VEE-ree</p>
  <p>ChiShona for twice, a second time. The demo is not re-recorded. It is made again.</p>
  <div class="foot-links">
    <a href="/">Home</a>
    <a href="/docs/">Docs</a>
    <a href="/play/">Playground</a>
    <a href="https://github.com/kaviridev/kaviri">GitHub</a>
    <a href="mailto:hello@kaviri.dev">hello@kaviri.dev</a>
  </div>
  <p class="muted legal-line">
    <a href="/company/">Company</a> &middot; <a href="/privacy/">Privacy</a> &middot; <a href="/terms/">Terms</a>
  </p>
</footer>
</body>
</html>
"""


def rewrite_links(body: str) -> str:
    """Point the repository's relative links at the published pages.

    A doc that says `docs/camera.md` is correct on GitHub and a 404 here. The mapping is the
    same list the index is built from, so a new page is wired up by adding one line to PAGES
    rather than by remembering to also edit a regex.
    """
    body = body.replace('src="docs/media/', 'src="/docs/media/')
    for slug, path, _, _ in PAGES:
        target = "/docs/" if slug == "index" else f"/docs/{slug}/"
        for spelling in (path, os.path.basename(path), "./" + path):
            body = body.replace(f'href="{spelling}"', f'href="{target}"')
    return body


def build():
    os.makedirs(OUT, exist_ok=True)
    # Images the Markdown points at (docs/media/...) are published next to the pages, so a
    # README that renders on GitHub renders the same here.
    media = os.path.join(ROOT, "docs", "media")
    if os.path.isdir(media):
        import shutil
        shutil.copytree(media, os.path.join(OUT, "media"), dirs_exist_ok=True)
    md = markdown.Markdown(
        extensions=["fenced_code", "tables", "toc", "attr_list", "sane_lists"],
        output_format="html5",
    )

    written = []
    for slug, path, title, blurb in PAGES:
        src = os.path.join(ROOT, path)
        if not os.path.exists(src):
            print(f"build-docs: skipping {path}, which does not exist")
            continue
        text = open(src, encoding="utf-8").read()
        md.reset()
        body = rewrite_links(md.convert(text))

        side = "".join(
            f'<li{" class=here" if s == slug else ""}><a href="{"/docs/" if s == "index" else "/docs/" + s + "/"}">{html.escape(t)}</a></li>'
            for s, _, t, _ in PAGES
        )

        url = SITE + ("/docs/" if slug == "index" else f"/docs/{slug}/")
        ld = jsonld([
            {"@type": "TechArticle", "headline": title, "description": blurb, "url": url,
             "inLanguage": "en", "isPartOf": {"@id": SITE + "/#website"},
             "about": {"@id": SITE + "/#software"}, "publisher": {"@id": SITE + "/#org"}},
            {"@type": "BreadcrumbList", "itemListElement": [
                {"@type": "ListItem", "position": 1, "name": "kaviri", "item": SITE + "/"},
                {"@type": "ListItem", "position": 2, "name": "Docs", "item": SITE + "/docs/"},
            ] + ([] if slug == "index" else
                 [{"@type": "ListItem", "position": 3, "name": title, "item": url}])},
        ])
        page = chrome(SHELL.format(
            url=url, site=SITE, ld=ld,
            title=html.escape(title),
            blurb=html.escape(blurb),
            side=side,
            body=body,
            source=html.escape(path),
        ))
        # index.md becomes /docs/, everything else becomes /docs/<slug>, which the asset
        # store serves from <slug>/index.html with its trailing-slash handling.
        dest = os.path.join(OUT, "index.html") if slug == "index" else os.path.join(OUT, slug, "index.html")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        open(dest, "w", encoding="utf-8").write(page)
        written.append((slug, len(page)))

    for slug, n in written:
        print(f"  /docs/{'' if slug == 'index' else slug + '/'}  {n:6} bytes")
    print(f"build-docs: {len(written)} pages into site/docs/")


if __name__ == "__main__":
    sync_static()
    build()
    write_search_files()
