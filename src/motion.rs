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
/// A hard cut: one frame of overlap, so every join is the same filter.
const CUT_S: f64 = 1.0 / FPS as f64;
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
    /// Between two parts of the take: the take cuts away to the card at this second and
    /// picks up where it left off after it.
    Cut(f64),
}

/// One card, as the script asked for it. The page is built at render time, not when the op
/// runs, because a `frame` op after it can still change the video's size.
#[derive(Clone, Debug)]
pub struct Card {
    pub op: Value,
    pub place: Place,
    pub ms: u64,
    /// Joined to what is next to it by a dissolve rather than a hard cut.
    pub fade: bool,
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

/// A `title` op. `now` is the take's clock if a take is in flight when it runs.
pub fn title(op: &Value, now: Option<f64>) -> Result<Card, String> {
    let style = op["style"].as_str().unwrap_or("launch");
    if !TITLE_STYLES.iter().any(|(n, _)| *n == style) {
        return Err(format!(
            "unknown title style {style:?}; try {}",
            names(TITLE_STYLES)
        ));
    }
    if !has_content(op) && style != "end" {
        return Err("title needs text (or html, or file: your own animated page)".into());
    }
    let place = match (op["at"].as_str(), now) {
        (Some("start"), _) => Place::Start,
        (Some("end"), _) => Place::End,
        (None, None) => Place::Start,
        (None | Some("here"), Some(t)) => Place::Cut(t),
        (Some("here"), None) => {
            return Err(
                "at: \"here\" cuts the take away to the card, so it needs a take in \
                        flight; before start_recording a title is an intro"
                    .into(),
            )
        }
        (Some(other), _) => return Err(format!("at must be start, here or end, not {other:?}")),
    };
    // Intros and outros dissolve, a card in the middle of the take cuts: that is the rhythm
    // of a launch film. `transition` overrides either.
    let fade = match op["transition"].as_str() {
        Some("fade") => true,
        Some("cut") => false,
        None => !matches!(place, Place::Cut(_)) && style != "punch",
        Some(other) => return Err(format!("transition must be fade or cut, not {other:?}")),
    };
    let default_ms = match style {
        "punch" => 1100,
        "stat" => 2200,
        "end" => 3200,
        _ => TITLE_MS,
    };
    Ok(Card {
        op: op.clone(),
        place,
        ms: card_ms(op, default_ms)?,
        fade,
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
        fade: false,
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
    (
        "punch",
        "one huge word or line, in fast and cut out: the beat of a launch film",
    ),
    ("stat", "a big number that counts up, with a label under it"),
    (
        "end",
        "the kaviri end card: logo, wordmark and one line (text optional)",
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

/// Text for a card. A hyphen between two letters or digits becomes a non-breaking one, so a
/// headline never wraps inside a name ("GPT-" over "6.2").
fn esc(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let s: String = c
        .iter()
        .enumerate()
        .map(|(i, &ch)| {
            let joined = ch == '-'
                && i > 0
                && c[i - 1].is_alphanumeric()
                && c.get(i + 1).is_some_and(|n| n.is_alphanumeric());
            if joined {
                '\u{2011}'
            } else {
                ch
            }
        })
        .collect();
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
            "punch" => punch(op, w, h),
            "stat" => stat(op, w, h),
            "end" => end_card(op, w, h),
            _ => launch(op, w, h),
        },
    };
    let transparent = matches!(card.place, Place::Over(_));
    Ok(format!(
        "<!doctype html><html><head><meta charset=utf-8><style>{font}\
         :root{{--dur:{dur}ms}}\
         *{{box-sizing:border-box}}p{{text-wrap:balance}}\
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

/// The unit everything on a card is sized from. On landscape and square video it is the
/// short side. A portrait video is watched full screen on a phone, where a card sized from
/// its 1080px width reads small, so it gets a third more.
fn unit(w: u32, h: u32) -> f64 {
    if h as f64 > w as f64 * 1.2 {
        w as f64 * 1.35
    } else {
        w.min(h) as f64
    }
}

/// Type size for a headline: as big as the short side allows, smaller for longer lines.
fn headline_px(text: &str, w: u32, h: u32, max: f64) -> f64 {
    let short = unit(w, h);
    let n = text.chars().count() as f64;
    let portrait = h as f64 > w as f64 * 1.2;
    let fit = if portrait { 2.3 } else { 2.6 } * w.min(h).max(1) as f64 / n.max(6.0);
    // The longest word cannot wrap, so it has to fit across the card by itself: a bold
    // glyph is about 0.62 em wide, and the word gets 84% of the width.
    let longest = text
        .split_whitespace()
        .map(|t| t.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let word = w as f64 * 0.84 / (longest * 0.62);
    (short * max).min(fit).min(word).max(short * 0.045)
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
    let short = unit(w, h);
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
         .glow{{position:absolute;inset:0;background:radial-gradient(circle {rad:.0}px at 50% 54%,{accent} 0%,transparent 100%);\
         opacity:0;animation:glow var(--dur) ease-in-out forwards}}\
         @keyframes glow{{0%{{opacity:0;transform:scale(.85)}}30%{{opacity:{peak}}}100%{{opacity:{rest};transform:scale(1.12) translateY(-2%)}}}}\
         .wrap{{position:relative;text-align:center;padding:0 {pad:.0}px;animation:out .5s ease-in forwards;\
         animation-delay:calc(var(--dur) - 500ms)}}\
         @keyframes out{{to{{opacity:0;filter:blur(12px);transform:scale(.985)}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:600;letter-spacing:-.04em;line-height:1.04;text-wrap:balance}}</style>\
         <div class=glow></div><div class=wrap>{kicker}<h1>{words}</h1>{sub}</div>",
        rad = short * 0.55,
        peak = if light { ".28" } else { ".42" },
        rest = if light { ".16" } else { ".24" },
        pad = w as f64 * 0.08,
        kicker = kicker.unwrap_or_default(),
        words = words(&text, 250, 90, anim),
        sub = sub.unwrap_or_default(),
    )
}

fn bold(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
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
         h1{{margin:0;font-size:{size:.0}px;font-weight:800;letter-spacing:-.045em;line-height:.98;text-wrap:balance}}</style>\
         <div class=stripes></div><div class=wrap><h1>{words}</h1>{sub}</div>",
        s1 = short * 0.04,
        s2 = short * 0.08,
        pad = w as f64 * 0.07,
        words = words(&text, 200, 130, anim),
        sub = sub.unwrap_or_default(),
    )
}

fn terminal(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
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

fn punch(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
    let light = op["theme"].as_str() == Some("light");
    let (bg, fg) = if light {
        ("#ffffff", "#0b0b0c")
    } else {
        ("#000000", "#ffffff")
    };
    let text = text_of(op);
    let size = headline_px(&text, w, h, 0.2);
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<p style=\"margin:{:.0}px 0 0;font-size:{:.0}px;font-weight:500;opacity:0;\
             color:{};animation:kvIn .35s ease forwards;animation-delay:180ms\">{}</p>",
            short * 0.03,
            short * 0.038,
            if light {
                "rgba(11,11,12,.6)"
            } else {
                "rgba(255,255,255,.62)"
            },
            esc(s)
        )
    });
    format!(
        "<style>body{{background:{bg};color:{fg};font-family:{SANS};display:flex;align-items:center;\
         justify-content:center}}\
         .wrap{{text-align:center;padding:0 {pad:.0}px;animation:push var(--dur) linear forwards}}\
         @keyframes push{{from{{transform:scale(1)}}to{{transform:scale(1.06)}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:700;letter-spacing:-.05em;line-height:1;\
         text-wrap:balance;opacity:0;filter:blur({b:.0}px);transform:scale(1.22);\
         animation:kvIn .42s cubic-bezier(.16,1,.3,1) forwards}}</style>\
         <div class=wrap><h1>{text}</h1>{sub}</div>",
        pad = w as f64 * 0.07,
        b = short * 0.02,
        text = esc(&text).replace('\n', "<br>"),
        sub = sub.unwrap_or_default(),
    )
}

/// Splits "$8.40M" into ("$", 8.40, 2 decimals, "M"), so the number can count up.
fn number_parts(s: &str) -> Option<(String, f64, usize, String)> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let rest = &s[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(rest.len());
    let num = rest[..end].trim_end_matches('.');
    let value: f64 = num.replace(',', "").parse().ok()?;
    let decimals = num.split('.').nth(1).map(|d| d.len()).unwrap_or(0);
    Some((
        s[..start].to_string(),
        value,
        decimals,
        s[start + num.len()..].to_string(),
    ))
}

fn stat(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
    let accent = colour(op, "accent", BRAND_GREEN);
    let text = text_of(op);
    let size = headline_px(&text, w, h, 0.26);
    // The number counts up over the first 0.8 s, eased, through kvFrame, so it is seekable.
    let counter = match number_parts(&text) {
        Some((pre, v, d, post)) => format!(
            "<script>window.kvFrame=function(t){{var e=document.getElementById('n');\
             var p=Math.min(1,Math.max(0,(t-.15)/.8));p=1-Math.pow(1-p,3);\
             e.textContent={pre}+(p*{v}).toLocaleString('en-US',{{minimumFractionDigits:{d},maximumFractionDigits:{d}}})+{post};}};</script>",
            pre = serde_json::to_string(&pre).unwrap_or_default(),
            post = serde_json::to_string(&post).unwrap_or_default(),
        ),
        None => String::new(),
    };
    let kicker = op["kicker"].as_str().map(|k| {
        format!(
            "<div style=\"font-size:{:.0}px;font-weight:600;letter-spacing:.08em;text-transform:uppercase;\
             color:{accent};margin-bottom:{:.0}px;opacity:0;animation:kvIn .4s ease forwards\">{}</div>",
            short * 0.03,
            short * 0.02,
            esc(k)
        )
    });
    let sub = op["sub"].as_str().map(|s| {
        format!(
            "<p style=\"margin:{:.0}px auto 0;max-width:{:.0}px;font-size:{:.0}px;line-height:1.3;\
             color:rgba(255,255,255,.66);opacity:0;transform:translateY(.4em);\
             animation:kvIn .5s cubic-bezier(.2,.8,.2,1) forwards;animation-delay:500ms\">{}</p>",
            short * 0.03,
            w as f64 * 0.8,
            short * 0.04,
            esc(s)
        )
    });
    format!(
        "<style>body{{background:#000;color:#fff;font-family:{SANS};display:flex;align-items:center;\
         justify-content:center}}\
         .wrap{{text-align:center;animation:out .3s ease-in forwards;animation-delay:calc(var(--dur) - 300ms)}}\
         @keyframes out{{to{{opacity:0;transform:scale(.97)}}}}\
         .n{{font-size:{size:.0}px;font-weight:700;letter-spacing:-.05em;line-height:1;\
         font-variant-numeric:tabular-nums;opacity:0;transform:scale(1.12);\
         animation:kvIn .45s cubic-bezier(.16,1,.3,1) forwards}}\
         .bar{{height:{bh:.0}px;width:{bw:.0}px;margin:{bm:.0}px auto 0;background:{accent};border-radius:99px;\
         transform:scaleX(0);animation:kvIn .7s cubic-bezier(.2,.8,.2,1) forwards;animation-delay:250ms}}</style>\
         {counter}<div class=wrap>{kicker}<div class=n id=n>{text}</div><div class=bar></div>{sub}</div>",
        bh = short * 0.008,
        bw = short * 0.12,
        bm = short * 0.03,
        kicker = kicker.unwrap_or_default(),
        text = esc(&text),
        sub = sub.unwrap_or_default(),
    )
}

const LOGOMARK: &str = include_str!("../brand/logomark.svg");
const SPACE_GROTESK: &[u8] = include_bytes!("../brand/fonts/space-grotesk.woff2");

fn end_card(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
    let line = op["text"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Filmed by a script.");
    let sub = op["sub"].as_str().unwrap_or("kaviri.dev");
    let mark = LOGOMARK.replacen(
        "<svg",
        &format!(
            "<svg width=\"{:.0}\" height=\"{:.0}\"",
            short * 0.13,
            short * 0.13
        ),
        1,
    );
    format!(
        "<style>@font-face{{font-family:KvDisplay;src:url(data:font/woff2;base64,{sg}) format('woff2');font-weight:300 700}}\
         body{{background:#050607;color:#f2f4f5;font-family:{SANS};display:flex;align-items:center;justify-content:center}}\
         .glow{{position:absolute;inset:0;background:radial-gradient(circle {rad:.0}px at 50% 50%,#3ddc97 0%,transparent 100%);\
         opacity:0;animation:glow var(--dur) ease-out forwards}}\
         @keyframes glow{{0%{{opacity:0}}40%{{opacity:.22}}100%{{opacity:.14}}}}\
         .wrap{{position:relative;display:flex;flex-direction:column;align-items:center}}\
         .lock{{display:flex;align-items:center;gap:{gap:.0}px;opacity:0;filter:blur(16px);transform:scale(.94);\
         animation:kvIn .9s cubic-bezier(.16,1,.3,1) forwards;animation-delay:150ms}}\
         .wm{{font-family:KvDisplay,{SANS};font-weight:700;font-size:{wm:.0}px;letter-spacing:-.04em;line-height:1}}\
         .line{{margin-top:{lm:.0}px;font-size:{ls:.0}px;font-weight:500;letter-spacing:-.01em;opacity:0;\
         transform:translateY(.4em);animation:kvIn .7s cubic-bezier(.2,.8,.2,1) forwards;animation-delay:700ms}}\
         .sub{{margin-top:{sm:.0}px;font-size:{ss:.0}px;color:#3ddc97;font-weight:600;letter-spacing:.01em;opacity:0;\
         animation:kvIn .6s ease forwards;animation-delay:1000ms}}</style>\
         <div class=glow></div><div class=wrap><div class=lock>{mark}<span class=wm>kaviri</span></div>\
         <div class=line>{line}</div><div class=sub>{sub}</div></div>",
        sg = base64::engine::general_purpose::STANDARD.encode(SPACE_GROTESK),
        rad = short * 0.6,
        gap = short * 0.035,
        wm = short * 0.13,
        lm = short * 0.06,
        ls = short * 0.045,
        sm = short * 0.018,
        ss = short * 0.034,
        line = esc(line),
        sub = esc(sub),
    )
}

fn lower_third(op: &Value, w: u32, h: u32) -> String {
    let short = unit(w, h);
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
         .card{{margin-left:{gap:.0}px;padding:{py:.0}px {px:.0}px;background:rgba(24,24,28,.97);\
         box-shadow:inset 0 0 0 1px rgba(255,255,255,.14);border-radius:{r:.0}px;clip-path:inset(0 100% 0 0);animation:reveal .5s cubic-bezier(.2,.8,.2,1) forwards;\
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
    let short = unit(w, h);
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
    let short = unit(w, h);
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
        "<style>.s{{position:absolute;inset:0;background:rgba(0,0,0,.84);opacity:0;animation:kvIn .3s ease forwards}}\
         .w{{position:absolute;inset:0;display:flex;flex-direction:column;align-items:center;justify-content:center;\
         text-align:center;padding:0 {pad:.0}px;font-family:{SANS};color:#fff}}\
         .all{{position:absolute;inset:0;animation:out .4s ease-in forwards;animation-delay:calc(var(--dur) - 400ms)}}\
         @keyframes out{{to{{opacity:0}}}}\
         h1{{margin:0;font-size:{size:.0}px;font-weight:700;letter-spacing:-.035em;line-height:1.05;text-wrap:balance}}</style>\
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

/// The length ffmpeg reads off a file, which is what the joins have to agree with. The take's
/// length as kaviri counted it can run past the last frame actually written, and a card cut in
/// at that counted end then starts after the picture has run out, taking every card after it
/// (the end card included) off the end of the video.
fn media_secs(ffmpeg: &str, path: &str) -> Option<f64> {
    let o = Command::new(ffmpeg)
        .args(["-hide_banner", "-i", path])
        .output()
        .ok()?;
    let err = String::from_utf8_lossy(&o.stderr);
    let d = err.split("Duration: ").nth(1)?.split(',').next()?.trim();
    let mut secs = 0.0;
    for part in d.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    (secs > 0.0).then_some(secs)
}

fn has_audio(ffmpeg: &str, path: &str) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-i", path])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stderr).contains("Audio:"))
        .unwrap_or(false)
}

/// One piece of the finished video: a stretch of the take, or a card.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Take seconds `from..to`.
    Take(f64, f64),
    /// Card at this index in the card list.
    Card(usize),
}

/// Where each piece lands in the finished video.
#[derive(Debug, Clone)]
pub struct Timeline {
    /// (piece, start in the finished video, length, joined by a dissolve rather than a cut)
    pub pieces: Vec<(Piece, f64, f64, bool)>,
    pub total: f64,
}

impl Timeline {
    /// Where take second `t` lands in the finished video.
    pub fn map(&self, t: f64) -> f64 {
        for (p, start, _, _) in &self.pieces {
            if let Piece::Take(a, b) = p {
                if t >= *a && t <= *b {
                    return start + (t - a);
                }
            }
        }
        t
    }
}

/// Lay the take and the cards end to end. Intros, then the take cut at each mid-take card,
/// then outros. A join dissolves when the card on either side of it asks for a fade.
pub fn timeline(take: f64, cards: &[Card]) -> Timeline {
    let mut order: Vec<(Piece, f64, bool)> = Vec::new(); // piece, length, fades
    for (k, c) in cards.iter().enumerate() {
        if c.place == Place::Start {
            order.push((Piece::Card(k), c.secs(), c.fade));
        }
    }
    let mut cuts: Vec<(f64, usize)> = cards
        .iter()
        .enumerate()
        .filter_map(|(k, c)| match c.place {
            // A card cut in within half a second of the end goes at the end: the scrap of
            // take that would come back between it and the outro reads as a glitch.
            Place::Cut(t) => Some((
                if take - t < 0.5 {
                    take
                } else {
                    t.clamp(0.0, take)
                },
                k,
            )),
            _ => None,
        })
        .collect();
    cuts.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut from = 0.0;
    for (t, k) in &cuts {
        if *t - from > 0.05 {
            order.push((Piece::Take(from, *t), t - from, false));
        }
        order.push((Piece::Card(*k), cards[*k].secs(), cards[*k].fade));
        from = *t;
    }
    if take - from > 0.05 || order.iter().all(|(p, _, _)| matches!(p, Piece::Card(_))) {
        order.push((Piece::Take(from, take), (take - from).max(0.05), false));
    }
    for (k, c) in cards.iter().enumerate() {
        if c.place == Place::End {
            order.push((Piece::Card(k), c.secs(), c.fade));
        }
    }
    let mut pieces = Vec::new();
    let mut at = 0.0;
    for (i, (p, len, fades)) in order.iter().enumerate() {
        let dissolve = i > 0 && (*fades || order[i - 1].2);
        let overlap = if i == 0 {
            0.0
        } else if dissolve {
            FADE_S
        } else {
            CUT_S
        };
        let start = (at - overlap).max(0.0);
        pieces.push((p.clone(), start, *len, dissolve));
        at = start + len;
    }
    Timeline { pieces, total: at }
}

/// The finished video's length, given the take's and the cards'.
pub fn total_secs(take: f64, cards: &[Card]) -> f64 {
    timeline(take, cards).total
}

/// The filter graph that puts the cards on the take. Input 0 is the take; input k (from 1) is
/// card k-1's frames. Returns the graph, the label of the video it ends on, and the audio's.
pub fn filter_graph(take: f64, cards: &[Card], audio: bool) -> (String, String, Option<String>) {
    let mut g: Vec<String> = Vec::new();
    g.push(format!("[0:v]fps={FPS},format=yuv420p,setsar=1[m0]"));
    let mut main = "m0".to_string();
    // Callouts first, onto the take, so they are cut with it.
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
    let tl = timeline(take, cards);
    let n_take = tl
        .pieces
        .iter()
        .filter(|(p, ..)| matches!(p, Piece::Take(..)))
        .count();
    if n_take > 1 {
        let outs: String = (0..n_take).map(|j| format!("[tk{j}]")).collect();
        g.push(format!("[{main}]split={n_take}{outs}"));
    } else {
        g.push(format!("[{main}]null[tk0]"));
    }
    // Each piece as a stream of its own, all at the same rate and timebase so they can join.
    let mut labels: Vec<String> = Vec::new();
    let mut j = 0;
    for (n, (p, _, len, _)) in tl.pieces.iter().enumerate() {
        let label = format!("p{n}");
        match p {
            Piece::Take(a, b) => {
                g.push(format!(
                    "[tk{j}]trim=start={a:.3}:end={b:.3},setpts=PTS-STARTPTS,fps={FPS}[{label}]"
                ));
                j += 1;
            }
            Piece::Card(k) => {
                g.push(format!(
                    "[{}:v]fps={FPS},format=yuv420p,setsar=1,trim=duration={len:.3},setpts=PTS-STARTPTS,fps={FPS}[{label}]",
                    k + 1
                ));
            }
        }
        labels.push(label);
    }
    let mut acc = labels[0].clone();
    for (n, (_, start, _, dissolve)) in tl.pieces.iter().enumerate().skip(1) {
        let out = format!("x{n}");
        // A cut is a one-frame dissolve. concat would be the obvious filter, but it drops the
        // frame rate, and an xfade anywhere after it then refuses the stream.
        let d = if *dissolve { FADE_S } else { CUT_S };
        g.push(format!(
            "[{acc}][{}]xfade=transition=fade:duration={d:.4}:offset={start:.4}[{out}]",
            labels[n]
        ));
        acc = out;
    }
    let audio_out = if audio {
        // Each stretch of the take's sound moved to where its picture went, then mixed.
        let spans: Vec<(f64, f64, f64)> = tl
            .pieces
            .iter()
            .filter_map(|(p, start, ..)| match p {
                Piece::Take(a, b) => Some((*a, *b, *start)),
                _ => None,
            })
            .collect();
        let outs: String = (0..spans.len()).map(|j| format!("[ak{j}]")).collect();
        g.push(format!("[0:a]asplit={}{outs}", spans.len()));
        let mut mix = String::new();
        for (j, (a, b, start)) in spans.iter().enumerate() {
            let ms = (start * 1000.0).round() as u64;
            g.push(format!(
                "[ak{j}]atrim=start={a:.3}:end={b:.3},asetpts=PTS-STARTPTS,adelay=delays={ms}:all=1[am{j}]"
            ));
            mix.push_str(&format!("[am{j}]"));
        }
        g.push(format!(
            "{mix}amix=inputs={}:normalize=0:duration=longest,apad,atrim=0:{:.3}[a]",
            spans.len(),
            tl.total
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
    let take = match media_secs(&ffmpeg, out) {
        Some(real) if real < take => {
            if std::env::var_os("KAVIRI_DEBUG_MOTION").is_some() {
                eprintln!("kaviri: motion: take counted {take:.3}s, file is {real:.3}s");
            }
            real
        }
        _ => take,
    };
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
    if std::env::var_os("KAVIRI_DEBUG_MOTION").is_some() {
        eprintln!("kaviri: motion composite: {ffmpeg} {}", args.join(" "));
    }
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
        "take_starts_at": timeline(take, cards).map(0.0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str, at: &str) -> Card {
        title(&json!({"op": "title", "text": text, "at": at}), None).unwrap()
    }

    #[test]
    fn a_title_before_the_take_is_an_intro_and_during_it_needs_to_say_where() {
        assert_eq!(
            title(&json!({"text": "Hi"}), None).unwrap().place,
            Place::Start
        );
        assert_eq!(
            title(&json!({"text": "Hi"}), Some(1.0)).unwrap().place,
            Place::Cut(1.0)
        );
        assert!(title(&json!({"text": "Hi", "at": "here"}), None).is_err());
        assert_eq!(
            title(&json!({"text": "Hi", "at": "end"}), Some(1.0))
                .unwrap()
                .place,
            Place::End
        );
        assert!(title(&json!({"at": "end"}), None).is_err());
        assert!(title(&json!({"text": "Hi", "style": "sparkles"}), None).is_err());
        assert!(title(&json!({"text": "Hi", "ms": 50}), None).is_err());
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
            g.contains("xfade=transition=fade:duration=0.4000:offset=2.2000"),
            "{g}"
        );
        // intro 2.6 + take 10 - fade: the outro starts at 12.2 - 0.4.
        assert!(g.contains("offset=11.8000"), "{g}");
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
        assert_eq!(out, "p0");
        assert!(a.is_none());
    }

    #[test]
    fn script_text_cannot_escape_into_the_page() {
        let c = title(
            &json!({"text": "<script>x</script>", "accent": "red;}</style>"}),
            None,
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
            None,
        )
        .unwrap();
        let p = page(&c, 1080, 1920).unwrap();
        assert!(p.contains("--w:1080px") && p.contains("__kvSeek") && p.contains("hi"));
    }

    #[test]
    fn every_style_builds_at_every_shape() {
        for (w, h) in [(1920, 1080), (1080, 1920), (1080, 1080)] {
            for (s, _) in TITLE_STYLES {
                let c = title(&json!({"text": "Introducing the thing", "sub": "today", "kicker": "New", "style": s}), None).unwrap();
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

    #[test]
    fn a_card_mid_take_cuts_the_take_in_two_and_the_sound_follows_its_picture() {
        let cards = vec![
            title(&json!({"text": "Intro"}), None).unwrap(),
            title(&json!({"text": "Astra.", "style": "punch"}), Some(4.0)).unwrap(),
            title(&json!({"style": "end", "at": "end"}), Some(9.0)).unwrap(),
        ];
        let tl = timeline(10.0, &cards);
        let kinds: Vec<_> = tl.pieces.iter().map(|p| p.0.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                Piece::Card(0),
                Piece::Take(0.0, 4.0),
                Piece::Card(1),
                Piece::Take(4.0, 10.0),
                Piece::Card(2)
            ]
        );
        // intro 2.6 dissolves into the take at 2.2; the punch card cuts in hard at 6.2.
        assert!((tl.pieces[1].1 - 2.2).abs() < 1e-9);
        assert!((tl.pieces[2].1 - (6.2 - CUT_S)).abs() < 1e-9 && !tl.pieces[2].3);
        assert!((tl.map(5.0) - (7.3 - 2.0 * CUT_S + 1.0)).abs() < 1e-9);
        let (g, _, a) = filter_graph(10.0, &cards, true);
        assert!(g.contains("split=2[tk0][tk1]"), "{g}");
        assert!(g.contains("duration=0.0333"), "{g}");
        assert!(g.contains("adelay=delays=7233:all=1"), "{g}");
        assert!(a.is_some());
    }

    #[test]
    fn a_stat_counts_up_to_its_number_and_keeps_its_symbols() {
        assert_eq!(number_parts("$8"), Some(("$".into(), 8.0, 0, "".into())));
        assert_eq!(
            number_parts("1,318 Elo"),
            Some(("".into(), 1318.0, 0, " Elo".into()))
        );
        assert_eq!(
            number_parts("62.5%"),
            Some(("".into(), 62.5, 1, "%".into()))
        );
        assert_eq!(number_parts("none"), None);
        assert_eq!(esc("GPT-6.2 <b>"), "GPT\u{2011}6.2 &lt;b&gt;");
        assert_eq!(esc("a - b"), "a - b");
        let c = title(&json!({"text": "$8", "style": "stat"}), None).unwrap();
        assert!(page(&c, 1080, 1920).unwrap().contains("kvFrame"));
    }
}
