/*
 * Motion graphics: title cards and callouts.
 *
 * A `title` is a full-screen card before or after the take (an intro, an outro). A `callout`
 * is drawn over the take from the moment the op runs: a lower third, a badge, or one big line
 * over a dimmed frame. Both are HTML and CSS animations, and both are rendered the same way:
 *
 *   1. kaviri builds the card's page (a built-in style, or the script's own HTML),
 *   2. opens it in a separate tab at the video's size, pauses every animation on it,
 *   3. seeks the animations to each frame's time and takes a screenshot.
 *
 * Seeking instead of filming is the point. A screencast of a CSS animation drops frames
 * whenever the machine is busy, which on a CI runner is always; a seek is exact, so a title
 * renders identically on a laptop and on a runner, frame for frame. It is also why custom
 * HTML works without any special API: anything driven by CSS animations or transitions, or by
 * a `window.kvFrame(t)` function, is seekable.
 *
 * The frames are then composited onto the rendered take by ffmpeg: callouts as overlays at
 * their times, titles joined on with a short dissolve, and the take's sound delayed by the
 * length of the intro so narration still lands on the action it was written for.
 */

use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Frames a second for the cards: the take's own rate, so the joins are seamless.
const FPS: u32 = crate::zoom::FPS;
/// How long the dissolve between a title and the take lasts.
const FADE_S: f64 = 0.4;
const TITLE_MS: u64 = 2600;
const CALLOUT_MS: u64 = 2400;
/// Upper bound on one card: past this a "title" is a video, and belongs in the take.
const MAX_MS: u64 = 15_000;

const INTER: &[u8] = include_bytes!("../brand/fonts/inter.woff2");
const BRAND_GREEN: &str = "#1fbf7a";

/// Where a card goes in the finished video.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// Before the take, in the order the ops ran.
    Start,
    /// After the take.
    End,
    /// Over the take, starting this many seconds in.
    Over(f64),
}

/// One card, as the script asked for it. The page is built at render time, not when the op
/// runs, because a `frame` op after it can still change the video's size.
#[derive(Clone, Debug)]
pub struct Card {
    pub op: Value,
    pub place: Place,
    pub ms: u64,
}

/// The seconds a card lasts.
impl Card {
    pub fn secs(&self) -> f64 {
        self.ms as f64 / 1000.0
    }
}

fn card_ms(op: &Value, default: u64) -> Result<u64, String> {
    let ms = match op.get("ms") {
        None | Some(Value::Null) => default,
        Some(v) => v
            .as_f64()
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| f.round() as u64)
            .ok_or("ms must be a positive number of milliseconds")?,
    };
    if !(600..=MAX_MS).contains(&ms) {
        return Err(format!(
            "ms must be between 600 and {MAX_MS}: shorter is a flash, longer belongs in the take"
        ));
    }
    Ok(ms)
}

fn has_content(op: &Value) -> bool {
    ["text", "html", "file"].iter().any(|k| {
        op[*k]
            .as_str()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    })
}

/// A `title` op. `recording` is whether a take is in flight when it runs.
pub fn title(op: &Value, recording: bool) -> Result<Card, String> {
    if !has_content(op) {
        return Err("title needs text (or html, or file: your own animated page)".into());
    }
    let place = match op["at"].as_str() {
        Some("start") => Place::Start,
        Some("end") => Place::End,
        None if !recording => Place::Start,
        None => {
            return Err(
                "a title sent during a take needs at: \"end\" (an outro). For a card over \
                        the take itself, use callout"
                    .into(),
            )
        }
        Some(other) => return Err(format!("at must be start or end, not {other:?}")),
    };
    if let Some(s) = op["style"].as_str() {
        if !TITLE_STYLES.iter().any(|(n, _)| *n == s) {
            return Err(format!(
                "unknown title style {s:?}; try {}",
                names(TITLE_STYLES)
            ));
        }
    }
    Ok(Card {
        op: op.clone(),
        place,
        ms: card_ms(op, TITLE_MS)?,
    })
}

