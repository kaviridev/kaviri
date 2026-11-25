/*
 * The trending pipeline: something people are already talking about, filmed by kaviri.
 *
 *   06:xx UTC  the hourly cron gathers what is trending (GitHub, Hugging Face, Hacker News),
 *              Workers AI picks the one most worth filming and writes the hook and the captions.
 *   06:40 UTC  .github/workflows/trend.yml claims the pick, films the script built here, and
 *              sends the video back with the claim code it was given (no secret on the runner).
 *   then       the video goes to Postiz's media library and the owner gets an email with it,
 *              the captions, and one button. Nothing is posted until that button is pressed.
 *
 * The model only chooses among real candidates and writes words. The URL it films and the
 * script that films it come from here, so a model that invents a link cannot make kaviri
 * navigate to it.
 */

import { notifyOwner } from "./mail.js";
import { render, h, rows, note, button, status } from "./brand-email.js";
import { tagPosts, postizApi, mediaUrl } from "./social.js";

export const TREND_HOUR_UTC = 6;
const MODEL = "@cf/meta/llama-3.3-70b-instruct-fp8-fast";
const UA = { "User-Agent": "kaviri-trends (+https://kaviri.dev)", Accept: "application/json" };
// The caption is about the thing; kaviri is credited in a first comment under it.
const COMMENT = "Filmed by a script with kaviri: kaviri.dev";

/*
 * The channels, by Postiz integration id. TikTok goes as an inbox draft (UPLOAD) because the
 * app is not audited for direct posting yet; the owner finishes it in the TikTok app.
 */
const CHANNELS = [
  { id: "cmwgcgd9l0001o97hyvjfvnw8", type: "x", settings: { who_can_reply_post: "everyone" } },
  { id: "cmwgckobv0009o97h2ikyz1n2", type: "bluesky", settings: {} },
  { id: "cmwgcjyxv0007o97h87tf1oj2", type: "mastodon", settings: {} },
  { id: "cmwgcgvb50003o97hmqv9gdl1", type: "linkedin-page", settings: { post_as_images_carousel: false } },
  { id: "cmwgchshx0005o97hq8n66kdh", type: "youtube", settings: { type: "public", selfDeclaredMadeForKids: "no" }, title: true },
  {
    id: "cmwgculb8000bo97h5udltss8", type: "tiktok", title: true,
    settings: {
      content_posting_method: "UPLOAD", privacy_level: "PUBLIC_TO_EVERYONE", duet: false, stitch: false,
      comment: true, autoAddMusic: "no", brand_content_toggle: false, brand_organic_toggle: false,
    },
  },
];

export async function ensureTrendSchema(env) {
  await env.kaviri_waitlist
    .prepare(
      `create table if not exists trends (
         id integer primary key autoincrement,
         created_at text not null default (datetime('now')),
         day text not null,
         source text not null,
         title text not null,
         url text not null,
         kind text not null,
         stat text,
         stat_sub text,
         hook1 text not null,
         hook2 text,
         caption text not null,
         short_title text not null,
         why text,
         status text not null default 'ready',
         claim text,
         claimed_at text,
         media_id text,
         media_path text,
         posted_at text,
         note text
       )`
    )
    .run();
}

// ---------------------------------------------------------------- gathering

async function json(url) {
  const r = await fetch(url, { headers: UA, cf: { cacheTtl: 0 } });
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return r.json();
}

const fmt = (n) => Number(n).toLocaleString("en-US");

async function fromGitHub() {
  const since = new Date(Date.now() - 10 * 864e5).toISOString().slice(0, 10);
  const d = await json(`https://api.github.com/search/repositories?q=created:>${since}&sort=stars&order=desc&per_page=12`);
  return (d.items || []).map((r) => ({
    source: "github",
    kind: "github",
    title: `${r.full_name}: ${r.description || ""}`.slice(0, 200),
    url: r.html_url,
    stat: `${fmt(r.stargazers_count)} stars`,
    stat_sub: `on GitHub, ${Math.max(1, Math.round((Date.now() - Date.parse(r.created_at)) / 864e5))} days after it went up`,
  }));
}

async function fromHuggingFace() {
  const d = await json("https://huggingface.co/api/models?sort=trendingScore&direction=-1&limit=10");
  return (d || []).map((m) => ({
    source: "huggingface",
    kind: "huggingface",
    title: `${m.id} (${m.pipeline_tag || "model"})`,
    url: `https://huggingface.co/${m.id}`,
    stat: m.likes ? `${fmt(m.likes)} likes` : null,
    stat_sub: "on Hugging Face",
  }));
}

