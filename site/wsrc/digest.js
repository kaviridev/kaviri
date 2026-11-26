/*
 * The owner's daily count of the email list, sent by the hourly cron in its 08:xx UTC run.
 *
 * It goes through notifyOwner (Cloudflare's own sender, one verified address) rather than the
 * subscriber mail path, so it keeps arriving on a day the real sender is down, and it is sent
 * every day, including days with no change: a digest that only arrives when something happened
 * cannot be told apart from a cron that stopped running.
 */

import { notifyOwner } from "./mail.js";
import { render, h, p, stats, rows, note, button, status, link } from "./brand-email.js";
import { socialStats, socialLines, whatWorks, whatWorksLines, bestLine } from "./social.js";

export const DIGEST_HOUR_UTC = 8;

export async function listCounts(env) {
  const row = await env.kaviri_waitlist
    .prepare(
      `select
         count(*) as total,
         sum(case when unsubscribed_at is null then 1 else 0 end) as active,
         sum(case when unsubscribed_at is not null then 1 else 0 end) as unsubscribed,
         sum(case when created_at >= datetime('now', '-1 day') then 1 else 0 end) as new_day,
         sum(case when created_at >= datetime('now', '-7 days') then 1 else 0 end) as new_week,
         sum(case when unsubscribed_at >= datetime('now', '-7 days') then 1 else 0 end) as gone_week,
         sum(case when confirmed_at is null and unsubscribed_at is null then 1 else 0 end) as unsent
       from waitlist`
    )
    .first();
  const n = (k) => Number(row?.[k] || 0);
  return {
    total: n("total"), active: n("active"), unsubscribed: n("unsubscribed"),
    newDay: n("new_day"), newWeek: n("new_week"), goneWeek: n("gone_week"), unsent: n("unsent"),
  };
}

export async function recentSignups(env, days = 1, limit = 20) {
  const { results } = await env.kaviri_waitlist
    .prepare(
      `select email, country, created_at from waitlist
       where created_at >= datetime('now', ?) order by id desc limit ?`
    )
    .bind(`-${days} day`, limit)
    .all();
  return results || [];
}

export async function sendDigest(env) {
  const c = await listCounts(env);
  const recent = c.newDay ? await recentSignups(env, 1) : [];
  const plus = (x) => (x > 0 ? `+${x}` : "no change");
  const subject = `kaviri list: ${c.active} subscribed (${c.newDay ? `+${c.newDay} today` : "no change today"})`;
  const blocks = [
    status("Daily list count", c.newDay ? "good" : "neutral"),
    stats([
      { label: "Subscribed", value: String(c.active) },
      { label: "24 hours", value: plus(c.newDay), subTone: c.newDay ? "good" : null },
      { label: "7 days", value: plus(c.newWeek), sub: c.goneWeek ? `${c.goneWeek} unsubscribed` : null },
    ]),
    rows([
      ["All time", `${c.total} signed up`],
      ["Unsubscribed", String(c.unsubscribed)],
    ]),
  ];
  if (c.unsent) blocks.push(note(`*${c.unsent} welcome email(s) not sent yet.* The hourly cron retries them.`));
  if (recent.length) {
    blocks.push(h("New since yesterday"));
    blocks.push(rows(recent.map((r) => [`${r.created_at.slice(11, 16)} UTC`, `${r.email}${r.country ? ` (${r.country})` : ""}`])));
  }
  // The socials, last 7 days, with the change since yesterday's email in brackets.
  const social = await socialStats(env).catch((e) => [{ label: "Socials", error: String(e) }]);
  blocks.push(h("Socials, last 7 days"));
  blocks.push(rows(socialLines(social)));
  blocks.push(note("Changes in brackets are since yesterday's email. TikTok is posted by hand, so it is not counted here."));
  // Which kind of post is working: launches, trends, explainers, text. Collected an hour
  // earlier by the cron (social.collectPostStats).
  const works = await whatWorks(env).catch(() => null);
  if (works?.kinds?.length) {
    blocks.push(h("What's working, last 30 days"));
    blocks.push(rows(whatWorksLines(works)));
    const best = bestLine(works);
    if (best) blocks.push(note(best));
    if (works.kinds.reduce((a, k) => a + k.videos, 0) < 10) blocks.push(note("Fewer than ten posts so far, so read this as a hint, not a verdict."));
  }
  // The explainer job reports its last run here, so a stalled or failing run shows up, and
  // links each video it rendered in the last day so it can be watched before it posts.
  const auto = await env.kaviri_waitlist
    .prepare("select * from autopilot_status where name = 'explainers'")
    .first()
    .catch(() => null);
  if (auto) {
    blocks.push(h("Explainers"));
    if (auto.problems) blocks.push(note(`*Needs a look:* ${auto.problems}`));
    if (Date.now() - Date.parse(auto.updated_at.replace(" ", "T") + "Z") > 36 * 36e5)
      blocks.push(note("*It has not run for over a day.* The machine it runs on may be off or asleep; it catches up when it wakes."));
    blocks.push(p(auto.summary));
    blocks.push(rows([["Next to post", auto.next_up || "nothing queued"], ["Last run", `${auto.updated_at.slice(5, 16)} UTC`]]));
    let recent = [];
    try { recent = JSON.parse(auto.recent || "[]"); } catch {}
    if (recent.length) {
      blocks.push(h("New videos, last 24 hours"));
      for (const r of recent) {
        const day = new Date(`${r.date}T12:00:00Z`).toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short", timeZone: "UTC" });
        blocks.push(link(`${r.title}, posts ${day}`, r.watch));
      }
    }
  }
  blocks.push(button("Open the list", "https://kaviri.dev/admin"));
  const { html, text } = render({
    preheader: `${c.active} subscribed, ${plus(c.newDay)} in the last 24 hours.`,
    blocks,
    reason: "Sent every day at 08:00 UTC to the kaviri owner, including days with no change. It carries the list and the socials.",
    signature: false,
  });
  return { ...(await notifyOwner(env, { subject, text, html })), ...c };
}