/// A `callout` op, starting at `t` seconds into the take.
pub fn callout(op: &Value, t: f64) -> Result<Card, String> {
    if !has_content(op) {
        return Err("callout needs text (or html, or file)".into());
    }
    if let Some(s) = op["style"].as_str() {
        if !CALLOUT_STYLES.iter().any(|(n, _)| *n == s) {
            return Err(format!(
                "unknown callout style {s:?}; try {}",
                names(CALLOUT_STYLES)
            ));
        }
    }
    Ok(Card {
        op: op.clone(),
        place: Place::Over(t.max(0.0)),
        ms: card_ms(op, CALLOUT_MS)?,
    })
}

pub const TITLE_STYLES: &[(&str, &str)] = &[
    (
        "launch",
        "a product-launch card: words blur in one by one over a soft glow (default)",
    ),
    (
        "bold",
        "big heavy words that pop in on a solid accent colour",
    ),
    (
        "terminal",
        "the text is typed at a prompt, the subtitle printed under it",
    ),
];

pub const CALLOUT_STYLES: &[(&str, &str)] = &[
    (
        "lower-third",
        "a name-strap card in the bottom corner (default)",
    ),
    ("badge", "a pill that pops in at the top"),
    ("big", "one big line over a dimmed take"),
];

fn names(list: &[(&str, &str)]) -> String {
    list.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

pub fn styles_help() -> String {
    let mut s = String::from("title styles:\n");
    for (n, d) in TITLE_STYLES {
        s.push_str(&format!("  {n:<12} {d}\n"));
    }
    s.push_str("callout styles:\n");
    for (n, d) in CALLOUT_STYLES {
        s.push_str(&format!("  {n:<12} {d}\n"));
    }
    s
}

/* ------------------------------------------------------------------ pages */

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A colour from the script, or the default. Only plain CSS colour syntax gets through, so a
/// value cannot close the style attribute it is written into.
fn colour(op: &Value, key: &str, default: &str) -> String {
    match op[key].as_str() {
        Some(c)
            if !c.is_empty()
                && c.len() < 40
                && c.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "#(),.% ".contains(ch)) =>
        {
            c.to_string()
        }
        _ => default.to_string(),
    }
}

/// The script that makes a page seekable, injected into every card, built-in or not.
const SEEK_JS: &str = r#"<script>
window.__kvSeek = function (ms) {
  for (const a of document.getAnimations()) { a.pause(); a.currentTime = ms; }
  document.documentElement.style.setProperty('--t', String(ms / 1000));
  if (typeof window.kvFrame === 'function') window.kvFrame(ms / 1000);
};
</script>"#;

fn font_face() -> String {
    format!(
        "@font-face{{font-family:KvInter;src:url(data:font/woff2;base64,{}) format('woff2');font-weight:100 900}}",
        base64::engine::general_purpose::STANDARD.encode(INTER)
    )
}

const SANS: &str = "KvInter,-apple-system,'Segoe UI',Helvetica,Arial,sans-serif";
const MONO: &str = "'JetBrains Mono','DejaVu Sans Mono',ui-monospace,Menlo,Consolas,monospace";

/// The page for a card at a given size.
pub fn page(card: &Card, w: u32, h: u32) -> Result<String, String> {
    let op = &card.op;
    if let Some(html) = custom_html(op)? {
        return Ok(with_seek(&html, w, h, card.ms));
    }
    let body = match card.place {
        Place::Over(_) => match op["style"].as_str().unwrap_or("lower-third") {
            "badge" => badge(op, w, h),
            "big" => big(op, w, h),
            _ => lower_third(op, w, h),
        },
        _ => match op["style"].as_str().unwrap_or("launch") {
            "bold" => bold(op, w, h),
            "terminal" => terminal(op, w, h),
            _ => launch(op, w, h),
        },
    };
    let transparent = matches!(card.place, Place::Over(_));
    Ok(format!(
        "<!doctype html><html><head><meta charset=utf-8><style>{font}\
         :root{{--dur:{dur}ms}}\
         *{{box-sizing:border-box}}\
         html,body{{margin:0;width:{w}px;height:{h}px;overflow:hidden;background:{bg}}}\
         @keyframes kvIn{{to{{opacity:1;filter:blur(0);transform:none}}}}\
         </style>{seek}</head><body>{body}</body></html>",
        font = font_face(),
        dur = card.ms,
        bg = if transparent { "transparent" } else { "#000" },
        seek = SEEK_JS,
    ))
}

