//! The take's soundtrack: narration placed on the recording clock, a voiceover and a music
//! bed laid under it, the page's own sound, all mixed to one AAC track at a set loudness,
//! with captions written beside the MP4 or drawn into it.
//!
//! Narration is placed here, in Rust, sample by sample: each `say` and `voice` op is a clip
//! dropped at the moment the op ran, so it lands exactly where the script put it. Everything
//! after that is one ffmpeg filter graph, built by `audio_graph`, which is a pure function
//! and is tested as a string:
//!
//! ```text
//! page ─────────── volume ───────────────────────────────┐
//! narration ─┐                                           ├─ amix ─ loudnorm ─ AAC
//! voiceover ─┴─ volume ─ amix ─ asplit ──────────────────┤
//!                                  └─ key ─┐             │
//! music ─ loop ─ trim ─ fades ─ volume ─ sidechaincompress
//! ```
//!
//! Loudness is two passes of EBU R128 `loudnorm`: the first measures the mix, the second
//! applies one linear gain to reach the target, so the dynamics the duck made survive. One
//! pass would normalise dynamically and pump the music back up between sentences.

use crate::captions::{self, Cue, Mode, Style};
use crate::tts::{Speech, RATE};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the flags asked of the soundtrack.
#[derive(Clone, Debug)]
pub struct Cfg {
    /// The speech engine for `say`: piper, none, openai, elevenlabs or cloudflare.
    pub tts: String,
    /// The voice a `say` without its own uses.
    pub voice: Option<String>,
    pub voiceover: Option<PathBuf>,
    pub voiceover_gain: f64,
    pub music: Option<PathBuf>,
    /// The bed's level in dB, before ducking.
    pub music_volume: f64,
    /// Seconds of fade at each end of the bed.
    pub music_fade: f64,
    pub music_loop: bool,
    /// Pull the bed down under narration and voiceover.
    pub duck: bool,
    pub page_gain: f64,
    pub narration_gain: f64,
    /// Integrated loudness target in LUFS, or None to leave the levels as mixed.
    pub loudness: Option<f64>,
    pub captions: Mode,
    pub caption_style: Style,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg {
            tts: crate::env::var("TTS").unwrap_or_else(|| "piper".into()),
            voice: None,
            voiceover: None,
            voiceover_gain: 0.0,
            music: None,
            music_volume: -18.0,
            music_fade: 1.5,
            music_loop: true,
            duck: true,
            page_gain: 0.0,
            narration_gain: 0.0,
            loudness: Some(-16.0),
            captions: Mode::Auto,
            caption_style: Style::Pill,
        }
    }
}

/// True peak ceiling and loudness range for `loudnorm`. -1.5 dBTP leaves room for the AAC
/// encoder's overshoot, which is what the platforms ask for.
const TRUE_PEAK: f64 = -1.5;
const LRA: f64 = 11.0;

/// One piece of narration on the take's clock.
#[derive(Clone, Debug)]
pub struct Clip {
    /// Seconds into the take.
    pub at: f64,
    pub speech: Speech,
    pub gain_db: f64,
    /// What the clip says, for captions. A `voice` clip has none unless the op gave it.
    pub text: Option<String>,
}

/// How long a `wait` of `ms` lasts when speech is in flight: until the speech ends, if that
/// is later. This is what lines the script's actions up with what is being said.
pub fn stretch_ms(ms: u64, now: f64, speech_until: f64) -> u64 {
    let rest = ((speech_until - now) * 1000.0).ceil();
    if rest.is_finite() && rest > ms as f64 {
        rest as u64
    } else {
        ms
    }
}

fn db(g: f64) -> f32 {
    10f64.powf(g / 20.0) as f32
}

/// Add `pcm` into `buf` starting `at` seconds in, both at `RATE`, growing `buf` as needed.
pub fn place_pcm(buf: &mut Vec<f32>, pcm: &[f32], at: f64, gain: f32) {
    let start = (at.max(0.0) * RATE as f64).round() as usize;
    if buf.len() < start + pcm.len() {
        buf.resize(start + pcm.len(), 0.0);
    }
    for (i, s) in pcm.iter().enumerate() {
        buf[start + i] += s * gain;
    }
}