async function fromHackerNews() {
  const ids = (await json("https://hacker-news.firebaseio.com/v0/topstories.json")).slice(0, 15);
  const items = await Promise.all(ids.map((id) => json(`https://hacker-news.firebaseio.com/v0/item/${id}.json`).catch(() => null)));
  return items
    .filter((i) => i && i.url && /^https:\/\//.test(i.url) && !/\.pdf($|\?)/i.test(i.url))
    .map((i) => ({
      source: "hackernews",
      kind: /^https:\/\/github\.com\/[^/]+\/[^/]+\/?$/.test(i.url) ? "github" : "site",
      title: i.title,
      url: i.url,
      stat: `${fmt(i.score)} points`,
      stat_sub: "on Hacker News today",
    }));
}

export async function gather() {
  const parts = await Promise.allSettled([fromGitHub(), fromHuggingFace(), fromHackerNews()]);
  const all = parts.flatMap((p) => (p.status === "fulfilled" ? p.value : []));
  const seen = new Set();
  return all.filter((c) => (seen.has(c.url) ? false : seen.add(c.url)));
}

// ---------------------------------------------------------------- picking

const SYSTEM = `You pick one trending thing for kaviri's social accounts to film today.
kaviri is a browser that films web pages from a script, used for product demo videos.
The audience is developers and people who build with AI.
Pick the candidate that the most of them would stop scrolling for: a new AI model, a dev tool,
an open-source project blowing up, something visual or surprising.
Never pick: piracy, cracks, leaks, cheats, emulating or running games or software people have
not bought, malware or "unlocked" anything, crypto tokens, politics, tragedy, crime, or
people's private lives. Never pick anything that needs a login or is mostly a wall of text.
Prefer github and huggingface candidates, because their pages film well.

The hooks are the two punch cards that open the video, big white words on black. Hook 1 is
the surprising claim, hook 2 the payoff that makes you watch. Each is 2 to 5 words and reads
as a sentence. Good: "A 7B model" / "beat GPT-6 at code." Good: "One file." / "A whole database."
Bad: labels like "AI Tool" or "Game Mod", or anything with a colon.
Use only facts in the candidate's own text. Never invent a number, a speed-up, a benchmark or a
comparison it does not state; a made-up claim on a real project's video is the worst outcome.
The caption leads with the outcome, the way a developer would tell a friend, under 240
characters. Do not mention kaviri in it; kaviri goes in a separate first comment.
Write plainly. No hashtags. No em dashes. No exclamation marks. Never speak for the project.
Answer with JSON only:
{"index": <number of the candidate>,
 "hook1": "...", "hook2": "...",
 "short_title": "<YouTube/TikTok title, under 70 characters, the outcome first>",
 "caption": "...",
 "why": "<one sentence, for the owner, on why this one>"}`;

function parseJson(text) {
  if (text && typeof text === "object") return text;
  const m = String(text || "").match(/\{[\s\S]*\}/);
  if (!m) throw new Error(`model gave no JSON: ${String(text).slice(0, 200)}`);
  return JSON.parse(m[0]);
}

const clean = (s, n) => String(s || "").replace(/—/g, ", ").replace(/\s+/g, " ").trim().slice(0, n);

export async function pick(env, candidates) {
  if (!env.AI) throw new Error("no AI binding");
  const list = candidates.map((c, i) => `${i}. [${c.source}] ${c.title} (${c.stat || "no stat"})`).join("\n");
  const r = await env.AI.run(MODEL, {
    messages: [
      { role: "system", content: SYSTEM },
      { role: "user", content: `Candidates:\n${list}` },
    ],
    max_tokens: 500,
    temperature: 0.4,
  });
  const p = parseJson(r.response ?? r);
  const c = candidates[Number(p.index)];
  if (!c) throw new Error(`model picked ${p.index}, which is not a candidate`);
  // kaviri stays out of the caption (it goes in the first comment, see COMMENT), whatever
  // the model did.
  let caption = clean(p.caption, 300)
    .replace(/\s*filmed by a script.*$/i, "")
    .replace(/\s*kaviri\.dev\s*$/i, "")
    .replace(/[\s,;:]+$/, "");
  if (caption && !/[.?]$/.test(caption)) caption += ".";
  return {
    ...c,
    hook1: clean(p.hook1, 40) || "Trending today.",
    hook2: clean(p.hook2, 40) || null,
    short_title: clean(p.short_title, 90) || clean(c.title, 90),
    caption,
    why: clean(p.why, 300),
  };
}

// ---------------------------------------------------------------- the script

/** The take, as kaviri ops. Built here from the kind, never from model output beyond words. */
export function script(t) {
  const ops = [
    { op: "frame", platform: "ios", style: "recording" },
    { op: "navigate", url: t.url },
    { op: "wait", ms: 4000 },
    { op: "title", text: t.hook1, style: "punch", transition: "cut", ms: 1000 },
  ];
  if (t.hook2) ops.push({ op: "title", text: t.hook2, style: "punch", transition: "cut", ms: 1000 });
  ops.push({ op: "start_recording" }, { op: "wait", ms: 1500 });
  // scroll's y is a position from the top of the page, not a distance, so each step goes
  // further down. On a repo the first step passes the file list and lands in the README.
  const stops = t.kind === "github" ? [1300, 2000, 2700] : t.kind === "huggingface" ? [600, 1300, 2000] : [700, 1400];
  for (const y of stops) ops.push({ op: "scroll", y, smooth: true }, { op: "wait", ms: 1600 });
  if (t.stat) ops.push({ op: "title", text: t.stat, sub: t.stat_sub || "", style: "stat" });
  ops.push({ op: "title", style: "end", at: "end" }, { op: "stop_recording" });
  return ops;
}

// ---------------------------------------------------------------- the daily run

const today = () => new Date().toISOString().slice(0, 10);

export async function runTrends(env, { force = false } = {}) {
  await ensureTrendSchema(env);
  const day = today();
  if (!force) {
    const had = await env.kaviri_waitlist.prepare("select id from trends where day = ? limit 1").bind(day).first();
    if (had) return { skipped: "already picked today", id: had.id };
  }
  const posted = await env.kaviri_waitlist
    .prepare("select url from trends where created_at >= datetime('now', '-30 days')")
    .all();
  const recent = new Set((posted.results || []).map((r) => r.url));
  const candidates = (await gather()).filter((c) => !recent.has(c.url));
  if (!candidates.length) return { skipped: "no candidates" };
  const t = await pick(env, candidates);
  const row = await env.kaviri_waitlist
    .prepare(
      `insert into trends (day, source, title, url, kind, stat, stat_sub, hook1, hook2, caption, short_title, why)
       values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) returning id`
    )
    .bind(day, t.source, t.title, t.url, t.kind, t.stat, t.stat_sub, t.hook1, t.hook2, t.caption, t.short_title, t.why)
    .first();
  return { id: row.id, title: t.title, url: t.url, candidates: candidates.length };
}

// ---------------------------------------------------------------- the runner's side

function code() {
  const b = new Uint8Array(18);
  crypto.getRandomValues(b);
  return btoa(String.fromCharCode(...b)).replace(/[+/=]/g, "");
}

const out = (body, status = 200) =>
  new Response(body == null ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
  });

