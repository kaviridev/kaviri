/*
 * The branded email shell.
 *
 * Every mail kaviri sends to a person goes through render(). One place decides what the
 * letterhead looks like, so changing it changes all of them, including the ones already
 * queued for a retry.
 *
 * Three rules this file follows that are not the rules the website follows:
 *
 * 1. RAW HEX IS CORRECT HERE. brand/tokens.css says hex outside that file is a bug, and for
 *    the site it is. Email clients do not support custom properties, Gmail strips <style>
 *    entirely, and Outlook resolves var() to nothing and paints black on black. The values
 *    below are copied from tokens.css by hand and the pairs are listed in TOKENS so a change
 *    there can be found here. There is no build step to do it for us.
 * 2. Layout is tables and inline styles. Not nostalgia: Gmail on Android still drops a flex
 *    container's children into a single column, and Outlook on Windows renders through Word.
 * 3. No remote images, no web fonts. An image-blocked email should look finished rather than
 *    broken, so the wordmark is text. It also keeps the mail out of the bucket where a
 *    tracking pixel would put it.
 *
 * Dark mode is offered through prefers-color-scheme in a <style> block. Apple Mail and iOS
 * honour it; Gmail ignores it and gets the light version, which is why the light version has
 * to be the one that is right.
 */

/** tokens.css name -> the literal this file uses. Keep them in step by hand. */
const TOKENS = {
  "--kv-ink": "#0f0f10",
  "--kv-n-50": "#f7f7f5",
  "--kv-n-100": "#ededea",
  "--kv-n-200": "#dfdfdb",
  "--kv-n-400": "#8a8a85",
  "--kv-n-500": "#5a5a57",
  "--kv-accent-600": "#0e7a4f",
  "--kv-accent-wash": "#e3f7ee",
  "--kv-white": "#ffffff",
};

const C = {
  ink: TOKENS["--kv-ink"],
  paper: TOKENS["--kv-white"],
  page: TOKENS["--kv-n-50"],
  rule: TOKENS["--kv-n-200"],
  wash: TOKENS["--kv-n-100"],
  muted: TOKENS["--kv-n-500"],
  faint: TOKENS["--kv-n-400"],
  accent: TOKENS["--kv-accent-600"],
  accentWash: TOKENS["--kv-accent-wash"],
};

const SANS = "Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";
const MONO = "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";

export function esc(s) {
  return String(s ?? "").replace(/[<>&"]/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;" })[c]);
}

/* ------------------------------------------------------------------ blocks */

/*
 * A body is an array of blocks rather than a string of HTML, so a step in the sequence
 * describes what it wants to say and never has to get a table nested correctly. Each block
 * knows how to render itself twice: once as HTML and once as the plain-text alternative,
 * which is what keeps the two versions from drifting apart.
 */

export const p = (text) => ({ kind: "p", text });
export const lead = (text) => ({ kind: "lead", text });
export const h = (text) => ({ kind: "h", text });
export const code = (text) => ({ kind: "code", text });
export const button = (label, href) => ({ kind: "button", label, href });
export const link = (label, href) => ({ kind: "link", label, href });
export const rule = () => ({ kind: "rule" });
export const note = (text) => ({ kind: "note", text });

/*
 * Report blocks, for the owner's own mail (the daily list count, the Monday plan, a signup).
 * Same shell as a subscriber mail so every kaviri email looks like one family.
 */
/** Big-number tiles in one row: [{ label, value, sub }]. Two to four read well on a phone. */
export const stats = (items) => ({ kind: "stats", items });
/** A label: value table: [[label, value], ...]. Values may use the inline markup. */
export const rows = (items) => ({ kind: "rows", items });
/** A bar from 0 to 1 with a caption under it. */
export const progress = (fraction, caption) => ({ kind: "progress", fraction, caption });
/** Unticked boxes, one per item. */
export const checklist = (items) => ({ kind: "checklist", items });
/** A coloured pill: tone is "good", "warn" or "bad". */
export const status = (text, tone = "good") => ({ kind: "status", text, tone });

/**
 * Inline markup, kept to the two things prose actually needs.
 *
 * `code` for a literal and *emphasis* for a word. Anything more and a step's copy becomes a
 * template language, which is the road to an email nobody can read in the source.
 */