fn custom_html(op: &Value) -> Result<Option<String>, String> {
    if let Some(h) = op["html"].as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(Some(h.to_string()));
    }
    if let Some(f) = op["file"].as_str().filter(|s| !s.trim().is_empty()) {
        return std::fs::read_to_string(f)
            .map(Some)
            .map_err(|e| format!("card file {f}: {e}"));
    }
    Ok(None)
}

/// The script's own page, with the seek hook and the size and duration it can read.
fn with_seek(html: &str, w: u32, h: u32, ms: u64) -> String {
    let head = format!(
        "<style>:root{{--dur:{ms}ms;--w:{w}px;--h:{h}px}}{font}</style>{SEEK_JS}",
        font = font_face()
    );
    if let Some(i) = html.find("</head>") {
        format!("{}{}{}", &html[..i], head, &html[i..])
    } else {
        format!("<!doctype html><html><head><meta charset=utf-8>{head}</head><body>{html}</body></html>")
    }
}

/// Type size for a headline: as big as the short side allows, smaller for longer lines.
fn headline_px(text: &str, w: u32, h: u32, max: f64) -> f64 {
    let short = w.min(h) as f64;
    let n = text.chars().count() as f64;
    let portrait = h as f64 > w as f64 * 1.2;
    let fit = if portrait { 1.6 } else { 2.6 } * w.min(h).max(1) as f64 / n.max(6.0);
    (short * max).min(fit).max(short * 0.045)
}

/// Words that blur in one after another, the way launch cards set their headline.
fn words(text: &str, first_ms: u64, step_ms: u64, anim: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push_str("<br>");
        }
        for word in line.split_whitespace() {
            if i > 0 && !out.ends_with("<br>") {
                out.push(' ');
            }
            out.push_str(&format!(
                "<span style=\"display:inline-block;{anim};animation-delay:{}ms\">{}</span>",
                first_ms + i * step_ms,
                esc(word)
            ));
            i += 1;
        }
    }
    out
}

fn text_of(op: &Value) -> String {
    op["text"].as_str().unwrap_or("").trim().to_string()
}

fn launch(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let light = op["theme"].as_str() == Some("light");
    let (bg, fg, muted) = if light {
        ("#ffffff", "#0b0b0c", "rgba(11,11,12,.58)")
    } else {
        ("#000000", "#ffffff", "rgba(255,255,255,.62)")
    };
    let accent = colour(op, "accent", BRAND_GREEN);
    let text = text_of(op);
    let size = headline_px(&text, w, h, 0.105);
    let n_words = text.split_whitespace().count() as u64;
    let sub_delay = 250 + n_words * 90 + 200;
    let anim = "opacity:0;filter:blur(14px);transform:translateY(.2em);\
                animation:kvIn .9s cubic-bezier(.2,.7,.2,1) forwards";
    let kicker = op["kicker"].as_str().map(|k| {
        format!(
            "<div style=\"font-size:{:.0}px;font-weight:500;letter-spacing:.02em;color:{muted};\
             margin-bottom:{:.0}px;opacity:0;filter:blur(8px);animation:kvIn .8s ease forwards\">{}</div>",
            short * 0.03,
            short * 0.03,
            esc(k)
        )
    });
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<p style=\"margin:{:.0}px auto 0;max-width:{:.0}px;font-size:{:.0}px;line-height:1.35;\
             font-weight:400;letter-spacing:-.01em;color:{muted};opacity:0;filter:blur(8px);\
             transform:translateY(.3em);animation:kvIn .9s cubic-bezier(.2,.7,.2,1) forwards;\
             animation-delay:{sub_delay}ms\">{}</p>",
            short * 0.035,
            w as f64 * 0.8,
            short * 0.036,
            esc(s)
        )
    });
    format!(
        "<style>body{{background:{bg};color:{fg};font-family:{SANS};display:flex;align-items:center;\
         justify-content:center}}\
         .glow{{position:absolute;inset:-25%;background:radial-gradient(38% 32% at 50% 58%,{accent} 0%,transparent 72%);\
         opacity:0;filter:blur({blur:.0}px);animation:glow var(--dur) ease-in-out forwards}}\
         @keyframes glow{{0%{{opacity:0;transform:scale(.85)}}30%{{opacity:{peak}}}100%{{opacity:{rest};transform:scale(1.12) translateY(-2%)}}}}\
         .wrap{{position:relative;text-align:center;padding:0 {pad:.0}px;animation:out .5s ease-in forwards;\
         animation-delay:calc(var(--dur) - 500ms)}}\
         @keyframes out{{to{{opacity:0;filter:blur(12px);transform:scale(.985)}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:600;letter-spacing:-.04em;line-height:1.04}}</style>\
         <div class=glow></div><div class=wrap>{kicker}<h1>{words}</h1>{sub}</div>",
        blur = short * 0.06,
        peak = if light { ".35" } else { ".55" },
        rest = if light { ".22" } else { ".32" },
        pad = w as f64 * 0.08,
        kicker = kicker.unwrap_or_default(),
        words = words(&text, 250, 90, anim),
        sub = sub.unwrap_or_default(),
    )
}