/**
 * GET /api/trends/next claims the oldest pick that is ready to film and hands back its script
 * and a claim code. POST /api/trends/<id>/video with that code delivers the film. A claim that
 * never delivers is offered again after 45 minutes (a run takes about 15).
 */
export async function api(request, env, url, ctx) {
  await ensureTrendSchema(env);
  if (url.pathname === "/api/trends/next" && request.method === "GET") {
    const t = await env.kaviri_waitlist
      .prepare(
        `select * from trends where status = 'ready'
            or (status = 'claimed' and claimed_at < datetime('now', '-45 minutes'))
         order by id limit 1`
      )
      .first();
    if (!t) return out(null, 204);
    const claim = code();
    await env.kaviri_waitlist
      .prepare("update trends set status = 'claimed', claim = ?, claimed_at = datetime('now') where id = ?")
      .bind(claim, t.id)
      .run();
    return out({ id: t.id, claim, script: script(t) });
  }
  const m = url.pathname.match(/^\/api\/trends\/(\d+)\/video$/);
  if (m && request.method === "POST") {
    const id = Number(m[1]);
    const t = await env.kaviri_waitlist.prepare("select * from trends where id = ?").bind(id).first();
    const claim = request.headers.get("x-claim") || "";
    if (!t || t.status !== "claimed" || !t.claim || claim !== t.claim) return out({ error: "no such claim" }, 403);
    const form = await request.formData();
    const file = form.get("video");
    if (!file || typeof file === "string") return out({ error: "send the film as the video field" }, 400);
    if (file.size > 60e6) return out({ error: "film is over 60 MB" }, 413);
    const media = await uploadToPostiz(env, file);
    await env.kaviri_waitlist
      .prepare("update trends set status = 'filmed', claim = null, media_id = ?, media_path = ? where id = ?")
      .bind(media.id, media.path, id)
      .run();
    ctx.waitUntil(reviewEmail(env, { ...t, media_path: media.path }).catch(() => {}));
    return out({ ok: true, media: media.path });
  }
  if (m && request.method === "GET") return out({ error: "POST the film here" }, 405);
  return out({ error: "not found" }, 404);
}