function inline(text) {
  return esc(text)
    .replace(/`([^`]+)`/g, `<code style="font-family:${MONO};font-size:13px;background:${C.wash};padding:1px 4px;border-radius:3px;">$1</code>`)
    .replace(/\*([^*]+)\*/g, '<em style="font-style:normal;font-weight:600;">$1</em>')
    .replace(/\n/g, "<br>");
}

/** The same markers, removed rather than rendered. */
function plain(text) {
  return String(text).replace(/`([^`]+)`/g, "$1").replace(/\*([^*]+)\*/g, "$1");
}

function blockHtml(b) {
  const pStyle = `margin:0 0 16px;font-family:${SANS};font-size:15px;line-height:1.6;color:${C.ink};`;
  switch (b.kind) {
    case "lead":
      return `<p style="margin:0 0 20px;font-family:${SANS};font-size:17px;line-height:1.5;color:${C.ink};">${inline(b.text)}</p>`;
    case "p":
      return `<p style="${pStyle}">${inline(b.text)}</p>`;
    case "h":
      return `<h2 style="margin:28px 0 12px;font-family:${SANS};font-size:15px;font-weight:600;letter-spacing:0.09em;text-transform:uppercase;color:${C.muted};">${esc(b.text)}</h2>`;
    case "code":
      // white-space:pre survives Gmail; the horizontal scroll does not, so lines are kept
      // short in the copy rather than relying on the client to wrap them well.
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px;"><tr><td style="background:${C.page};border:1px solid ${C.rule};border-radius:6px;padding:14px 16px;font-family:${MONO};font-size:13px;line-height:1.6;color:${C.ink};white-space:pre;">${esc(b.text)}</td></tr></table>`;
    case "button":
      // A bordered table cell, not a styled <a>. Outlook ignores padding on an anchor and
      // the button collapses to the width of its text.
      return `<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:4px 0 20px;"><tr><td style="background:${C.accent};border-radius:3px;"><a href="${esc(b.href)}" style="display:inline-block;padding:11px 20px;font-family:${SANS};font-size:15px;font-weight:600;color:#ffffff;text-decoration:none;">${esc(b.label)}</a></td></tr></table>`;
    case "link":
      return `<p style="${pStyle}"><a href="${esc(b.href)}" style="color:${C.accent};text-decoration:underline;">${esc(b.label)}</a></p>`;
    case "rule":
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0"><tr><td style="border-top:1px solid ${C.rule};font-size:0;line-height:0;height:1px;">&nbsp;</td></tr></table><div style="height:24px;line-height:24px;">&nbsp;</div>`;
    case "note":
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px;"><tr><td style="background:${C.accentWash};border-left:3px solid ${C.accent};padding:12px 16px;font-family:${SANS};font-size:14px;line-height:1.55;color:${C.ink};">${inline(b.text)}</td></tr></table>`;
    case "stats": {
      const w = Math.floor(100 / b.items.length);
      const cells = b.items
        .map(
          (it, i) => `<td class="kv-tile" width="${w}%" valign="top" style="background:${C.page};border:1px solid ${C.rule};border-radius:8px;padding:14px 14px 12px;">
  <div style="font-family:${SANS};font-size:11px;font-weight:600;letter-spacing:0.08em;text-transform:uppercase;color:${C.muted};">${esc(it.label)}</div>
  <div style="margin-top:6px;font-family:${SANS};font-size:26px;line-height:1.1;font-weight:700;letter-spacing:-0.02em;color:${C.ink};">${esc(it.value)}</div>
  ${it.sub ? `<div style="margin-top:4px;font-family:${SANS};font-size:12px;line-height:1.4;color:${it.subTone === "good" ? C.accent : C.muted};">${esc(it.sub)}</div>` : ""}
</td>${i < b.items.length - 1 ? '<td width="8" style="font-size:0;">&nbsp;</td>' : ""}`
        )
        .join("");
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 20px;"><tr>${cells}</tr></table>`;
    }
    case "rows":
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 20px;border-collapse:collapse;">${b.items
        .map(
          ([k, v], i) => `<tr><td style="padding:9px 12px 9px 0;${i ? `border-top:1px solid ${C.rule};` : ""}font-family:${SANS};font-size:13px;color:${C.muted};white-space:nowrap;vertical-align:top;">${esc(k)}</td><td style="padding:9px 0;${i ? `border-top:1px solid ${C.rule};` : ""}font-family:${SANS};font-size:14px;line-height:1.5;color:${C.ink};text-align:right;word-break:break-word;">${inline(v)}</td></tr>`
        )
        .join("")}</table>`;
    case "progress": {
      const pct = Math.max(0, Math.min(100, Math.round(Number(b.fraction || 0) * 100)));
      // One row, two cells: the fill and the rest of the track. Rounded ends only where they are
      // the end of the bar, so a partly filled bar reads as one track rather than two pills.
      const cell = (w, bg, radius) =>
        `<td ${w ? `width="${w}%" ` : ""}class="${bg === C.wash ? "kv-track" : ""}" style="background:${bg};height:10px;font-size:0;line-height:0;border-radius:${radius};">&nbsp;</td>`;
      const shown = Math.max(pct, pct > 0 ? 2 : 0);
      const parts =
        shown === 0 ? cell(0, C.wash, "5px")
        : shown >= 100 ? cell(100, C.accent, "5px")
        : cell(shown, C.accent, "5px 0 0 5px") + cell(0, C.wash, "0 5px 5px 0");
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 6px;"><tr>${parts}</tr></table>
<p style="margin:0 0 20px;font-family:${SANS};font-size:12px;line-height:1.5;color:${C.muted};">${inline(b.caption || `${pct}%`)}</p>`;
    }
    case "checklist":
      return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 20px;">${b.items
        .map(
          (t) => `<tr><td width="26" valign="top" style="padding:7px 0;"><div style="width:14px;height:14px;border:2px solid ${C.accent};border-radius:4px;font-size:0;line-height:0;">&nbsp;</div></td><td style="padding:6px 0;font-family:${SANS};font-size:14px;line-height:1.5;color:${C.ink};">${inline(t)}</td></tr>`
        )
        .join("")}</table>`;
    case "status": {
      const tone = { good: [C.accentWash, C.accent], warn: ["#fdf3dc", "#8a5a00"], bad: ["#fde4e1", "#b42318"] }[b.tone] || [C.wash, C.muted];
      return `<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px;"><tr><td style="background:${tone[0]};border-radius:999px;padding:5px 12px;font-family:${SANS};font-size:12px;font-weight:700;letter-spacing:0.06em;text-transform:uppercase;color:${tone[1]};">${esc(b.text)}</td></tr></table>`;
    }
    default:
      return "";
  }
}