fn bold(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let accent = colour(op, "accent", "#0e7a4f");
    let fg = colour(op, "color", "#ffffff");
    let text = text_of(op);
    let size = headline_px(&text, w, h, 0.13);
    let n_words = text.split_whitespace().count() as u64;
    let anim = "opacity:0;transform:scale(1.7) rotate(-4deg);\
                animation:kvIn .45s cubic-bezier(.3,1.5,.5,1) forwards";
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<p style=\"margin:{:.0}px 0 0;font-size:{:.0}px;font-weight:600;opacity:0;\
             transform:translateY(.6em);animation:kvIn .5s cubic-bezier(.2,.8,.2,1) forwards;\
             animation-delay:{}ms\">{}</p>",
            short * 0.03,
            short * 0.04,
            200 + n_words * 130 + 150,
            esc(s)
        )
    });
    format!(
        "<style>body{{background:{accent};color:{fg};font-family:{SANS};display:flex;align-items:center;\
         justify-content:center}}\
         .stripes{{position:absolute;inset:-50%;background:repeating-linear-gradient(-45deg,rgba(255,255,255,.06) 0 {s1:.0}px,transparent {s1:.0}px {s2:.0}px);\
         animation:slide var(--dur) linear forwards}}\
         @keyframes slide{{to{{transform:translate({s2:.0}px,{s2:.0}px)}}}}\
         .wrap{{position:relative;text-align:center;padding:0 {pad:.0}px;animation:out .4s cubic-bezier(.6,0,.8,.2) forwards;\
         animation-delay:calc(var(--dur) - 400ms)}}\
         @keyframes out{{to{{transform:translateY(-12%);opacity:0}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:800;letter-spacing:-.045em;line-height:.98}}</style>\
         <div class=stripes></div><div class=wrap><h1>{words}</h1>{sub}</div>",
        s1 = short * 0.04,
        s2 = short * 0.08,
        pad = w as f64 * 0.07,
        words = words(&text, 200, 130, anim),
        sub = sub.unwrap_or_default(),
    )
}

fn terminal(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let accent = colour(op, "accent", BRAND_GREEN);
    let text = text_of(op).replace('\n', " ");
    let n = text.chars().count().max(1);
    let size = (short * 0.06).min(w as f64 * 0.8 / (n as f64 + 2.0) / 0.6);
    let type_ms = (n as u64 * 45).clamp(400, 2200);
    let start = 300u64;
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<div style=\"margin-top:{:.0}px;color:rgba(255,255,255,.62);font-size:{:.0}px;opacity:0;\
             animation:kvIn .01s linear forwards;animation-delay:{}ms\">{}</div>",
            size * 0.6,
            size * 0.7,
            start + type_ms + 250,
            esc(s)
        )
    });
    format!(
        "<style>body{{background:#0b0d0c;color:#f2f2ef;font-family:{MONO};display:flex;align-items:center;\
         justify-content:center}}\
         .wrap{{font-size:{size:.1}px;animation:out .35s ease-in forwards;animation-delay:calc(var(--dur) - 350ms)}}\
         @keyframes out{{to{{opacity:0}}}}\
         .p{{color:{accent}}}\
         .t{{display:inline-block;overflow:hidden;white-space:pre;vertical-align:bottom;width:0;\
         animation:type {type_ms}ms steps({n}) forwards;animation-delay:{start}ms}}\
         @keyframes type{{to{{width:{n}ch}}}}\
         .c{{display:inline-block;width:.6ch;height:1.1em;vertical-align:-.15em;background:{accent};\
         animation:blink 1s steps(1) infinite}}\
         @keyframes blink{{50%{{opacity:0}}}}</style>\
         <div class=wrap><div><span class=p>$ </span><span class=t>{text}</span><span class=c></span></div>{sub}</div>",
        text = esc(&text),
        sub = sub.unwrap_or_default(),
    )
}

