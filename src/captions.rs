//! Captions from what the take says: cues timed to the words, an .srt and a .vtt beside the
//! MP4, and, under `--captions burn`, the cues drawn into the picture.
//!
//! Burned captions are drawn by the browser, like the device frames, rather than by an
//! ffmpeg text filter: the filters that draw text need ffmpeg built with libass or
//! freetype, which a static build may lack, and a page lays type out far better than a
//! filter does. Each cue becomes one transparent PNG the size of the video, overlaid for the
//! seconds it is on screen.

use crate::tts::Word;

/// How the captions look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// White on a dark translucent pill, the way kaviri's own chrome looks. The default.
    Pill,
    /// Large heavy white type with a dark edge, the social-video look.
    Bold,
    /// Plain white type with a soft shadow, for a take that wants to stay out of the way.
    Plain,
}

impl Style {
    pub fn parse(s: &str) -> Result<Style, String> {
        match s {
            "pill" => Ok(Style::Pill),
            "bold" => Ok(Style::Bold),
            "plain" => Ok(Style::Plain),
            o => Err(format!(
                "--caption-style must be pill, bold or plain, got {o}"
            )),
        }
    }
}

/// What to do with captions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Sidecars when the take says something, nothing otherwise.
    Auto,
    Off,
    /// The .srt and .vtt only.
    File,
    /// The sidecars, and the cues drawn into the video.
    Burn,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s {
            "auto" => Ok(Mode::Auto),
            "off" => Ok(Mode::Off),
            "file" | "srt" => Ok(Mode::File),
            "burn" | "on" => Ok(Mode::Burn),
            o => Err(format!(
                "--captions must be auto, off, file or burn, got {o}"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Longest line, in characters, for a video of this shape. A vertical video is narrow and
/// its captions are read at arm's length, so its lines are short.
pub fn line_chars(w: u32, h: u32) -> usize {
    if h as f64 > w as f64 * 1.3 {
        22
    } else if w as f64 > h as f64 * 1.3 {
        42
    } else {
        30
    }
}

/// A cue never lingers longer than this after its last word, and is never shorter than
/// `MIN_CUE`, so a one-word cue can still be read.
const LINGER: f64 = 0.6;
const MIN_CUE: f64 = 0.7;

/// Cut a line's words into cues of at most two lines of `max` characters, breaking early at
/// the end of a sentence or after a comma when a cue is already half full.
pub fn cues(lines: &[(f64, Vec<Word>)], max: usize) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::new();
    for (at, words) in lines {
        let mut cur: Vec<&Word> = Vec::new();
        let flush = |cur: &mut Vec<&Word>, out: &mut Vec<Cue>| {
            if let (Some(a), Some(b)) = (cur.first(), cur.last()) {
                out.push(Cue {
                    start: at + a.start,
                    end: at + b.end,
                    text: wrap(
                        &cur.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
                        max,
                    ),
                });
            }
            cur.clear();
        };
        for w in words {
            let len: usize = cur.iter().map(|x| x.text.chars().count() + 1).sum();
            if !cur.is_empty() && len + w.text.chars().count() > 2 * max {
                flush(&mut cur, &mut out);
            }
            cur.push(w);
            let len = len + w.text.chars().count();
            let last = w.text.chars().last();
            if matches!(last, Some('.' | '!' | '?'))
                || (matches!(last, Some(',' | ';' | ':')) && len >= max)
            {
                flush(&mut cur, &mut out);
            }
        }
        flush(&mut cur, &mut out);
    }
    // Each cue stays up a little after its last word, but never into the next cue.
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|c| c.start).unwrap_or(f64::INFINITY);
        let c = &mut out[i];
        c.end = (c.end + LINGER).max(c.start + MIN_CUE).min(next);
        if c.end <= c.start {
            c.end = c.start + 0.05;
        }
    }
    out
}