function blockText(b) {
  switch (b.kind) {
    case "lead":
    case "p":
    case "note":
      return plain(b.text);
    case "h":
      return String(b.text).toUpperCase();
    case "code":
      return String(b.text).split("\n").map((l) => `    ${l}`).join("\n");
    case "button":
    case "link":
      return `${b.label}: ${b.href}`;
    case "rule":
      return "-".repeat(56);
    case "stats":
      return b.items.map((it) => `${it.label}: ${it.value}${it.sub ? ` (${it.sub})` : ""}`).join("\n");
    case "rows":
      return b.items.map(([k, v]) => `${k}: ${plain(v)}`).join("\n");
    case "progress":
      return `[${"#".repeat(Math.round(Math.max(0, Math.min(1, b.fraction || 0)) * 20)).padEnd(20, ".")}] ${plain(b.caption || "")}`;
    case "checklist":
      return b.items.map((t) => `[ ] ${plain(t)}`).join("\n");
    case "status":
      return `[${String(b.text).toUpperCase()}]`;
    default:
      return "";
  }
}

/* ------------------------------------------------------------------- shell */

/**
 * The wordmark.
 *
 * Text, in the mono face, with the accent on `.dev`, which is the same shape the site's
 * header uses. It survives image blocking, it costs no request, and it is the one piece of
 * branding that appears before the reader has decided whether to keep reading.
 */