/// Every clip on one mono track `secs` long.
pub fn narration(clips: &[Clip], secs: f64) -> Result<Vec<f32>, String> {
    let mut buf = Vec::new();
    for c in clips {
        let (pcm, rate) = crate::tts::read_wav(&c.speech.wav)?;
        if rate != RATE {
            return Err(format!(
                "{} is {rate} Hz, not {RATE}",
                c.speech.wav.display()
            ));
        }
        place_pcm(&mut buf, &pcm, c.at, db(c.gain_db));
    }
    buf.resize((secs.max(0.0) * RATE as f64).round() as usize, 0.0);
    Ok(buf)
}

/// The captions a take's clips make.
pub fn take_cues(clips: &[Clip], w: u32, h: u32) -> Vec<Cue> {
    let lines: Vec<(f64, Vec<crate::tts::Word>)> = clips
        .iter()
        .filter_map(|c| {
            let text = c.text.as_deref()?;
            let words = if c.speech.words.is_empty() {
                crate::tts::spread_words(text, 0.0, c.speech.secs)
            } else {
                c.speech.words.clone()
            };
            Some((c.at, words))
        })
        .collect();
    captions::cues(&lines, captions::line_chars(w, h))
}

/// Where each source is among ffmpeg's inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub page: Option<usize>,
    pub narration: Option<usize>,
    pub voiceover: Option<usize>,
    pub music: Option<usize>,
}

/// What `loudnorm` does at the end of the graph.
#[derive(Clone, Debug, PartialEq)]
pub enum Loud {
    Off,
    /// The first pass: measure, and print what it measured.
    Measure(f64),
    /// The second: one linear gain to the target, from the first pass's numbers.
    Apply(f64, Measured),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub i: String,
    pub tp: String,
    pub lra: String,
    pub thresh: String,
    pub offset: String,
}

/// Read `loudnorm`'s JSON report off the end of ffmpeg's stderr.
pub fn parse_measured(stderr: &str) -> Option<Measured> {
    let start = stderr.rfind('{')?;
    let end = stderr[start..].find('}')? + start + 1;
    let v: Value = serde_json::from_str(&stderr[start..end]).ok()?;
    let f = |k: &str| -> Option<String> {
        let s = v[k].as_str()?.trim().to_string();
        // Silence measures as -inf, which loudnorm will not take back.
        s.parse::<f64>().ok().filter(|x| x.is_finite())?;
        Some(s)
    };
    Some(Measured {
        i: f("input_i")?,
        tp: f("input_tp")?,
        lra: f("input_lra")?,
        thresh: f("input_thresh")?,
        offset: f("target_offset")?,
    })
}

/// The `loudnorm` filter for each pass.
pub fn loudnorm(l: &Loud) -> Option<String> {
    match l {
        Loud::Off => None,
        Loud::Measure(t) => Some(format!(
            "loudnorm=I={t}:TP={TRUE_PEAK}:LRA={LRA}:print_format=json"
        )),
        Loud::Apply(t, m) => Some(format!(
            "loudnorm=I={t}:TP={TRUE_PEAK}:LRA={LRA}:measured_I={}:measured_TP={}:\
             measured_LRA={}:measured_thresh={}:offset={}:linear=true",
            m.i, m.tp, m.lra, m.thresh, m.offset
        )),
    }
}

const FMT: &str = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";

