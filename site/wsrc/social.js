/*
 * How the social accounts are doing, for the owner's daily email.
 *
 * X, YouTube and LinkedIn come from Postiz's analytics (it holds those platforms' tokens).
 * Bluesky and Mastodon come from their public APIs, which need no key; Postiz has no
 * analytics for them. TikTok is posted by hand for now, so it is not here.
 *
 * Each day's numbers are kept in D1, so the email can say what changed since yesterday rather
 * than only what the totals are. Any one source failing leaves a note in its row and the rest
 * of the email intact.
 */

const POSTIZ = "https://social.ishe.dev/api/public/v1";
const BLUESKY = "kaviri.dev";
const MASTODON = { host: "mastodon.social", id: "117614266944080663" };

const POSTIZ_CHANNELS = [
  { key: "x", label: "X", id: "cmwgcgd9l0001o97hyvjfvnw8",
    metrics: [["IMPRESSION", "impressions"], ["LIKE", "likes"], ["REPLY", "replies"], ["RETWEET", "reposts"]] },
  { key: "youtube", label: "YouTube", id: "cmwgchshx0005o97hq8n66kdh", lag: "about 3 days behind",
    metrics: [["Likes", "likes"], ["Estimated Minutes Watched", "minutes watched"], ["Subscribers Gained", "subscribers gained"], ["Subscribers Lost", "subscribers lost"]] },
  { key: "linkedin", label: "LinkedIn", id: "cmwgcgvb50003o97hmqv9gdl1",
    metrics: [["Page Views", "page views"], ["Clicks", "clicks"], ["Engagement", "engagement"], ["Organic Followers", "new followers"]] },
];

async function getJson(url, headers = {}) {
  const r = await fetch(url, { headers: { Accept: "application/json", ...headers } });
  if (!r.ok) throw new Error(`${r.status}`);
  return r.json();
}

async function fromPostiz(env, ch) {
  if (!env.POSTIZ_API_KEY) throw new Error("no POSTIZ_API_KEY");
  const list = await getJson(`${POSTIZ}/analytics/${ch.id}?date=7`, { Authorization: env.POSTIZ_API_KEY });
  const by = new Map((list || []).map((m) => [m.label, m]));
  const out = {};
  for (const [label, name] of ch.metrics) {
    const m = by.get(label);
    if (!m) continue;
    out[name] = Math.round((m.data || []).reduce((a, d) => a + Number(d.total || 0), 0));
  }
  return out;
}

async function fromBluesky() {
  const base = "https://public.api.bsky.app/xrpc";
  const p = await getJson(`${base}/app.bsky.actor.getProfile?actor=${BLUESKY}`);
  const f = await getJson(`${base}/app.bsky.feed.getAuthorFeed?actor=${BLUESKY}&limit=30&filter=posts_no_replies`);
  const since = Date.now() - 7 * 864e5;
  const week = (f.feed || []).map((x) => x.post).filter((x) => x && Date.parse(x.indexedAt) > since);
  const sum = (k) => week.reduce((a, x) => a + (x[k] || 0), 0);
  return {
    followers: p.followersCount || 0,
    "posts this week": week.length,
    likes: sum("likeCount"),
    reposts: sum("repostCount"),
    replies: sum("replyCount"),
  };
}

async function fromMastodon() {
  const base = `https://${MASTODON.host}/api/v1/accounts/${MASTODON.id}`;
  const a = await getJson(base);
  const st = await getJson(`${base}/statuses?limit=30&exclude_replies=true`);
  const since = Date.now() - 7 * 864e5;
  const week = (st || []).filter((x) => Date.parse(x.created_at) > since);
  const sum = (k) => week.reduce((n, x) => n + (x[k] || 0), 0);
  return {
    followers: a.followers_count || 0,
    "posts this week": week.length,
    likes: sum("favourites_count"),
    boosts: sum("reblogs_count"),
    replies: sum("replies_count"),
  };
}

async function ensureSchema(env) {
  await env.kaviri_waitlist
    .prepare(
      `create table if not exists social_daily (
         day text not null, channel text not null, metric text not null, value integer not null,
         primary key (day, channel, metric))`
    )
    .run();
}