function letterhead() {
  return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
  <tr><td style="padding:0 0 22px;">
    <span style="font-family:${MONO};font-size:16px;font-weight:600;letter-spacing:-0.01em;color:${C.ink};">kaviri</span><span style="font-family:${MONO};font-size:16px;font-weight:600;color:${C.accent};">.dev</span>
  </td></tr>
</table>`;
}

function footer({ unsubscribeUrl, reason, signature = true }) {
  const small = `font-family:${SANS};font-size:12px;line-height:1.6;color:${C.faint};`;
  return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin-top:8px;">
  <tr><td style="border-top:1px solid ${C.rule};padding-top:18px;">
    <!-- Dogfooding: the animation is a kaviri take (examples/signature/), so the footer is a demo. -->
    ${signature ? `<a href="https://kaviri.dev/?utm_source=email&amp;utm_medium=footer" style="display:block;margin:0 0 16px;text-decoration:none;">
      <img src="https://kaviri.dev/email/signature.gif?v=3" width="360" alt="kaviri: your demo video is a build artifact. This animation was filmed by kaviri." style="display:block;width:360px;max-width:100%;height:auto;border:0;">
    </a>` : ""}
    <p style="margin:0 0 6px;${small}">${esc(reason)}</p>
    <p style="margin:0;${small}">
      <a href="https://kaviri.dev" style="color:${C.faint};text-decoration:underline;">kaviri.dev</a>
      &nbsp;&middot;&nbsp;
      <a href="https://github.com/kaviridev/kaviri" style="color:${C.faint};text-decoration:underline;">source</a>
      ${unsubscribeUrl ? `&nbsp;&middot;&nbsp;<a href="${esc(unsubscribeUrl)}" style="color:${C.faint};text-decoration:underline;">unsubscribe</a>` : ""}
    </p>
  </td></tr>
</table>`;
}

/**
 * Render one email.
 *
 * Returns `{ html, text }`. Both are always produced: a text/plain alternative is not a
 * courtesy, it is the difference between landing in the inbox and landing in spam, and it is
 * what a terminal mail client and a screen reader actually get.
 */
export function render({ preheader, blocks, unsubscribeUrl, reason, signature = true }) {
  const body = blocks.map(blockHtml).join("\n");

  /*
   * The preheader: the grey line a client shows after the subject. Without one it shows the
   * first words of the body, which here would be the wordmark. Hidden with the usual four
   * declarations, because no single one of them works everywhere, then padded with
   * zero-width joiners so the client does not spill the body text in after it.
   */
  const hidden =
    `<div style="display:none;max-height:0;overflow:hidden;mso-hide:all;font-size:1px;line-height:1px;color:${C.paper};opacity:0;">` +
    `${esc(preheader)}${"&#8204;&nbsp;".repeat(60)}</div>`;

  const html = `<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<style>
  /* Apple Mail and iOS honour this. Gmail drops the whole block, which is why the inline
     light styles above have to stand on their own. */
  @media (prefers-color-scheme: dark) {
    .kv-page { background: #0b0b0c !important; }
    .kv-card { background: #141416 !important; border-color: #26262a !important; }
    .kv-card p, .kv-card td, .kv-card span, .kv-card h2 { color: #f2f2ef !important; }
    .kv-card a { color: #3ddc97 !important; }
    .kv-card code { background: #1f1f22 !important; color: #f2f2ef !important; }
    .kv-tile, .kv-track { background: #1f1f22 !important; border-color: #2c2c30 !important; }
  }
  @media only screen and (max-width: 620px) {
    .kv-outer { padding: 16px 10px !important; }
    .kv-card { padding: 24px 18px !important; }
    .kv-tile { padding: 12px 10px 10px !important; }
  }
</style>
</head>
<body class="kv-page" style="margin:0;padding:0;background:${C.page};">
${hidden}
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" class="kv-page" style="background:${C.page};">
  <tr><td align="center" class="kv-outer" style="padding:32px 16px;">
    <!--[if mso]><table role="presentation" width="600" cellpadding="0" cellspacing="0" border="0"><tr><td><![endif]-->
    <!-- Fluid up to 600px. A table with width="600" ignores max-width and pushes a phone sideways. -->
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="width:100%;max-width:600px;">
      <tr><td class="kv-card" style="background:${C.paper};border:1px solid ${C.rule};border-radius:10px;padding:32px 36px;">
${letterhead()}
${body}
${footer({ unsubscribeUrl, reason, signature })}
      </td></tr>
    </table>
    <!--[if mso]></td></tr></table><![endif]-->
  </td></tr>
</table>
</body></html>`;

  const text = [
    "kaviri.dev",
    "",
    ...blocks.map(blockText).filter((s) => s !== ""),
    "",
    "-".repeat(56),
    reason,
    "https://kaviri.dev",
    unsubscribeUrl ? `Unsubscribe: ${unsubscribeUrl}` : null,
  ]
    .filter((l) => l !== null)
    .join("\n\n")
    .replace(/\n{3,}/g, "\n\n");

  return { html, text };
}