/// The audio half of the filter graph, ending in `[aout]`, exactly `dur` seconds long.
/// None when there is nothing to mix.
pub fn audio_graph(inp: &Inputs, cfg: &Cfg, dur: f64, loud: &Loud) -> Option<String> {
    let mut g: Vec<String> = Vec::new();
    let mut bus: Vec<String> = Vec::new();
    if let Some(i) = inp.page {
        g.push(format!("[{i}:a]{FMT},volume={}dB[page]", cfg.page_gain));
        bus.push("[page]".into());
    }
    let mut speech: Vec<String> = Vec::new();
    if let Some(i) = inp.narration {
        g.push(format!(
            "[{i}:a]{FMT},volume={}dB[narr]",
            cfg.narration_gain
        ));
        speech.push("[narr]".into());
    }
    if let Some(i) = inp.voiceover {
        g.push(format!("[{i}:a]{FMT},volume={}dB[vo]", cfg.voiceover_gain));
        speech.push("[vo]".into());
    }
    let duck = cfg.duck && inp.music.is_some() && !speech.is_empty();
    if !speech.is_empty() {
        let joined = if speech.len() == 1 {
            speech[0].clone()
        } else {
            g.push(format!(
                "{}amix=inputs={}:duration=longest:normalize=0[speech]",
                speech.join(""),
                speech.len()
            ));
            "[speech]".into()
        };
        if duck {
            // The key is padded with silence to the take's length: sidechaincompress stops
            // at the end of either input, so a voiceover shorter than the take would
            // otherwise cut the bed off where the voiceover ends. Every pad in the graph is
            // bounded; two unbounded ones spin ffmpeg forever after the last sample.
            g.push(format!(
                "{joined}asplit=2[said][k];[k]apad=whole_dur={dur:.3}[key]"
            ));
            bus.push("[said]".into());
        } else {
            bus.push(joined);
        }
    }
    if let Some(i) = inp.music {
        let fade = cfg.music_fade.min(dur / 3.0).max(0.0);
        let mut chain = format!(
            "[{i}:a]{FMT},atrim=0:{dur:.3},asetpts=PTS-STARTPTS,volume={}dB",
            cfg.music_volume
        );
        if fade > 0.0 {
            chain.push_str(&format!(
                ",afade=t=in:st=0:d={fade:.3},afade=t=out:st={:.3}:d={fade:.3}",
                (dur - fade).max(0.0)
            ));
        }
        if duck {
            // A hard knee at a low threshold: any speech at all pulls the bed down by
            // most of 20 dB, and it comes back up over half a second when the line ends.
            g.push(format!("{chain}[bed]"));
            g.push(
                "[bed][key]sidechaincompress=threshold=0.015:ratio=12:attack=25:release=450:\
                 makeup=1[music]"
                    .into(),
            );
        } else {
            g.push(format!("{chain}[music]"));
        }
        bus.push("[music]".into());
    }
    if bus.is_empty() {
        return None;
    }
    let mut tail = if bus.len() == 1 {
        format!("{}anull", bus[0])
    } else {
        format!(
            "{}amix=inputs={}:duration=longest:normalize=0",
            bus.join(""),
            bus.len()
        )
    };
    if let Some(l) = loudnorm(loud) {
        tail.push(',');
        tail.push_str(&l);
    }
    // loudnorm works at 192 kHz; back to the take's rate, then exactly the video's length.
    tail.push_str(&format!(
        ",aresample=48000,apad=whole_dur={dur:.3},atrim=0:{dur:.3},asetpts=PTS-STARTPTS[aout]"
    ));
    g.push(tail);
    Some(g.join(";"))
}

/// The picture half: each caption PNG overlaid for its seconds. Inputs from `first`.
pub fn caption_graph(cues: &[Cue], first: usize) -> Option<String> {
    if cues.is_empty() {
        return None;
    }
    let mut g = Vec::new();
    let mut prev = "[0:v]".to_string();
    for (k, c) in cues.iter().enumerate() {
        let next = if k + 1 == cues.len() {
            "[vout]".to_string()
        } else {
            format!("[v{k}]")
        };
        g.push(format!(
            "{prev}[{}:v]overlay=0:0:enable='between(t,{:.3},{:.3})'{next}",
            first + k,
            c.start,
            c.end
        ));
        prev = next;
    }
    Some(g.join(";"))
}

/// Draws a page to a PNG of the given size: the browser, for burned captions.
pub type RenderPng<'a> = dyn FnMut(&str, u32, u32) -> Result<Vec<u8>, String> + 'a;