async function uploadToPostiz(env, file) {
  if (!env.POSTIZ_API_KEY) throw new Error("POSTIZ_API_KEY is not set on the worker");
  const fd = new FormData();
  fd.append("file", file, "trend.mp4");
  const r = await fetch(`${postizApi(env)}/upload`, { method: "POST", headers: { Authorization: env.POSTIZ_API_KEY }, body: fd });
  if (!r.ok) throw new Error(`postiz upload: ${r.status} ${await r.text()}`);
  const j = await r.json();
  if (!j.path) throw new Error("postiz upload gave no path");
  return j;
}

// ---------------------------------------------------------------- the owner's side

/*
 * One click from the email. The link is signed with the admin secret over the pick and its
 * film, so it is good for that film only, and posting moves the pick out of "filmed", so it is
 * good once. Opening it does not post by itself: mail scanners open links to check them, so the
 * page the link opens is what posts, from a script, and scanners do not run scripts.
 */
const enc = new TextEncoder();
const LINK_DAYS = 3;

async function sign(env, id, action, media) {
  const key = await crypto.subtle.importKey("raw", enc.encode(env.ADMIN_PASSWORD || ""), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(`trend:${id}:${action}:${media}`)));
  return btoa(String.fromCharCode(...mac)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function link(env, t, action) {
  return `https://kaviri.dev/t/${t.id}/${action}/${await sign(env, t.id, action, t.media_id)}`;
}

async function reviewEmail(env, t) {
  const subject = `kaviri trend ready: ${t.short_title}`;
  const blocks = [
    status("A trending video is filmed and waiting for you", "good"),
    h(t.short_title),
    button("Watch the video", mediaUrl(t.media_path, true)),
    note(`Caption: ${t.caption}`),
    note(`First comment: ${COMMENT}`),
    rows([
      ["Cards", [t.hook1, t.hook2].filter(Boolean).join(" / ")],
      ["Topic", t.title],
      ["Source", t.url],
      ["Why", t.why || ""],
    ]),
    button("Post everywhere", await link(env, t, "post")),
    note(`One click posts to X, Bluesky, Mastodon, LinkedIn and YouTube, and sends TikTok to your TikTok inbox. Not this one: ${await link(env, t, "skip")} . Links work once and expire in ${LINK_DAYS} days.`),
  ];
  const { html, text } = render({ preheader: `${t.title}`, blocks, signature: false });
  return notifyOwner(env, { subject, text, html });
}

const page = (title, body) =>
  new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title}</title><link rel="icon" href="/favicon.svg"><link rel="stylesheet" href="/brand/tokens.css"><link rel="stylesheet" href="/style.css"><link rel="stylesheet" href="/admin.css">
