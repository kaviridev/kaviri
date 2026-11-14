/*
 * The Monday plan email: where the business is against its MRR target, and what this week is for.
 *
 * The plan itself (one row per week: start date, MRR target, focus, actions) lives in the D1
 * table plan_weeks and is seeded from the private billing repository, so strategy is never in
 * this public source. This file only measures and reports:
 *
 *   MRR        from the Stripe events the site already buffers (live mode only; test MRR is
 *              shown separately so a sandbox checkout never reads as revenue)
 *   visitors   unique visitors in the last 7 days, from the site's own cookieless counter
 *   list       active subscribers and the week's new signups
 *   stars      the kaviri repository's GitHub stars
 */

import { notifyOwner } from "./mail.js";
import { listCounts } from "./digest.js";

export const PLAN_CRON = "0 7 * * 1";

export const SCHEMA = `
create table if not exists plan_weeks (
  week        integer primary key,
  starts      text not null,
  target_mrr  integer not null,
  focus       text not null,
  actions     text not null
);
`;

const REPO = "kaviridev/kaviri";

/** Monthly recurring revenue in cents, from the latest event per subscription. */
export async function mrrFromEvents(env, live) {
  const { results } = await env.kaviri_waitlist
    .prepare(
      `select payload from stripe_events
       where type like 'customer.subscription.%' and livemode = ?
       order by created asc`
    )
    .bind(live ? 1 : 0)
    .all()
    .catch(() => ({ results: [] }));
  const latest = new Map();
  for (const r of results || []) {
    try {
      const sub = JSON.parse(r.payload)?.data?.object;
      if (sub?.id) latest.set(sub.id, sub);
    } catch (e) {
      /* a payload that is not JSON is not revenue */
    }
  }
  let cents = 0;
  let paying = 0;
  for (const sub of latest.values()) {
    if (sub.status !== "active" && sub.status !== "past_due") continue;
    let subCents = 0;
    for (const item of sub.items?.data || []) {
      const price = item.price || item.plan || {};
      const amount = Number(price.unit_amount ?? price.amount ?? 0) * Number(item.quantity || 1);
      const interval = price.recurring?.interval || price.interval || "month";
      const count = Number(price.recurring?.interval_count || price.interval_count || 1);
      const perMonth = interval === "year" ? amount / (12 * count) : interval === "week" ? (amount * 52) / (12 * count) : amount / count;
      subCents += perMonth;
    }
    // A subscription's discount, when there is one, comes off what it is worth.
    const pct = Number(sub.discount?.coupon?.percent_off || 0);
    subCents *= 1 - pct / 100;
    if (subCents > 0) paying += 1;
    cents += subCents;
  }
  return { cents: Math.round(cents), paying };
}

async function visitors7d(env) {
  const row = await env.kaviri_waitlist
    .prepare("select count(distinct vid) as n from visits where d >= date('now', '-7 days')")
    .first()
    .catch(() => null);
  return Number(row?.n || 0);
}

async function stars() {
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}`, {
      headers: { "User-Agent": "kaviri.dev-weekly-plan", Accept: "application/vnd.github+json" },
    });
    if (!r.ok) return null;
    return Number((await r.json()).stargazers_count);
  } catch (e) {
    return null;
  }
}

async function weeks(env) {
  await env.kaviri_waitlist.exec(SCHEMA.replace(/\s+/g, " ").trim()).catch(() => {});
  const { results } = await env.kaviri_waitlist
    .prepare("select * from plan_weeks order by week")
    .all()
    .catch(() => ({ results: [] }));
  return results || [];
}

const usd = (cents) => `$${(cents / 100).toLocaleString("en-US", { maximumFractionDigits: 0 })}`;

export async function sendPlan(env, now = new Date()) {
  const all = await weeks(env);
  if (!all.length) return { sent: false, error: "plan_weeks is empty; seed it from kaviri-billing" };
  const today = now.toISOString().slice(0, 10);
  const current = [...all].reverse().find((w) => w.starts <= today) || all[0];
  const next = all.find((w) => w.week === current.week + 1);
  const last = all[all.length - 1];

  const [live, test, list, vis, st] = await Promise.all([
    mrrFromEvents(env, true), mrrFromEvents(env, false), listCounts(env), visitors7d(env), stars(),
  ]);
  const target = current.target_mrr * 100;
  const gap = live.cents - target;
  const status = live.cents >= target ? "ON TRACK" : live.cents >= target * 0.6 ? "SLIGHTLY BEHIND" : "BEHIND";

  const bar = (cents, goal) => {
    const n = Math.max(0, Math.min(20, Math.round((cents / goal) * 20)));
    return "#".repeat(n) + ".".repeat(20 - n);
  };

  const lines = [
    `Week ${current.week} of ${last.week} (from ${current.starts}): ${current.focus}`,
    "",
    `MRR        ${usd(live.cents)} from ${live.paying} paying   target this week ${usd(target)}   ${status}${gap < 0 ? ` (${usd(-gap)} short)` : ""}`,
    `           [${bar(live.cents, last.target_mrr * 100)}] ${usd(live.cents)} of ${usd(last.target_mrr * 100)} by ${last.starts}`,
    test.cents ? `           (test mode: ${usd(test.cents)} from ${test.paying}; not counted)` : null,
    `List       ${list.active} subscribed, +${list.newWeek} this week`,
    `Visitors   ${vis} unique in the last 7 days`,
    st != null ? `Stars      ${st} on github.com/${REPO}` : null,
    "",
    "This week:",
    ...String(current.actions).split("\n").filter(Boolean).map((a) => `  [ ] ${a.replace(/^[-*]\s*/, "")}`),
  ].filter((l) => l !== null);
  if (next) lines.push("", `Next week (${next.starts}): ${next.focus}`);
  if (status === "BEHIND" && current.week >= 4) {
    lines.push("", "Behind by more than 40%: move this week's hours from content to outbound and design-partner calls.");
  }
  lines.push("", "Daily numbers arrive in the 08:00 list email. Plan source: kaviri-billing/docs/PLAN-1K-MRR.md");

  const subject = `kaviri week ${current.week}/${last.week}: ${usd(live.cents)} MRR, ${status.toLowerCase()}`;
  return { ...(await notifyOwner(env, { subject, text: lines.join("\n") })), week: current.week, mrr: live.cents };
}
