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