/** Every channel's last-7-day numbers, and the change in each since yesterday's snapshot. */
export async function socialStats(env) {
  const jobs = [
    ...POSTIZ_CHANNELS.map((ch) => [ch.key, ch.label, ch.lag, () => fromPostiz(env, ch)]),
    ["bluesky", "Bluesky", null, fromBluesky],
    ["mastodon", "Mastodon", null, fromMastodon],
  ];
  const got = await Promise.allSettled(jobs.map(([, , , f]) => f()));
  await ensureSchema(env).catch(() => {});
  const day = new Date().toISOString().slice(0, 10);
  const prev = new Date(Date.now() - 864e5).toISOString().slice(0, 10);
  const { results: old } = await env.kaviri_waitlist
    .prepare("select channel, metric, value from social_daily where day = ?")
    .bind(prev)
    .all()
    .catch(() => ({ results: [] }));
  const before = new Map((old || []).map((r) => [`${r.channel}|${r.metric}`, r.value]));
  const rows = [];
  const writes = [];
  jobs.forEach(([key, label, lag], i) => {
    const g = got[i];
    if (g.status !== "fulfilled") {
      rows.push({ key, label, error: String(g.reason?.message || g.reason) });
      return;
    }
    const metrics = Object.entries(g.value).map(([metric, value]) => {
      const b = before.get(`${key}|${metric}`);
      writes.push(
        env.kaviri_waitlist
          .prepare("insert or replace into social_daily (day, channel, metric, value) values (?, ?, ?, ?)")
          .bind(day, key, metric, value)
      );
      return { metric, value, change: b == null ? null : value - b };
    });
    rows.push({ key, label, lag, metrics });
  });
  if (writes.length) await env.kaviri_waitlist.batch(writes).catch(() => {});
  return rows;
}

const ONE = { likes: "like", followers: "follower", replies: "reply", reposts: "repost", boosts: "boost",
  impressions: "impression", clicks: "click", "page views": "page view", "minutes watched": "minute watched",
  "new followers": "new follower", "subscribers gained": "subscriber gained", "subscribers lost": "subscriber lost",
  "posts this week": "post this week" };

/** One line per channel: "56 impressions (+12), 6 likes, 5 replies". */
export function socialLines(rows) {
  return rows.map((r) => {
    if (r.error) return [r.label, `not available today (${r.error})`];
    const parts = r.metrics
      .filter((m) => m.value || m.change)
      .map((m) => `${m.value.toLocaleString("en-US")} ${m.value === 1 && ONE[m.metric] ? ONE[m.metric] : m.metric}${m.change ? ` (${m.change > 0 ? "+" : ""}${m.change})` : ""}`);
    const text = parts.length ? parts.join(", ") : "nothing yet this week";
    return [r.label, r.lag ? `${text} · ${r.lag}` : text];
  });
}

// ---------------------------------------------------------------- what kind of post works

/*
 * Every post has a kind (launch, trend, explainer, text, ...), kept in social_kinds by Postiz
 * post id: the trend pipeline writes its own, the rest are written when posted. Once a day
 * (POSTS_HOUR_UTC, an hour before the email, because a free-plan run gets about 50 outside
 * requests) each recent post's numbers are fetched and kept in social_post_stats. The email
 * then ranks the kinds.
 *
 *   reach       X impressions and YouTube views, the two platforms that report them per post
 *   engagement  likes, replies, reposts and comments, everywhere that reports them
 */
export const POSTS_HOUR_UTC = 7;
const POST_FETCH_CAP = 30;

async function ensurePostSchema(env) {
  await env.kaviri_waitlist.batch([
    env.kaviri_waitlist.prepare(
      `create table if not exists social_kinds (post_id text primary key, kind text not null, title text)`
    ),
    env.kaviri_waitlist.prepare(
      `create table if not exists social_post_stats (
         post_id text primary key, platform text, kind text, title text, url text, posted_at text,
         reach integer, engagement integer, checked_at text)`
    ),
  ]);
}

/** Record what kind of post these Postiz post ids are. */
export async function tagPosts(env, ids, kind, title = null) {
  await ensurePostSchema(env);
  if (!ids.length) return;
  await env.kaviri_waitlist.batch(
    ids.map((id) =>
      env.kaviri_waitlist
        .prepare("insert or replace into social_kinds (post_id, kind, title) values (?, ?, ?)")
        .bind(id, kind, title)
    )
  );
}

