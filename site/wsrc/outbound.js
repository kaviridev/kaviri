/*
 * Outbound: one-to-one emails to people who have not signed up, sent from a separate domain.
 *
 * This file is only the machinery. Who is written to and what each email says are seeded into
 * the D1 table `outbound` from the private billing repository, like the plan, so neither the
 * list nor the wording is in this public source. Every row is a finished email for one person.
 *
 * The rules this enforces, because a new sending domain lives or dies by them:
 *
 *   - Off unless OUTBOUND_ENABLED is "1". The kill switch is a variable, not a deploy.
 *   - A warm-up ramp from OUTBOUND_START: 5 a day in week one, 8 in week two, 12 in week
 *     three, then OUTBOUND_MAX (default 15). Weekdays only, in one hourly run (14:xx UTC,
 *     morning in the US, afternoon in the UK).
 *   - Never from kaviri.dev: OUTBOUND_FROM must be on a different domain, and the run refuses
 *     to send otherwise. Replies go to OUTBOUND_REPLY_TO.
 *   - Every email carries the one-click unsubscribe header pair and a plain link, and anyone
 *     who used it is never written to again, by any row.
 *   - Three failed sends in a day pause outbound until someone looks (the heartbeat says why).
 */

export const OUTBOUND_HOUR_UTC = 14;

export const SCHEMA = [
  `create table if not exists outbound (
     id         integer primary key autoincrement,
     email      text not null,
     company    text,
     subject    text not null,
     text_body  text not null,
     html_body  text,
     token      text not null unique,
     status     text not null default 'queued',  -- queued | sent | failed | suppressed | skipped
     queued_at  text not null default (datetime('now')),
     sent_at    text,
     error      text
   )`,
  `create unique index if not exists outbound_email on outbound(email)`,
  `create table if not exists outbound_suppress (
     email text primary key,
     at    text not null default (datetime('now')),
     why   text
   )`,
];

export async function ensureOutboundSchema(env) {
  for (const s of SCHEMA) await env.kaviri_waitlist.prepare(s).run();
}

/** Today's allowance under the warm-up ramp. */
export function dailyCap(env, now = new Date()) {
  const max = Number(env.OUTBOUND_MAX || 15);
  const start = env.OUTBOUND_START ? new Date(`${env.OUTBOUND_START}T00:00:00Z`) : null;
  if (!start || isNaN(start)) return 0;
  const day = Math.floor((now - start) / 86400000);
  if (day < 0) return 0;
  const ramp = day < 7 ? 5 : day < 14 ? 8 : day < 21 ? 12 : max;
  return Math.min(ramp, max);
}

function domainOf(addr) {
  const m = /@([^>\s]+)>?\s*$/.exec(addr || "");
  return m ? m[1].toLowerCase() : "";
}

async function send(env, row, unsubUrl) {
  const text = `${row.text_body}\n\n--\nNot interested? One click and you will not hear from me again: ${unsubUrl}\n`;
  const body = {
    from: env.OUTBOUND_FROM,
    to: [row.email],
    subject: row.subject,
    text,
    reply_to: env.OUTBOUND_REPLY_TO || "hello@kaviri.dev",
    headers: { "List-Unsubscribe": `<${unsubUrl}>`, "List-Unsubscribe-Post": "List-Unsubscribe=One-Click" },
  };
  if (row.html_body) {
    body.html = `${row.html_body}<p style="color:#888;font-size:12px">Not interested? <a href="${unsubUrl}">One click</a> and you will not hear from me again.</p>`;
  }
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 15000);
  try {
    const r = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { Authorization: `Bearer ${env.RESEND_TRYKAVIRI_KEY}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: ctl.signal,
    });
    if (!r.ok) return { ok: false, error: `${r.status} ${(await r.text()).slice(0, 200)}` };
    return { ok: true };
  } catch (e) {
    return { ok: false, error: String(e && e.message ? e.message : e) };
  } finally {
    clearTimeout(timer);
  }
}

/** The hourly run's outbound step. Returns what it did, for the heartbeat. */
export async function runOutbound(env, now = new Date()) {
  if (env.OUTBOUND_ENABLED !== "1") return { skipped: "disabled" };
  if (now.getUTCHours() !== OUTBOUND_HOUR_UTC) return { skipped: "not the sending hour" };
  const dow = now.getUTCDay();
  if (dow === 0 || dow === 6) return { skipped: "weekend" };
  if (!env.RESEND_TRYKAVIRI_KEY || !env.OUTBOUND_FROM) return { skipped: "not configured" };
  if (domainOf(env.OUTBOUND_FROM) === "kaviri.dev") return { skipped: "refusing to send outbound from kaviri.dev" };

  const db = env.kaviri_waitlist;
  const today = now.toISOString().slice(0, 10);
  const { n: failedToday } = await db
    .prepare(`select count(*) as n from outbound where status = 'failed' and substr(sent_at, 1, 10) = ?`)
    .bind(today)
    .first();
  if (failedToday >= 3) return { skipped: `paused: ${failedToday} failed sends today` };

  const { n: sentToday } = await db
    .prepare(`select count(*) as n from outbound where status = 'sent' and substr(sent_at, 1, 10) = ?`)
    .bind(today)
    .first();
  const room = dailyCap(env, now) - sentToday;
  if (room <= 0) return { sent: 0, sentToday, cap: dailyCap(env, now) };

  const { results } = await db
    .prepare(`select * from outbound where status = 'queued' order by id limit ?`)
    .bind(room)
    .all();
  let sent = 0, failed = 0, skipped = 0;
  for (const row of results) {
    const sup = await db.prepare(`select 1 from outbound_suppress where email = ?`).bind(row.email.toLowerCase()).first();
    if (sup) {
      await db.prepare(`update outbound set status = 'skipped', error = 'suppressed' where id = ?`).bind(row.id).run();
      skipped++;
      continue;
    }
    const r = await send(env, row, `https://kaviri.dev/u/${row.token}`);
    const at = new Date().toISOString();
    if (r.ok) {
      await db.prepare(`update outbound set status = 'sent', sent_at = ?, error = null where id = ?`).bind(at, row.id).run();
      sent++;
    } else {
      await db.prepare(`update outbound set status = 'failed', sent_at = ?, error = ? where id = ?`).bind(at, r.error, row.id).run();
      if (++failed >= 3) break;
    }
  }
  return { sent, failed, skipped, cap: dailyCap(env, now) };
}

/** /u/<token>: GET is the link in the email, POST is the mail client's one-click control. */
export async function outboundUnsubscribe(request, env, token) {
  const db = env.kaviri_waitlist;
  await ensureOutboundSchema(env);
  const row = token ? await db.prepare(`select email from outbound where token = ?`).bind(token).first() : null;
  if (row) {
    await db
      .prepare(`insert into outbound_suppress (email, why) values (?, 'unsubscribed') on conflict(email) do nothing`)
      .bind(row.email.toLowerCase())
      .run();
    await db.prepare(`update outbound set status = 'suppressed' where email = ? and status = 'queued'`).bind(row.email).run();
  }
  if (request.method === "POST") return new Response(null, { status: 204 });
  return new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Unsubscribed</title>` +
      `<body style="font:16px/1.5 system-ui;max-width:32em;margin:15vh auto;padding:0 16px">` +
      `<p>Done. You will not get another email from me.</p><p>Ishe, kaviri</p>`,
    { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" } }
  );
}