/// Words into at most two lines of `max` characters, balanced so the second is not a stub.
pub fn wrap(words: &[&str], max: usize) -> String {
    let all = words.join(" ");
    if all.chars().count() <= max {
        return all;
    }
    // The break that leaves the two lines closest in length.
    let mut best = (usize::MAX, 1);
    for i in 1..words.len() {
        let a = words[..i].join(" ").chars().count();
        let b = words[i..].join(" ").chars().count();
        let d = a.abs_diff(b);
        if d < best.0 {
            best = (d, i);
        }
    }
    format!(
        "{}\n{}",
        words[..best.1].join(" "),
        words[best.1..].join(" ")
    )
}

fn stamp(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn srt(cues: &[Cue]) -> String {
    let mut s = String::new();
    for (i, c) in cues.iter().enumerate() {
        s.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            stamp(c.start, ','),
            stamp(c.end, ','),
            c.text
        ));
    }
    s
}

pub fn vtt(cues: &[Cue]) -> String {
    let mut s = String::from("WEBVTT\n\n");
    for c in cues {
        s.push_str(&format!(
            "{} --> {}\n{}\n\n",
            stamp(c.start, '.'),
            stamp(c.end, '.'),
            c.text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        ));
    }
    s
}

/// Where captions may sit, as (bottom, side) margins in pixels.
///
/// On a 9:16 video the bottom fifth belongs to the platform (the caption, the handle, the
/// sound) and the right edge to its buttons, so captions sit above the one and clear of the
/// other. TikTok, Reels and Shorts differ by a few percent; this clears all three.
pub fn safe_area(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w as f64, h as f64);
    if h > w * 1.3 {
        ((h * 0.24).round() as u32, (w * 0.14).round() as u32)
    } else if w > h * 1.3 {
        ((h * 0.07).round() as u32, (w * 0.08).round() as u32)
    } else {
        ((h * 0.10).round() as u32, (w * 0.08).round() as u32)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}

const FONT: &str = "-apple-system, BlinkMacSystemFont, 'SF Pro Display', 'Segoe UI Variable', \
                    'Segoe UI', Inter, Cantarell, Roboto, 'Noto Sans', 'Helvetica Neue', Arial, \
                    system-ui, sans-serif";

