/*
 * The owner's daily count of the email list, sent by the hourly cron in its 08:xx UTC run.
 *
 * It goes through notifyOwner (Cloudflare's own sender, one verified address) rather than the
 * subscriber mail path, so it keeps arriving on a day the real sender is down, and it is sent
 * every day, including days with no change: a digest that only arrives when something happened
 * cannot be told apart from a cron that stopped running.
 */

import { notifyOwner } from "./mail.js";

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
  const lines = [
    `kaviri email list: ${c.active} subscribed`,
    "",
    `Last 24 hours: ${plus(c.newDay)}`,
    `Last 7 days:   ${plus(c.newWeek)}${c.goneWeek ? `, ${c.goneWeek} unsubscribed` : ""}`,
    `All time:      ${c.total} signed up, ${c.unsubscribed} unsubscribed`,
  ];
  if (c.unsent) lines.push("", `${c.unsent} welcome email(s) not sent yet. The hourly cron retries them.`);
  if (recent.length) {
    lines.push("", "New since yesterday:");
    for (const r of recent) lines.push(`  ${r.email}${r.country ? ` (${r.country})` : ""}  ${r.created_at} UTC`);
  }
  lines.push("", "Full list and CSV: https://kaviri.dev/admin");
  const subject = `kaviri list: ${c.active} subscribed (${c.newDay ? `+${c.newDay} today` : "no change today"})`;
  return { ...(await notifyOwner(env, { subject, text: lines.join("\n") })), ...c };
}