</head><body><div class="wrap adm">${body}</div></body></html>`,
    { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store", "X-Robots-Tag": "noindex" } }
  );

// The script the one-click page runs: POST the same URL, show what came back.
export const GO_JS = `(async () => {
  const s = document.getElementById("state");
  try {
    const r = await fetch(location.pathname, { method: "POST" });
    const j = await r.json();
    s.textContent = j.message || j.error || "Done.";
    s.className = r.ok ? "note" : "err";
  } catch (e) { s.textContent = "Could not reach kaviri.dev: " + e; s.className = "err"; }
})();`;

const escHtml = (x) => String(x ?? "").replace(/[<>&"]/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;" })[c]);

export async function oneClick(request, env, url) {
  if (url.pathname === "/t/go.js") {
    return new Response(GO_JS, { headers: { "Content-Type": "text/javascript; charset=utf-8", "Cache-Control": "no-store" } });
  }
  const m = url.pathname.match(/^\/t\/(\d+)\/(post|skip)\/([A-Za-z0-9_-]{20,})$/);
  if (!m) return page("Not found", "<h1>Not found</h1>");
  const [, idS, action, sig] = m;
  const id = Number(idS);
  await ensureTrendSchema(env);
  const t = await env.kaviri_waitlist.prepare("select * from trends where id = ?").bind(id).first();
  const good = t && t.media_id && sig === (await sign(env, id, action, t.media_id));
  const fresh = t && Date.parse(`${t.claimed_at || t.created_at}Z`) > Date.now() - LINK_DAYS * 864e5;
  const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });

  if (request.method === "POST") {
    if (!good) return json({ error: "This link is not valid." }, 403);
    if (!fresh) return json({ error: "This link has expired. Use kaviri.dev/admin/trends." }, 410);
    if (t.status === "posted") return json({ message: "Already posted. Nothing was sent twice." });
    if (t.status === "skipped") return json({ message: "This one was skipped, so nothing was posted." });
    if (t.status !== "filmed") return json({ error: `This pick is ${t.status}, not ready to post.` }, 409);
    if (action === "skip") {
      await setStatus(env, id, "skipped");
      return json({ message: "Skipped. Nothing was posted." });
    }
    await postEverywhere(env, id);
    return json({ message: "Posted to X, Bluesky, Mastodon, LinkedIn and YouTube. TikTok is in your TikTok inbox to finish." });
  }
  if (!good) return page("Link not valid", "<h1>This link is not valid</h1><p>Use <a href=\"/admin/trends\">the trends page</a>.</p>");
  const verb = action === "post" ? "Posting to every channel" : "Skipping";
  return page(
    `${verb}: ${escHtml(t.short_title)}`,
    `<h1>${escHtml(t.short_title)}</h1>
<p id="state" class="note">${verb}...</p>
<video src="${escHtml(mediaUrl(t.media_path))}" controls playsinline preload="metadata" style="max-width:280px;border-radius:12px"></video>
<p>${escHtml(t.caption)}</p>
<noscript><form method="POST"><button class="btn btn-primary">${action === "post" ? "Post everywhere" : "Skip"}</button></form></noscript>
<p class="muted"><a href="/admin/trends">All trends</a></p>
<script src="/t/go.js"></script>`
  );
}

export async function postEverywhere(env, id) {
  if (!env.POSTIZ_API_KEY) throw new Error("POSTIZ_API_KEY is not set on the worker");
  const t = await env.kaviri_waitlist.prepare("select * from trends where id = ?").bind(id).first();
  if (!t || t.status !== "filmed") throw new Error("only a filmed trend can be posted");
  const content = `<p>${t.caption.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c])}</p>`;
  const body = {
    type: "now",
    shortLink: false,
    date: new Date().toISOString(),
    tags: [],
    posts: CHANNELS.map((c) => ({
      integration: { id: c.id },
      value: [
        { content, image: [{ id: t.media_id, path: t.media_path }] },
        { content: `<p>${COMMENT}</p>`, image: [] },
      ],
      settings: { __type: c.type, ...c.settings, ...(c.title ? { title: t.short_title.slice(0, 90) } : {}) },
    })),
  };
  const r = await fetch(`${postizApi(env)}/posts`, {
    method: "POST",
    headers: { Authorization: env.POSTIZ_API_KEY, "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const res = await r.text();
  if (!r.ok) throw new Error(`postiz posts: ${r.status} ${res.slice(0, 300)}`);
  await env.kaviri_waitlist.prepare("update trends set status = 'posted', posted_at = datetime('now') where id = ?").bind(id).run();
  // So the daily email can tell how trend videos do against the other kinds.
  try {
    const ids = (JSON.parse(res) || []).map((x) => x.postId).filter(Boolean);
    await tagPosts(env, ids, "trend", t.short_title);
  } catch (e) {
    console.error("trend: could not tag posts", String(e));
  }
  return res;
}

export async function setStatus(env, id, s) {
  await env.kaviri_waitlist.prepare("update trends set status = ? where id = ?").bind(s, id).run();
}

export async function recentTrends(env, n = 14) {
  await ensureTrendSchema(env);
  const { results } = await env.kaviri_waitlist.prepare("select * from trends order by id desc limit ?").bind(n).all();
  return results || [];
}