/// The page one cue is drawn from: transparent, the video's size, the text in its safe area.
pub fn html(cue: &Cue, style: Style, w: u32, h: u32) -> String {
    let (bottom, side) = safe_area(w, h);
    // Type scales with the short side, so a 1080x1920 and a 1920x1080 read the same.
    let short = w.min(h) as f64;
    let (size, box_css) = match style {
        Style::Pill => (
            short * 0.042,
            format!(
                "background:rgba(18,18,24,.78);color:#fff;font-weight:600;\
                 padding:{p1:.0}px {p2:.0}px;border-radius:{r:.0}px;\
                 box-shadow:0 {s:.0}px {b:.0}px rgba(0,0,0,.28);\
                 -webkit-backdrop-filter:blur(8px)",
                p1 = short * 0.014,
                p2 = short * 0.026,
                r = short * 0.022,
                s = short * 0.006,
                b = short * 0.024
            ),
        ),
        Style::Bold => (
            short * 0.06,
            format!(
                "color:#fff;font-weight:800;letter-spacing:-0.01em;\
                 -webkit-text-stroke:{k:.1}px rgba(0,0,0,.85);paint-order:stroke fill;\
                 text-shadow:0 {s:.0}px {b:.0}px rgba(0,0,0,.45)",
                k = short * 0.008,
                s = short * 0.004,
                b = short * 0.014
            ),
        ),
        Style::Plain => (
            short * 0.038,
            format!(
                "color:#fff;font-weight:500;text-shadow:0 1px {b:.0}px rgba(0,0,0,.85),\
                 0 0 {b2:.0}px rgba(0,0,0,.5)",
                b = short * 0.006,
                b2 = short * 0.02
            ),
        ),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;width:{w}px;height:{h}px;background:transparent;overflow:hidden}}\
         .z{{position:absolute;left:{side}px;right:{side}px;bottom:{bottom}px;\
            display:flex;justify-content:center}}\
         .c{{font-family:{FONT};font-size:{size:.1}px;line-height:1.28;text-align:center;\
            max-width:100%;{box_css}}}\
         </style></head><body><div class=\"z\"><div class=\"c\">{}</div></div></body></html>",
        esc(&cue.text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn cues_follow_the_word_timings_offset_by_where_the_line_was_said() {
        let line = vec![
            w("Open", 0.0, 0.3),
            w("the", 0.3, 0.4),
            w("dashboard.", 0.4, 1.0),
            w("Then", 1.3, 1.5),
            w("click", 1.5, 1.8),
            w("export.", 1.8, 2.3),
        ];
        let c = cues(&[(10.0, line)], 42);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text, "Open the dashboard.");
        assert_eq!(c[0].start, 10.0);
        // Lingers, but not into the next cue.
        assert!((c[0].end - 11.3).abs() < 1e-9);
        assert_eq!(c[1].start, 11.3);
        assert!((c[1].end - 12.9).abs() < 1e-9);
    }

    #[test]
    fn a_long_line_is_cut_into_cues_of_two_short_lines_on_a_vertical_video() {
        let text = "kaviri films your web app from a script and the camera zooms onto \
                    every click so nobody has to edit anything afterwards";
        let words = crate::tts::spread_words(text, 0.0, 8.0);
        let max = line_chars(1080, 1920);
        assert_eq!(max, 22);
        let c = cues(&[(0.0, words)], max);
        assert!(c.len() >= 3, "{c:?}");
        for cue in &c {
            assert!(cue.text.lines().count() <= 2, "{:?}", cue.text);
            for l in cue.text.lines() {
                assert!(l.chars().count() <= max + 12, "{l:?}");
            }
        }
        assert!(c.windows(2).all(|p| p[0].end <= p[1].start));
        assert_eq!(line_chars(1920, 1080), 42);
        assert_eq!(line_chars(1080, 1080), 30);
    }

    #[test]
    fn srt_and_vtt_are_written_the_way_players_read_them() {
        let c = vec![
            Cue {
                start: 0.5,
                end: 2.25,
                text: "Hello\nthere".into(),
            },
            Cue {
                start: 3723.004,
                end: 3724.0,
                text: "a < b & c".into(),
            },
        ];
        assert_eq!(
            srt(&c),
            "1\n00:00:00,500 --> 00:00:02,250\nHello\nthere\n\n\
             2\n01:02:03,004 --> 01:02:04,000\na < b & c\n\n"
        );
        let v = vtt(&c);
        assert!(v.starts_with("WEBVTT\n\n00:00:00.500 --> 00:00:02.250\nHello\nthere\n\n"));
        assert!(v.contains("a &lt; b &amp; c"));
    }

    #[test]
    fn wrapping_balances_two_lines() {
        assert_eq!(wrap(&["short"], 20), "short");
        assert_eq!(
            wrap(&["one", "two", "three", "four", "five", "six"], 14),
            "one two three\nfour five six"
        );
    }

    #[test]
    fn vertical_captions_sit_clear_of_the_platform_chrome() {
        let (bottom, side) = safe_area(1080, 1920);
        assert!(bottom >= 1920 / 5, "bottom {bottom}");
        assert!(side >= 1080 / 10, "side {side}");
        let (bottom, _) = safe_area(1920, 1080);
        assert!(bottom < 1080 / 10);
        let h = html(
            &Cue {
                start: 0.0,
                end: 1.0,
                text: "<b>hi</b>".into(),
            },
            Style::Pill,
            1080,
            1920,
        );
        assert!(h.contains("&lt;b&gt;hi"));
        assert!(h.contains(&format!(
            "bottom:{bottom}px",
            bottom = safe_area(1080, 1920).0
        )));
        assert!(Style::parse("neon").is_err());
        assert_eq!(Mode::parse("burn"), Ok(Mode::Burn));
        assert!(Mode::parse("yes").is_err());
    }
}