fn lower_third(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let (bottom, side) = crate::captions::safe_area(w, h);
    let accent = colour(op, "accent", BRAND_GREEN);
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<div style=\"margin-top:{:.0}px;font-size:{:.0}px;font-weight:400;color:rgba(255,255,255,.7);\
             opacity:0;transform:translateY(.4em);animation:kvIn .5s ease forwards;animation-delay:420ms\">{}</div>",
            short * 0.006,
            short * 0.027,
            esc(s)
        )
    });
    format!(
        "<style>.lt{{position:absolute;left:{side}px;bottom:{b:.0}px;display:flex;align-items:stretch;\
         font-family:{SANS};color:#fff;filter:drop-shadow(0 {s1:.0}px {s2:.0}px rgba(0,0,0,.35));\
         animation:out .4s ease-in forwards;animation-delay:calc(var(--dur) - 400ms)}}\
         @keyframes out{{to{{opacity:0;transform:translateX(-{s2:.0}px)}}}}\
         .bar{{width:{bar:.0}px;background:{accent};border-radius:{bar:.0}px;transform:scaleY(0);\
         animation:kvIn .3s cubic-bezier(.2,.8,.2,1) forwards}}\
         .card{{margin-left:{gap:.0}px;padding:{py:.0}px {px:.0}px;background:rgba(12,12,14,.84);\
         border-radius:{r:.0}px;clip-path:inset(0 100% 0 0);animation:reveal .5s cubic-bezier(.2,.8,.2,1) forwards;\
         animation-delay:150ms}}\
         @keyframes reveal{{to{{clip-path:inset(0 0 0 0)}}}}\
         .ti{{font-size:{ts:.0}px;font-weight:600;letter-spacing:-.015em;opacity:0;transform:translateY(.3em);\
         animation:kvIn .5s ease forwards;animation-delay:300ms}}</style>\
         <div class=lt><div class=bar></div><div class=card><div class=ti>{text}</div>{sub}</div></div>",
        b = bottom as f64 * 0.75 + short * 0.03,
        s1 = short * 0.008,
        s2 = short * 0.03,
        bar = short * 0.008,
        gap = short * 0.01,
        py = short * 0.018,
        px = short * 0.026,
        r = short * 0.012,
        ts = short * 0.04,
        text = esc(&text_of(op)),
        sub = sub.unwrap_or_default(),
    )
}

fn badge(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let accent = colour(op, "accent", "#0e7a4f");
    let top = if h as f64 > w as f64 * 1.3 {
        h as f64 * 0.12
    } else {
        h as f64 * 0.07
    };
    format!(
        "<style>.b{{position:absolute;left:0;right:0;top:{top:.0}px;display:flex;justify-content:center;\
         animation:out .35s ease-in forwards;animation-delay:calc(var(--dur) - 350ms)}}\
         @keyframes out{{to{{opacity:0;transform:translateY(-40%)}}}}\
         .pill{{font-family:{SANS};font-size:{fs:.0}px;font-weight:700;color:#fff;background:{accent};\
         padding:{py:.0}px {px:.0}px;border-radius:999px;box-shadow:0 {s1:.0}px {s2:.0}px rgba(0,0,0,.3);\
         opacity:0;transform:scale(.5);animation:kvIn .45s cubic-bezier(.3,1.6,.5,1) forwards}}</style>\
         <div class=b><div class=pill>{text}</div></div>",
        fs = short * 0.036,
        py = short * 0.014,
        px = short * 0.03,
        s1 = short * 0.008,
        s2 = short * 0.03,
        text = esc(&text_of(op)),
    )
}