/// What there is to finish a take with.
pub struct Parts<'a> {
    pub out: &'a str,
    pub dur: f64,
    /// The page's sound, already written as a WAV.
    pub page: Option<PathBuf>,
    pub clips: &'a [Clip],
    pub cfg: &'a Cfg,
    /// The video's size.
    pub size: (u32, u32),
    pub keep_temp: bool,
}

/// Lay the soundtrack under the rendered video and write the captions. Returns what was
/// done, for the `recording_rendered` result, or Null when there was nothing to do.
pub fn finish(p: &Parts, render_png: &mut RenderPng) -> Result<Value, String> {
    let (w, h) = (p.size.0 - p.size.0 % 2, p.size.1 - p.size.1 % 2);
    let dir = crate::cdp::session_dir()?.join(format!("soundtrack-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let r = finish_in(p, &dir, w, h, render_png);
    if p.keep_temp {
        eprintln!("kaviri: kept the soundtrack's parts in {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    r
}

fn finish_in(
    p: &Parts,
    dir: &Path,
    w: u32,
    h: u32,
    render_png: &mut RenderPng,
) -> Result<Value, String> {
    let cfg = p.cfg;
    let mut report = serde_json::Map::new();

    // Captions first: they need nothing but the clips.
    let cues = take_cues(p.clips, w, h);
    let want_captions = match cfg.captions {
        Mode::Off => false,
        Mode::Auto => !cues.is_empty(),
        Mode::File | Mode::Burn => true,
    };
    let mut burn: Vec<PathBuf> = Vec::new();
    if want_captions {
        if cues.is_empty() {
            eprintln!("kaviri: --captions: the take says nothing, so there are no captions");
        } else {
            let stem = Path::new(p.out).with_extension("");
            let srt = PathBuf::from(format!("{}.srt", stem.display()));
            let vtt = PathBuf::from(format!("{}.vtt", stem.display()));
            std::fs::write(&srt, captions::srt(&cues))
                .map_err(|e| format!("{}: {e}", srt.display()))?;
            std::fs::write(&vtt, captions::vtt(&cues))
                .map_err(|e| format!("{}: {e}", vtt.display()))?;
            report.insert(
                "captions".into(),
                json!({"cues": cues.len(), "srt": srt.display().to_string(),
                       "vtt": vtt.display().to_string(), "burned": cfg.captions == Mode::Burn}),
            );
            if cfg.captions == Mode::Burn {
                for (k, c) in cues.iter().enumerate() {
                    let png = render_png(&captions::html(c, cfg.caption_style, w, h), w, h)?;
                    let f = dir.join(format!("cue{k:04}.png"));
                    std::fs::write(&f, png).map_err(|e| format!("{}: {e}", f.display()))?;
                    burn.push(f);
                }
            }
        }
    }

    // The audio inputs, in the order ffmpeg will see them after the video.
    let mut files: Vec<(PathBuf, bool)> = Vec::new(); // (path, loop it)
    let mut inp = Inputs::default();
    let add = |f: PathBuf, looped: bool, files: &mut Vec<(PathBuf, bool)>| -> usize {
        files.push((f, looped));
        files.len()
    };
    if let Some(page) = &p.page {
        inp.page = Some(add(page.clone(), false, &mut files));
    }
    if !p.clips.is_empty() {
        let wav = dir.join("narration.wav");
        std::fs::write(
            &wav,
            crate::tts::wav_bytes(&narration(p.clips, p.dur)?, RATE),
        )
        .map_err(|e| format!("{}: {e}", wav.display()))?;
        inp.narration = Some(add(wav, false, &mut files));
    }
    for (src, name) in [(&cfg.voiceover, "--voiceover"), (&cfg.music, "--music")] {
        if let Some(f) = src {
            if !f.is_file() {
                return Err(format!("{name}: no file at {}", f.display()));
            }
        }
    }
    if let Some(vo) = &cfg.voiceover {
        inp.voiceover = Some(add(vo.clone(), false, &mut files));
    }
    if let Some(m) = &cfg.music {
        inp.music = Some(add(m.clone(), cfg.music_loop, &mut files));
    }
    let has_audio = !files.is_empty();
    if !has_audio && burn.is_empty() {
        return Ok(if report.is_empty() {
            Value::Null
        } else {
            Value::Object(report)
        });
    }

    let ffmpeg = crate::zoom::find_ffmpeg()?;
    let input_args = |files: &[(PathBuf, bool)]| -> Vec<String> {
        let mut a = Vec::new();
        for (f, looped) in files {
            if *looped {
                a.extend(["-stream_loop".to_string(), "-1".to_string()]);
            }
            a.push("-i".into());
            a.push(crate::zoom::arg_path(&f.display().to_string()));
        }
        a
    };

    // Pass one: measure the mix, audio only, with the inputs counted from zero.
    let mut loud = Loud::Off;
    if let (Some(target), true) = (cfg.loudness, has_audio) {
        let shift = |o: Option<usize>| o.map(|i| i - 1);
        let solo = Inputs {
            page: shift(inp.page),
            narration: shift(inp.narration),
            voiceover: shift(inp.voiceover),
            music: shift(inp.music),
        };
        let graph =
            audio_graph(&solo, cfg, p.dur, &Loud::Measure(target)).ok_or("nothing to measure")?;
        let o = Command::new(&ffmpeg)
            .args(["-hide_banner", "-nostats", "-loglevel", "info"])
            .args(input_args(&files))
            .args([
                "-filter_complex",
                &graph,
                "-map",
                "[aout]",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("spawn ffmpeg: {e}"))?;
        let err = String::from_utf8_lossy(&o.stderr);
        if !o.status.success() {
            return Err(format!(
                "ffmpeg could not measure the soundtrack, exit {}:\n{}",
                o.status,
                crate::zoom::stderr_tail(&o.stderr)
            ));
        }
        loud = match parse_measured(&err) {
            Some(m) => {
                report.insert("measured_lufs".into(), json!(m.i.parse::<f64>().ok()));
                Loud::Apply(target, m)
            }
            // Silence all the way through: nothing to bring up.
            None => Loud::Off,
        };
        report.insert("loudness".into(), json!(target));
    }

    // Pass two: everything into the take.
    let mut graph: Vec<String> = Vec::new();
    if let Some(a) = audio_graph(&inp, cfg, p.dur, &loud) {
        graph.push(a);
    }
    let cue_first = 1 + files.len();
    let burned = if burn.is_empty() {
        None
    } else {
        caption_graph(&cues, cue_first)
    };
    if let Some(v) = &burned {
        graph.push(v.clone());
    }
    let out = Path::new(p.out);
    let staged = out.with_file_name(format!(
        ".{}.sound.mp4",
        out.file_name().and_then(|n| n.to_str()).unwrap_or("kaviri")
    ));
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(crate::zoom::arg_path(p.out))
        .args(input_args(&files));
    for f in &burn {
        cmd.arg("-i")
            .arg(crate::zoom::arg_path(&f.display().to_string()));
    }
    let script = dir.join("graph.txt");
    std::fs::write(&script, graph.join(";")).map_err(|e| format!("{}: {e}", script.display()))?;
    cmd.args(["-filter_complex", &graph.join(";")]);
    if burned.is_some() {
        cmd.args([
            "-map", "[vout]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt",
            "yuv420p",
        ]);
    } else {
        cmd.args(["-map", "0:v:0", "-c:v", "copy"]);
    }
    if has_audio {
        cmd.args([
            "-map", "[aout]", "-c:a", "aac", "-b:a", "192k", "-ar", "48000",
        ]);
    }
    cmd.args(["-t", &format!("{:.3}", p.dur), "-movflags", "+faststart"])
        .arg(crate::zoom::arg_path(&staged.display().to_string()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let done = cmd.output().map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !done.status.success() {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "ffmpeg could not lay the soundtrack under the take, exit {}:\n{}",
            done.status,
            crate::zoom::stderr_tail(&done.stderr)
        ));
    }
    std::fs::rename(&staged, out).map_err(|e| format!("move the finished take into place: {e}"))?;
    let mut sources = Vec::new();
    for (k, on) in [
        ("page", inp.page.is_some()),
        ("narration", inp.narration.is_some()),
        ("voiceover", inp.voiceover.is_some()),
        ("music", inp.music.is_some()),
    ] {
        if on {
            sources.push(k);
        }
    }
    report.insert("sources".into(), json!(sources));
    if inp.narration.is_some() {
        report.insert("lines".into(), json!(p.clips.len()));
    }
    Ok(Value::Object(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Cfg {
        Cfg {
            tts: "none".into(),
            ..Cfg::default()
        }
    }

    #[test]
    fn a_wait_stretches_to_the_end_of_the_speech_in_flight_and_no_further() {
        assert_eq!(stretch_ms(800, 2.0, 4.5), 2500);
        assert_eq!(stretch_ms(3000, 2.0, 4.5), 3000);
        assert_eq!(stretch_ms(800, 5.0, 4.5), 800);
        assert_eq!(stretch_ms(0, 0.0, f64::NEG_INFINITY), 0);
    }

    #[test]
    fn a_clip_lands_on_the_sample_its_op_ran_at() {
        let mut buf = Vec::new();
        place_pcm(&mut buf, &[0.5, 0.5], 1.5, 1.0);
        let first = buf.iter().position(|s| *s != 0.0).unwrap();
        assert_eq!(first, 72_000);
        // Overlapping clips add rather than replace.
        place_pcm(&mut buf, &[0.25], 1.5, 2.0);
        assert_eq!(buf[72_000], 1.0);
        assert_eq!(buf.len(), 72_002);
        // A gain in dB.
        assert!((db(-6.0) - 0.501).abs() < 1e-3);
    }

    #[test]
    fn narration_alone_is_normalised_and_cut_to_the_video() {
        let inp = Inputs {
            narration: Some(1),
            ..Inputs::default()
        };
        let g = audio_graph(&inp, &cfg(), 12.5, &Loud::Measure(-16.0)).unwrap();
        assert!(g.starts_with("[1:a]aformat="), "{g}");
        assert!(g.contains("volume=0dB[narr]"), "{g}");
        assert!(g.contains("[narr]anull,loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json"));
        assert!(
            g.ends_with("apad=whole_dur=12.500,atrim=0:12.500,asetpts=PTS-STARTPTS[aout]"),
            "{g}"
        );
        assert!(!g.contains("sidechaincompress"));
    }

    #[test]
    fn music_ducks_under_speech_and_fades_at_both_ends() {
        let inp = Inputs {
            page: Some(1),
            narration: Some(2),
            voiceover: Some(3),
            music: Some(4),
        };
        let mut c = cfg();
        c.page_gain = -3.0;
        c.music_volume = -20.0;
        let g = audio_graph(&inp, &c, 30.0, &Loud::Off).unwrap();
        assert!(
            g.contains("[1:a]") && g.contains("volume=-3dB[page]"),
            "{g}"
        );
        assert!(g.contains("[narr][vo]amix=inputs=2:duration=longest:normalize=0[speech]"));
        assert!(
            g.contains("[speech]asplit=2[said][k];[k]apad=whole_dur=30.000[key]"),
            "{g}"
        );
        assert!(g.contains("[4:a]"), "{g}");
        assert!(g.contains("atrim=0:30.000"), "{g}");
        assert!(
            g.contains("volume=-20dB,afade=t=in:st=0:d=1.500,afade=t=out:st=28.500:d=1.500[bed]")
        );
        assert!(g.contains("[bed][key]sidechaincompress="), "{g}");
        assert!(g.contains("[page][said][music]amix=inputs=3"), "{g}");
        assert!(!g.contains("loudnorm"));
        // Ducking off: the bed goes straight to the bus.
        c.duck = false;
        let g = audio_graph(&inp, &c, 30.0, &Loud::Off).unwrap();
        assert!(
            !g.contains("sidechaincompress") && !g.contains("asplit"),
            "{g}"
        );
        assert!(g.contains("[page][speech][music]amix=inputs=3"), "{g}");
    }

    #[test]
    fn a_voiceover_shorter_than_the_take_does_not_cut_the_bed_off() {
        // sidechaincompress ends with its shorter input, so the key must last the take.
        let inp = Inputs {
            voiceover: Some(1),
            music: Some(2),
            ..Inputs::default()
        };
        let g = audio_graph(&inp, &cfg(), 20.0, &Loud::Off).unwrap();
        assert!(
            g.contains("[vo]asplit=2[said][k];[k]apad=whole_dur=20.000[key]"),
            "{g}"
        );
        assert!(g.contains("[bed][key]sidechaincompress="), "{g}");
        // No pad in the graph is unbounded.
        assert!(!g.contains("apad,") && !g.contains("apad["), "{g}");
    }

    #[test]
    fn music_with_nothing_to_duck_under_is_not_ducked_and_a_short_take_has_short_fades() {
        let inp = Inputs {
            music: Some(1),
            ..Inputs::default()
        };
        let g = audio_graph(&inp, &cfg(), 3.0, &Loud::Off).unwrap();
        assert!(!g.contains("sidechaincompress"), "{g}");
        assert!(g.contains("afade=t=in:st=0:d=1.000"), "{g}");
        assert_eq!(
            audio_graph(&Inputs::default(), &cfg(), 3.0, &Loud::Off),
            None
        );
    }

    #[test]
    fn the_second_loudness_pass_applies_what_the_first_measured() {
        let stderr = "[Parsed_loudnorm_3 @ 0x1]\n{\n\t\"input_i\" : \"-27.61\",\n\
                      \t\"input_tp\" : \"-9.12\",\n\t\"input_lra\" : \"4.10\",\n\
                      \t\"input_thresh\" : \"-38.02\",\n\t\"output_i\" : \"-16.2\",\n\
                      \t\"target_offset\" : \"0.21\"\n}\n";
        let m = parse_measured(stderr).unwrap();
        assert_eq!(m.i, "-27.61");
        let f = loudnorm(&Loud::Apply(-16.0, m)).unwrap();
        assert_eq!(
            f,
            "loudnorm=I=-16:TP=-1.5:LRA=11:measured_I=-27.61:measured_TP=-9.12:\
             measured_LRA=4.10:measured_thresh=-38.02:offset=0.21:linear=true"
        );
        // A silent mix measures -inf, and is left alone rather than handed back to loudnorm.
        let silent = stderr.replace("-27.61", "-inf");
        assert_eq!(parse_measured(&silent), None);
        assert_eq!(loudnorm(&Loud::Off), None);
        assert_eq!(Cfg::default().loudness, Some(-16.0));
    }

    #[test]
    fn each_caption_is_overlaid_only_for_its_seconds() {
        let cues = vec![
            Cue {
                start: 1.0,
                end: 2.5,
                text: "a".into(),
            },
            Cue {
                start: 3.0,
                end: 4.0,
                text: "b".into(),
            },
        ];
        let g = caption_graph(&cues, 3).unwrap();
        assert_eq!(
            g,
            "[0:v][3:v]overlay=0:0:enable='between(t,1.000,2.500)'[v0];\
             [v0][4:v]overlay=0:0:enable='between(t,3.000,4.000)'[vout]"
        );
        assert_eq!(caption_graph(&[], 1), None);
    }

    #[test]
    fn captions_come_from_what_the_clips_say() {
        let speech = Speech {
            wav: PathBuf::from("x.wav"),
            secs: 2.0,
            words: Vec::new(),
            timed: false,
        };
        let clips = vec![
            Clip {
                at: 4.0,
                speech: speech.clone(),
                gain_db: 0.0,
                text: Some("Watch this.".into()),
            },
            Clip {
                at: 8.0,
                speech,
                gain_db: 0.0,
                text: None,
            },
        ];
        let c = take_cues(&clips, 1920, 1080);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "Watch this.");
        assert_eq!(c[0].start, 4.0);
    }
}