const plain = (html) => String(html || "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
const sumOf = (list, labels) =>
  (list || []).filter((m) => labels.includes(m.label)).reduce((a, m) => a + (m.data || []).reduce((b, d) => b + Number(d.total || 0), 0), 0);

export async function collectPostStats(env) {
  if (!env.POSTIZ_API_KEY) return { skipped: "no POSTIZ_API_KEY" };
  await ensurePostSchema(env);
  const auth = { Authorization: env.POSTIZ_API_KEY };
  const end = new Date();
  const start = new Date(Date.now() - 30 * 864e5);
  const listing = await getJson(`${POSTIZ}/posts?startDate=${start.toISOString()}&endDate=${end.toISOString()}`, auth);
  const { results: kinds } = await env.kaviri_waitlist.prepare("select post_id, kind, title from social_kinds").all();
  const kindOf = new Map((kinds || []).map((k) => [k.post_id, k]));
  const posts = (listing.posts || [])
    .filter((p) => p.state === "PUBLISHED" && kindOf.has(p.id))
    .map((p) => ({ ...p, platform: p.integration?.providerIdentifier }))
    .filter((p) => ["x", "youtube", "linkedin-page", "bluesky", "mastodon"].includes(p.platform))
    .sort((a, b) => Date.parse(b.publishDate) - Date.parse(a.publishDate));

  const stats = new Map();
  // Bluesky, every post in one public call.
  const bsky = posts.filter((p) => p.platform === "bluesky" && String(p.releaseId).startsWith("at://")).slice(0, 25);
  if (bsky.length) {
    const q = bsky.map((p) => `uris=${encodeURIComponent(p.releaseId)}`).join("&");
    const got = await getJson(`https://public.api.bsky.app/xrpc/app.bsky.feed.getPosts?${q}`).catch(() => ({ posts: [] }));
    const byUri = new Map((got.posts || []).map((x) => [x.uri, x]));
    for (const p of bsky) {
      const x = byUri.get(p.releaseId);
      if (x) stats.set(p.id, { reach: null, engagement: (x.likeCount || 0) + (x.repostCount || 0) + (x.replyCount || 0) + (x.quoteCount || 0) });
    }
  }
  // The rest, one call a post, newest first, within the cap.
  let calls = 0;
  for (const p of posts) {
    if (p.platform === "bluesky" || calls >= POST_FETCH_CAP) continue;
    calls++;
    try {
      if (p.platform === "mastodon") {
        const s = await getJson(`https://${MASTODON.host}/api/v1/statuses/${encodeURIComponent(p.releaseId)}`);
        stats.set(p.id, { reach: null, engagement: (s.favourites_count || 0) + (s.reblogs_count || 0) + (s.replies_count || 0) });
      } else {
        const a = await getJson(`${POSTIZ}/analytics/post/${p.id}?date=30`, auth);
        const reach = p.platform === "x" ? sumOf(a, ["Impressions"]) : p.platform === "youtube" ? sumOf(a, ["Views"]) : null;
        const engagement = sumOf(a, ["Likes", "Retweets", "Replies", "Quotes", "Bookmarks", "Comments", "Favorites"]);
        stats.set(p.id, { reach, engagement });
      }
    } catch (e) {
      // One post's numbers missing today is not worth failing the rest for.
    }
  }
  const now = new Date().toISOString();
  const writes = posts
    .filter((p) => stats.has(p.id))
    .map((p) => {
      const k = kindOf.get(p.id);
      const s = stats.get(p.id);
      const title = k.title || p.settings?.title || plain(p.content).slice(0, 70);
      return env.kaviri_waitlist
        .prepare(
          `insert or replace into social_post_stats (post_id, platform, kind, title, url, posted_at, reach, engagement, checked_at)
           values (?, ?, ?, ?, ?, ?, ?, ?, ?)`
        )
        .bind(p.id, p.platform, k.kind, title, p.releaseURL || null, p.publishDate, s.reach, s.engagement, now);
    });
  if (writes.length) await env.kaviri_waitlist.batch(writes);
  return { posts: posts.length, measured: writes.length, calls };
}

const PLATFORM = { x: "X", youtube: "YouTube", "linkedin-page": "LinkedIn", bluesky: "Bluesky", mastodon: "Mastodon" };

/** The kinds ranked by reach per post, then engagement per post, and the best single post. */
export async function whatWorks(env) {
  await ensurePostSchema(env);
  const { results } = await env.kaviri_waitlist
    // Posts under 12 hours old are left out: their numbers are still climbing, and next to a
    // week-old post they would always look like the losing kind.
    .prepare(
      `select * from social_post_stats
       where posted_at >= datetime('now', '-30 days') and posted_at <= datetime('now', '-12 hours')`
    )
    .all();
  const list = results || [];
  if (!list.length) return null;
  const by = new Map();
  for (const r of list) {
    const k = by.get(r.kind) || { kind: r.kind, posts: new Set(), reach: 0, reachN: 0, eng: 0, engN: 0 };
    k.posts.add(r.title);
    if (r.reach != null) { k.reach += r.reach; k.reachN++; }
    if (r.engagement != null) { k.eng += r.engagement; k.engN++; }
    by.set(r.kind, k);
  }
  const kinds = [...by.values()]
    .map((k) => ({
      kind: k.kind,
      videos: k.posts.size,
      reachPer: k.reachN ? k.reach / k.reachN : 0,
      engPer: k.engN ? k.eng / k.engN : 0,
    }))
    .sort((a, b) => b.reachPer - a.reachPer || b.engPer - a.engPer);
  const best = list.filter((r) => r.reach != null).sort((a, b) => b.reach - a.reach)[0] || null;
  return { kinds, best, posts: list.length };
}

export function whatWorksLines(w) {
  const one = (n, word) => `${n % 1 ? n.toFixed(1) : n} ${word}`;
  return w.kinds.map((k) => [
    k.kind,
    `${one(k.videos, k.videos === 1 ? "post" : "posts")} · ${one(Math.round(k.reachPer * 10) / 10, "views or impressions")} per post · ${one(Math.round(k.engPer * 10) / 10, Math.round(k.engPer * 10) / 10 === 1 ? "engagement" : "engagements")} per post`,
  ]);
}

export function bestLine(w) {
  if (!w.best) return null;
  return `Best single post: ${w.best.title} on ${PLATFORM[w.best.platform] || w.best.platform}, ${w.best.reach.toLocaleString("en-US")} ${w.best.platform === "youtube" ? "views" : "impressions"}.`;
}