fn big(op: &Value, w: u32, h: u32) -> String {
    let short = w.min(h) as f64;
    let text = text_of(op);
    let size = headline_px(&text, w, h, 0.1);
    let anim = "opacity:0;filter:blur(12px);transform:translateY(.2em);\
                animation:kvIn .7s cubic-bezier(.2,.7,.2,1) forwards";
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<p style=\"margin:{:.0}px 0 0;font-size:{:.0}px;color:rgba(255,255,255,.75);opacity:0;\
             animation:kvIn .6s ease forwards;animation-delay:{}ms\">{}</p>",
            short * 0.025,
            short * 0.034,
            200 + text.split_whitespace().count() as u64 * 80 + 150,
            esc(s)
        )
    });
    format!(
        "<style>.s{{position:absolute;inset:0;background:rgba(0,0,0,.6);opacity:0;animation:kvIn .3s ease forwards}}\
         .w{{position:absolute;inset:0;display:flex;flex-direction:column;align-items:center;justify-content:center;\
         text-align:center;padding:0 {pad:.0}px;font-family:{SANS};color:#fff}}\
         .all{{position:absolute;inset:0;animation:out .4s ease-in forwards;animation-delay:calc(var(--dur) - 400ms)}}\
         @keyframes out{{to{{opacity:0}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:700;letter-spacing:-.035em;line-height:1.05}}</style>\
         <div class=all><div class=s></div><div class=w><h1>{words}</h1>{sub}</div></div>",
        pad = w as f64 * 0.08,
        words = words(&text, 150, 80, anim),
        sub = sub.unwrap_or_default(),
    )
}

/* ------------------------------------------------------------- composite */

/// Renders a card's frames into a directory: `(page, w, h, frames, fps, dir, prefix, transparent)`.
pub type RenderFrames<'a> =
    dyn FnMut(&str, u32, u32, usize, u32, &Path, &str, bool) -> Result<(), String> + 'a;

fn has_audio(ffmpeg: &str, path: &str) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-i", path])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stderr).contains("Audio:"))
        .unwrap_or(false)
}

/// The finished video's length, given the take's and the cards'.
pub fn total_secs(take: f64, cards: &[Card]) -> f64 {
    let joined: Vec<f64> = cards
        .iter()
        .filter(|c| !matches!(c.place, Place::Over(_)))
        .map(Card::secs)
        .collect();
    take + joined.iter().sum::<f64>() - FADE_S * joined.len() as f64
}

/// The filter graph that puts the cards on the take. Input 0 is the take; input k (from 1) is
/// card k-1's frames. Returns the graph and the label of the video it ends on.
pub fn filter_graph(take: f64, cards: &[Card], audio: bool) -> (String, String, Option<String>) {
    let mut g: Vec<String> = Vec::new();
    g.push(format!("[0:v]fps={FPS},format=yuv420p,setsar=1[m0]"));
    let mut main = "m0".to_string();
    // Callouts first, onto the take, so the titles are joined to the finished take.
    for (k, c) in cards.iter().enumerate() {
        if let Place::Over(t) = c.place {
            let i = k + 1;
            let end = (t + c.secs()).min(take);
            g.push(format!(
                "[{i}:v]format=rgba,setpts=PTS-STARTPTS+{t:.3}/TB[o{i}]"
            ));
            g.push(format!(
                "[{main}][o{i}]overlay=0:0:eof_action=pass:enable='between(t,{t:.3},{end:.3})'[m{i}]"
            ));
            main = format!("m{i}");
        }
    }
    let seq_of = |p: &Place| -> Vec<(usize, &Card)> {
        cards
            .iter()
            .enumerate()
            .filter(|(_, c)| &c.place == p)
            .map(|(k, c)| (k + 1, c))
            .collect()
    };
    let intros = seq_of(&Place::Start);
    let outros = seq_of(&Place::End);
    // Every segment in order, with its label and length.
    let mut segs: Vec<(String, f64)> = Vec::new();
    for (i, c) in intros.iter().chain(outros.iter()) {
        g.push(format!(
            "[{i}:v]fps={FPS},format=yuv420p,setsar=1,settb=1/{FPS}[t{i}]"
        ));
        let _ = c;
    }
    g.push(format!("[{main}]settb=1/{FPS}[take]"));
    for (i, c) in &intros {
        segs.push((format!("t{i}"), c.secs()));
    }
    segs.push(("take".into(), take));
    for (i, c) in &outros {
        segs.push((format!("t{i}"), c.secs()));
    }
    let mut acc = segs[0].0.clone();
    let mut at = segs[0].1;
    for (n, (label, len)) in segs.iter().enumerate().skip(1) {
        let out = format!("x{n}");
        g.push(format!(
            "[{acc}][{label}]xfade=transition=fade:duration={FADE_S}:offset={:.3}[{out}]",
            (at - FADE_S).max(0.0)
        ));
        at += len - FADE_S;
        acc = out;
    }
    let total = at;
    let audio_out = if audio {
        let lead: f64 = intros.iter().map(|(_, c)| c.secs() - FADE_S).sum();
        let ms = (lead.max(0.0) * 1000.0).round() as u64;
        g.push(format!(
            "[0:a]adelay=delays={ms}:all=1,apad,atrim=0:{total:.3}[a]"
        ));
        Some("a".to_string())
    } else {
        None
    };
    (g.join(";"), acc, audio_out)
}

/// Put the cards on a rendered take, in place. Returns what was done, for the op result.
pub fn finish(
    out: &str,
    take: f64,
    cards: &[Card],
    size: (u32, u32),
    keep_temp: bool,
    render: &mut RenderFrames,
) -> Result<Value, String> {
    if cards.is_empty() {
        return Ok(Value::Null);
    }
    let ffmpeg = crate::zoom::find_ffmpeg()?;
    let (w, h) = (size.0 - size.0 % 2, size.1 - size.1 % 2);
    let dir = std::env::temp_dir().join(format!(
        "kaviri-motion-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let r = finish_in(out, take, cards, w, h, &dir, &ffmpeg, render);
    if keep_temp {
        eprintln!(
            "kaviri: kept the motion graphics frames in {}",
            dir.display()
        );
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    r
}

#[allow(clippy::too_many_arguments)]
fn finish_in(
    out: &str,
    take: f64,
    cards: &[Card],
    w: u32,
    h: u32,
    dir: &Path,
    ffmpeg: &str,
    render: &mut RenderFrames,
) -> Result<Value, String> {
    let mut inputs: Vec<String> = vec!["-i".into(), out.to_string()];
    for (k, c) in cards.iter().enumerate() {
        let frames = ((c.secs() * FPS as f64).round() as usize).max(1);
        let html = page(c, w, h)?;
        let transparent = matches!(c.place, Place::Over(_));
        let prefix = format!("c{k:03}_");
        eprintln!(
            "kaviri: rendering {} {k} ({frames} frames) ...",
            if transparent { "callout" } else { "title" }
        );
        render(&html, w, h, frames, FPS, dir, &prefix, transparent)?;
        inputs.extend([
            "-framerate".into(),
            FPS.to_string(),
            "-i".into(),
            dir.join(format!("{prefix}%05d.png")).display().to_string(),
        ]);
    }
    let audio = has_audio(ffmpeg, out);
    let (graph, vout, aout) = filter_graph(take, cards, audio);
    let tmp: PathBuf = {
        let p = Path::new(out);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        p.with_file_name(format!(".{name}.motion.mp4"))
    };
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    args.extend(inputs);
    args.extend([
        "-filter_complex".into(),
        graph,
        "-map".into(),
        format!("[{vout}]"),
    ]);
    if let Some(a) = &aout {
        args.extend([
            "-map".into(),
            format!("[{a}]"),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "160k".into(),
        ]);
    }
    args.extend(
        [
            "-c:v", "libx264", "-preset", "medium", "-crf", "19", "-pix_fmt", "yuv420p", "-r",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.push(FPS.to_string());
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        tmp.display().to_string(),
    ]);
    let o = Command::new(ffmpeg)
        .args(&args)
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "motion graphics composite failed: {}",
            String::from_utf8_lossy(&o.stderr)
                .trim()
                .chars()
                .take(600)
                .collect::<String>()
        ));
    }
    std::fs::rename(&tmp, out).map_err(|e| format!("{out}: {e}"))?;
    let titles = cards
        .iter()
        .filter(|c| !matches!(c.place, Place::Over(_)))
        .count();
    Ok(json!({
        "titles": titles,
        "callouts": cards.len() - titles,
        "duration": total_secs(take, cards),
        "take_starts_at": cards.iter().filter(|c| c.place == Place::Start).map(|c| c.secs() - FADE_S).sum::<f64>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str, at: &str) -> Card {
        title(&json!({"op": "title", "text": text, "at": at}), false).unwrap()
    }

    #[test]
    fn a_title_before_the_take_is_an_intro_and_during_it_needs_to_say_where() {
        assert_eq!(
            title(&json!({"text": "Hi"}), false).unwrap().place,
            Place::Start
        );
        assert!(title(&json!({"text": "Hi"}), true).is_err());
        assert_eq!(
            title(&json!({"text": "Hi", "at": "end"}), true)
                .unwrap()
                .place,
            Place::End
        );
        assert!(title(&json!({"at": "end"}), false).is_err());
        assert!(title(&json!({"text": "Hi", "style": "sparkles"}), false).is_err());
        assert!(title(&json!({"text": "Hi", "ms": 50}), false).is_err());
    }

    #[test]
    fn titles_are_joined_with_a_dissolve_and_the_sound_waits_for_the_intro() {
        let cards = vec![
            t("Intro", "start"),
            callout(&json!({"text": "Look"}), 2.0).unwrap(),
            t("Outro", "end"),
        ];
        let (g, out, a) = filter_graph(10.0, &cards, true);
        assert!(
            g.contains("overlay=0:0:eof_action=pass:enable='between(t,2.000,4.400)'"),
            "{g}"
        );
        assert!(
            g.contains("xfade=transition=fade:duration=0.4:offset=2.200"),
            "{g}"
        );
        // intro 2.6 + take 10 - fade: the outro starts at 12.2 - 0.4.
        assert!(g.contains("offset=11.800"), "{g}");
        assert!(g.contains("adelay=delays=2200:all=1"), "{g}");
        assert_eq!(out, "x2");
        assert_eq!(a.as_deref(), Some("a"));
        assert!((total_secs(10.0, &cards) - 14.4).abs() < 1e-9);
    }

    #[test]
    fn a_callout_never_outlasts_the_take() {
        let cards = vec![callout(&json!({"text": "Late"}), 9.0).unwrap()];
        let (g, out, a) = filter_graph(10.0, &cards, false);
        assert!(g.contains("between(t,9.000,10.000)"), "{g}");
        assert_eq!(out, "take");
        assert!(a.is_none());
    }

    #[test]
    fn script_text_cannot_escape_into_the_page() {
        let c = title(
            &json!({"text": "<script>x</script>", "accent": "red;}</style>"}),
            false,
        )
        .unwrap();
        let p = page(&c, 1920, 1080).unwrap();
        assert!(!p.contains("<script>x"));
        assert!(!p.contains("red;}"));
        assert!(p.contains("__kvSeek"));
    }

    #[test]
    fn custom_html_gets_the_seek_hook_and_its_size() {
        let c = title(
            &json!({"html": "<html><head></head><body>hi</body></html>"}),
            false,
        )
        .unwrap();
        let p = page(&c, 1080, 1920).unwrap();
        assert!(p.contains("--w:1080px") && p.contains("__kvSeek") && p.contains("hi"));
    }

    #[test]
    fn every_style_builds_at_every_shape() {
        for (w, h) in [(1920, 1080), (1080, 1920), (1080, 1080)] {
            for (s, _) in TITLE_STYLES {
                let c = title(&json!({"text": "Introducing the thing", "sub": "today", "kicker": "New", "style": s}), false).unwrap();
                assert!(page(&c, w, h).unwrap().len() > 1000);
            }
            for (s, _) in CALLOUT_STYLES {
                let c = callout(
                    &json!({"text": "A callout", "sub": "with a line", "style": s}),
                    1.0,
                )
                .unwrap();
                assert!(page(&c, w, h).unwrap().contains("transparent"));
            }
        }
    }
}
