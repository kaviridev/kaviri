//! `kaviri mcp`: the op protocol as a Model Context Protocol server, on stdio.
//!
//! Nothing here is a second implementation of anything. Each interactive tool is one op,
//! run through the same `Session::exec` that `record` and `serve` use, against one browser
//! that lives across calls exactly as it does under `kaviri serve`. The whole-take tool
//! runs `kaviri record` itself, in a child process, so a script filmed through MCP and the
//! same script filmed on the command line are the same take.
//!
//! Three things the NDJSON protocol does not need and an MCP client does:
//!
//! - **Takes that outlast a tool call.** A client gives up on a call after a minute or two,
//!   and a take with `--slowmo 8` runs for many. `record_demo` answers at once with a take
//!   id and `get_take` reports on it.
//! - **A look at the page.** `inspect` lists what can be acted on and the selector for it,
//!   so an agent can write a script for an app it has never seen.
//! - **A look at the take.** An agent cannot watch an MP4. `preview` hands back a contact
//!   sheet of the moments the camera was meant to land, as an image it can read.
//!
//! stdout is the protocol channel, so the first thing `run` does is move it: the transport
//! gets a private copy of the descriptor and fd 1 becomes stderr, so a stray print anywhere
//! in the process lands in the log rather than in the middle of a JSON-RPC message.

use crate::ops::Session;
use crate::{launch_session, parse_argv_from, preflight, Args, Parsed, TICK};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What the server tells a client about itself when it connects. The short form of
/// AGENTS.md: the order things happen in, and the mistakes that cost a take.
const INSTRUCTIONS: &str = "\
kaviri films a web app from a script and renders an MP4 whose camera zooms onto each \
interaction. Two ways to use it:

1. Whole take (preferred for anything longer than a few steps): write the script as JSONL, one \
op per line, and call record_demo. It returns a take id at once; poll get_take until status is \
done, then call preview to see the moments the camera landed on.

2. Step by step: open, navigate, inspect (to find selectors), then start, the interactions \
(click, type, hover, press, scroll, wait, mark), and stop, which renders the MP4. One browser \
is kept across calls.

Narration: a say op ({\"op\":\"say\",\"text\":\"...\"}) speaks at that point in the take and \
the next wait stretches until the line is said, so actions land after the words that introduce \
them. record_demo takes music, voiceover and captions (burn draws them in; captions are also \
written as .srt and .vtt). tts none makes timing-only silence for a dry run.

Rules that save a take: start recording only after a wait that proves the page is ready; end \
with a wait of at least 1600 ms so the last zoom lands; if click says a selector is covered, \
the page is not in the state you think (dismiss the overlay); zoom_events: 0 in the result \
means nothing was framed and the script is broken. Selectors come from inspect, not from \
guessing.";

/// The interactive tools that are exactly one op, and the op each one sends.
pub const OP_TOOLS: &[(&str, &str)] = &[
    ("navigate", "navigate"),
    ("click", "click"),
    ("type", "type"),
    ("hover", "hover"),
    ("press", "press"),
    ("scroll", "scroll"),
    ("wait", "wait"),
    ("mark", "mark"),
    ("frame", "frame"),
    ("inspect", "inspect"),
    ("say", "say"),
    ("voice", "voice"),
    ("title", "title"),
    ("callout", "callout"),
];

/// The soundtrack flags, which `open` and `record_demo` both take.
const SOUND_FLAGS: &[(&str, &str)] = &[
    ("tts", "--tts"),
    ("voice", "--voice"),
    ("voiceover", "--voiceover"),
    ("voiceover_gain", "--voiceover-gain"),
    ("music", "--music"),
    ("music_volume", "--music-volume"),
    ("music_fade", "--music-fade"),
    ("music_loop", "--music-loop"),
    ("duck", "--duck"),
    ("page_gain", "--page-gain"),
    ("narration_gain", "--narration-gain"),
    ("loudness", "--loudness"),
    ("captions", "--captions"),
    ("caption_style", "--caption-style"),
];

/// The flags `open` accepts, by field name. The values are checked by the same parser the
/// command line goes through, so a bad preset fails here with the same list of presets.
pub const OPEN_FLAGS: &[(&str, &str)] = &[
    ("preset", "--preset"),
    ("width", "--width"),
    ("height", "--height"),
    ("scale", "--scale"),
    ("frame", "--frame"),
    ("frame_style", "--frame-style"),
    ("background", "--background"),
    ("cursor", "--cursor"),
    ("slowmo", "--slowmo"),
    ("audio", "--audio"),
];

/// The flags `record_demo` passes through to `kaviri record`.
pub const RECORD_FLAGS: &[(&str, &str)] = &[
    ("preset", "--preset"),
    ("frame", "--frame"),
    ("frame_style", "--frame-style"),
    ("background", "--background"),
    ("slowmo", "--slowmo"),
    ("audio", "--audio"),
];

/// What a tool call asks the server to do, decided without touching a browser.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Launch (or relaunch) the browser with these command-line flags, then go to `url`.
    Open {
        argv: Vec<String>,
        url: Option<String>,
    },
    /// One op through the session.
    Op(Value),
    /// `start_recording`, to `out` if given, else a new file in the takes directory.
    Start { out: Option<String> },
    /// `stop_recording`.
    Stop,
    /// Close the browser.
    Close,
    /// Film a whole script in the background.
    RecordDemo {
        script: String,
        argv: Vec<String>,
        out: Option<String>,
    },
    /// Report on one take, or list them all.
    GetTake(Option<String>),
    /// A contact sheet and poster for a take.
    Preview { id: Option<String>, frames: usize },
}

fn as_object<'a>(tool: &str, args: &'a Value) -> Result<&'a Map<String, Value>, String> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match args {
        Value::Null => Ok(EMPTY.get_or_init(Map::new)),
        Value::Object(m) => Ok(m),
        other => Err(format!("{tool}: arguments must be an object, got {other}")),
    }
}

/// A flag's value as the command line would spell it.
fn flag_value(field: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(format!("{field} must be a string or a number, got {other}")),
    }
}

/// Turn tool fields into command-line flags, refusing anything not in `table`.
pub fn flags_from(
    tool: &str,
    m: &Map<String, Value>,
    table: &[(&str, &str)],
    skip: &[&str],
) -> Result<Vec<String>, String> {
    let mut argv = Vec::new();
    for (k, v) in m {
        if skip.contains(&k.as_str()) || v.is_null() {
            continue;
        }
        let Some((_, flag)) = table.iter().chain(SOUND_FLAGS.iter()).find(|(f, _)| f == k) else {
            let mut known: Vec<&str> = table
                .iter()
                .chain(SOUND_FLAGS.iter())
                .map(|(f, _)| *f)
                .collect();
            known.extend_from_slice(skip);
            return Err(format!(
                "{tool}: no field \"{k}\"; it takes {}",
                known.join(", ")
            ));
        };
        // A switch is a bare flag: present when true, absent otherwise.
        if *flag == "--audio" {
            match v.as_bool() {
                Some(true) => argv.push(flag.to_string()),
                Some(false) => {}
                None => return Err(format!("{tool}: {k} must be true or false, got {v}")),
            }
            continue;
        }
        argv.push(flag.to_string());
        // An on/off flag takes a JSON boolean too, which is what an agent will send.
        let value = match v {
            Value::Bool(b) => (if *b { "on" } else { "off" }).to_string(),
            v => flag_value(k, v).map_err(|e| format!("{tool}: {e}"))?,
        };
        argv.push(value);
    }
    Ok(argv)
}

/// Check a script before anything is spawned for it: every line that is not blank or a
/// comment is a JSON object with an `op`. The same reader rules as `kaviri record`, so a
/// script that passes here parses there, and a typo is an answer in this call rather than
/// a failed take a poll later.
pub fn check_script(body: &str) -> Result<usize, String> {
    let mut n = 0;
    for (i, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("script line {}: {e}", i + 1))?;
        if !v["op"].is_string() {
            return Err(format!(
                "script line {}: each line is an object with an \"op\", got {line}",
                i + 1
            ));
        }
        n += 1;
    }
    if n == 0 {
        return Err("script_jsonl has no ops in it".into());
    }
    Ok(n)
}

/// The JSON-RPC to op mapping: which action a tool call is, and its fields as an op or as
/// flags. Pure, so the whole table is testable without a browser.
pub fn route(tool: &str, args: &Value) -> Result<Action, String> {
    let m = as_object(tool, args)?;
    if let Some((_, op)) = OP_TOOLS.iter().find(|(t, _)| *t == tool) {
        if m.contains_key("op") {
            return Err(format!(
                "{tool}: the tool name is the op; leave \"op\" out of its arguments"
            ));
        }
        let mut o = m.clone();
        o.insert("op".into(), json!(op));
        return Ok(Action::Op(Value::Object(o)));
    }
    let text = |k: &str| -> Result<Option<String>, String> {
        match m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(o) => Err(format!("{tool}: {k} must be a string, got {o}")),
        }
    };
    match tool {
        "open" => Ok(Action::Open {
            argv: flags_from(tool, m, OPEN_FLAGS, &["url"])?,
            url: text("url")?,
        }),
        "start" => {
            if let Some(k) = m.keys().find(|k| *k != "out") {
                return Err(format!("start: no field \"{k}\"; it takes out"));
            }
            let out = text("out")?;
            if let Some(o) = &out {
                if !o.ends_with(".mp4") {
                    return Err(format!("start: out must name an .mp4 file, got {o}"));
                }
            }
            Ok(Action::Start { out })
        }
        "stop" => Ok(Action::Stop),
        "close" => Ok(Action::Close),
        "record_demo" => {
            let script = text("script_jsonl")?.ok_or("record_demo needs script_jsonl")?;
            check_script(&script)?;
            let out = text("out")?;
            if let Some(o) = &out {
                if !o.ends_with(".mp4") {
                    return Err(format!("record_demo: out must name an .mp4 file, got {o}"));
                }
            }
            Ok(Action::RecordDemo {
                script,
                argv: flags_from(tool, m, RECORD_FLAGS, &["script_jsonl", "out"])?,
                out,
            })
        }
        "get_take" => Ok(Action::GetTake(text("id")?)),
        "preview" => {
            let frames = match m.get("frames") {
                None | Some(Value::Null) => 9,
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=16).contains(n))
                    .ok_or_else(|| format!("preview: frames must be 1 to 16, got {v}"))?
                    as usize,
            };
            Ok(Action::Preview {
                id: text("id")?,
                frames,
            })
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

// ---------------------------------------------------------------------------------------
// The take registry
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

/// Lines of a child's stderr kept for the report. The tail is where the reason is.
const LOG_TAIL: usize = 30;

pub struct Take {
    pub id: String,
    /// `record_demo` or `session`.
    pub kind: &'static str,
    pub status: Status,
    pub path: PathBuf,
    pub marks: Vec<Value>,
    /// The `recording_rendered` result, once there is one.
    pub rendered: Option<Value>,
    pub error: Option<String>,
    pub log: std::collections::VecDeque<String>,
    pub created: Instant,
    pub finished: Option<Instant>,
}

/// Every take this server has started, by id. Lives as long as the process: a take is a
/// file on disk, and the registry is only the index to it.
#[derive(Default)]
pub struct Takes {
    next: u64,
    map: BTreeMap<String, Take>,
    /// Ids in the order they were made, newest last.
    order: Vec<String>,
}

impl Takes {
    /// A new take, queued, writing to `path` (or to `dir/<id>.mp4`). Returns its id.
    pub fn create(&mut self, kind: &'static str, dir: &Path, path: Option<PathBuf>) -> String {
        self.next += 1;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // The time keeps ids from repeating across restarts, which matters because the id
        // is also the file name in the takes directory.
        let id = format!("take-{stamp}-{}", self.next);
        let path = path.unwrap_or_else(|| dir.join(format!("{id}.mp4")));
        self.map.insert(
            id.clone(),
            Take {
                id: id.clone(),
                kind,
                status: Status::Queued,
                path,
                marks: Vec::new(),
                rendered: None,
                error: None,
                log: Default::default(),
                created: Instant::now(),
                finished: None,
            },
        );
        self.order.push(id.clone());
        id
    }

    pub fn get(&self, id: &str) -> Option<&Take> {
        self.map.get(id)
    }

    pub fn set_running(&mut self, id: &str) {
        if let Some(t) = self.map.get_mut(id) {
            t.status = Status::Running;
        }
    }

    /// Fold one line of `kaviri record` stdout into the take: a mark filmed during the
    /// recording, the render result, or the error that ended the script.
    pub fn absorb(&mut self, id: &str, line: &str) {
        let Some(t) = self.map.get_mut(id) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return;
        };
        if v["ok"].as_bool() == Some(false) {
            let e = v["error"].as_str().unwrap_or("unknown error");
            t.error = Some(match (v["op"].as_str(), v["index"].as_u64()) {
                (Some(op), Some(i)) => format!("op {i} ({op}): {e}"),
                _ => e.to_string(),
            });
            return;
        }
        let r = &v["result"];
        match r["event"].as_str() {
            Some("mark") if r["recording"].as_bool() == Some(true) => {
                t.marks.push(json!({"t": r["t"], "kind": r["kind"],
                                    "label": r["label"], "box": r["box"]}));
            }
            Some("recording_rendered") => t.rendered = Some(r.clone()),
            _ => {}
        }
    }

    pub fn log(&mut self, id: &str, line: String) {
        if let Some(t) = self.map.get_mut(id) {
            if t.log.len() == LOG_TAIL {
                t.log.pop_front();
            }
            t.log.push_back(line);
        }
    }

    /// Close a take. A take is done when it rendered and nothing failed; a failed take may
    /// still have a partial video, which the report says.
    pub fn finish(&mut self, id: &str, ok: bool, error: Option<String>) {
        if let Some(t) = self.map.get_mut(id) {
            if t.error.is_none() {
                t.error = error;
            }
            t.status = if ok && t.rendered.is_some() && t.error.is_none() {
                Status::Done
            } else {
                if t.error.is_none() {
                    t.error = Some(match t.log.back() {
                        Some(l) => format!("the take did not render: {l}"),
                        None => "the take did not render".into(),
                    });
                }
                Status::Failed
            };
            t.finished = Some(Instant::now());
        }
    }

    /// Close an interactive take with what `stop_recording` returned.
    pub fn finish_session(&mut self, id: &str, r: &Result<Value, String>) {
        if let Some(t) = self.map.get_mut(id) {
            match r {
                Ok(v) => {
                    t.marks = v["marks"].as_array().cloned().unwrap_or_default();
                    let mut v = v.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.remove("marks");
                    }
                    t.rendered = Some(v);
                }
                Err(e) => t.error = Some(e.clone()),
            }
        }
        self.finish(id, r.is_ok(), None);
    }

    /// The newest take that finished with a video, for a `preview` with no id.
    pub fn latest_done(&self) -> Option<&Take> {
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.map.get(id))
            .find(|t| t.status == Status::Done)
    }

    /// The report `get_take` returns.
    pub fn view(&self, id: &str) -> Option<Value> {
        let t = self.map.get(id)?;
        let end = t.finished.unwrap_or_else(Instant::now);
        let mut v = json!({
            "id": t.id,
            "kind": t.kind,
            "status": t.status.name(),
            "path": t.path.display().to_string(),
            "elapsed_s": (end.duration_since(t.created).as_secs_f64() * 10.0).round() / 10.0,
            "marks": t.marks,
        });
        if let Some(r) = &t.rendered {
            for k in [
                "duration",
                "zoom_events",
                "frames",
                "audio",
                "soundtrack",
                "warning",
            ] {
                if !r[k].is_null() {
                    v[k] = r[k].clone();
                }
            }
        }
        if let Some(e) = &t.error {
            v["error"] = json!(e);
            v["video_exists"] = json!(t.path.exists());
        }
        if t.status != Status::Done && !t.log.is_empty() {
            v["log"] = json!(t.log.iter().collect::<Vec<_>>());
        }
        Some(v)
    }

    /// One line per take, newest first.
    pub fn list(&self) -> Value {
        Value::Array(
            self.order
                .iter()
                .rev()
                .filter_map(|id| self.map.get(id))
                .map(|t| {
                    json!({"id": t.id, "kind": t.kind, "status": t.status.name(),
                                "path": t.path.display().to_string()})
                })
                .collect(),
        )
    }
}

// ---------------------------------------------------------------------------------------
// Preview: keyframes at the marks
// ---------------------------------------------------------------------------------------

/// How long after an interaction the camera has settled on it. The spring takes most of a
/// second to land, and a frame taken at the mark itself shows the wide shot it left.
const SETTLE_S: f64 = 0.7;

/// Two keyframes closer than this show the same thing.
const MIN_GAP_S: f64 = 0.4;

/// The kinds of mark that are moments worth seeing. A `wait` is the absence of one.
fn is_moment(kind: &str) -> bool {
    matches!(
        kind,
        "click" | "type" | "hover" | "press" | "scroll" | "mark" | "navigate"
    )
}

/// Where in the video to take the keyframes: just after each interaction lands, thinned
/// evenly to `max`, and topped up with evenly spaced moments when a take has too few
/// interactions to make a useful sheet.
pub fn keyframe_times(marks: &[(f64, String)], duration: f64, max: usize) -> Vec<f64> {
    let max = max.max(1);
    let last = (duration - 0.05).max(0.0);
    let mut t: Vec<f64> = Vec::new();
    for (at, kind) in marks {
        if !is_moment(kind) || !at.is_finite() {
            continue;
        }
        let x = (at + SETTLE_S).clamp(0.0, last);
        if t.last().map_or(true, |p| x - p >= MIN_GAP_S) {
            t.push(x);
        }
    }
    if t.len() > max {
        let n = t.len();
        t = (0..max)
            .map(|i| {
                t[if max == 1 {
                    n - 1
                } else {
                    i * (n - 1) / (max - 1)
                }]
            })
            .collect();
    }
    let want = max.min(6);
    if t.len() < want && duration > 0.0 {
        for i in 0..want {
            if t.len() >= want {
                break;
            }
            let x = (duration * (i as f64 + 0.5) / want as f64).min(last);
            if t.iter().all(|p| (p - x).abs() >= MIN_GAP_S) {
                t.push(x);
            }
        }
        t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    }
    t
}

/// The poster: the moment the last real interaction has landed, which is usually the
/// payoff of the demo. The middle of the take when there is none.
pub fn poster_time(marks: &[(f64, String)], duration: f64) -> f64 {
    let last = (duration - 0.05).max(0.0);
    marks
        .iter()
        .rev()
        .find(|(_, k)| matches!(k.as_str(), "click" | "type" | "press" | "hover"))
        .map(|(t, _)| (t + 1.0).clamp(0.0, last))
        .unwrap_or(duration / 2.0)
}

/// Columns and rows for `n` tiles: one row up to three, then three across.
pub fn grid(n: usize) -> (usize, usize) {
    match n {
        0 | 1 => (1, 1),
        2 | 3 => (n, 1),
        4 => (2, 2),
        _ => (3, n.div_ceil(3)),
    }
}

fn ffmpeg_frame(ffmpeg: &str, mp4: &Path, t: f64, vf: &str, out: &Path) -> Result<(), String> {
    let o = Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-ss", &format!("{t:.3}"), "-i"])
        .arg(mp4)
        .args(["-frames:v", "1", "-vf", vf, "-q:v", "3"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() || !out.exists() {
        return Err(format!(
            "ffmpeg could not take a frame at {t:.2}s: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(())
}

/// Write `<take>.sheet.jpg` and `<take>.poster.jpg` beside the MP4. Returns their paths and
/// the times the sheet's tiles were taken at, left to right and top to bottom.
pub fn make_preview(
    mp4: &Path,
    marks: &[(f64, String)],
    duration: f64,
    max: usize,
) -> Result<(PathBuf, PathBuf, Vec<f64>), String> {
    if !mp4.exists() {
        return Err(format!("no video at {}", mp4.display()));
    }
    let ffmpeg = crate::zoom::find_ffmpeg()?;
    let times = keyframe_times(marks, duration, max);
    let stem = mp4.with_extension("");
    let sheet = PathBuf::from(format!("{}.sheet.jpg", stem.display()));
    let poster = PathBuf::from(format!("{}.poster.jpg", stem.display()));
    // One directory per call: two previews running at once must not share (and then
    // delete) each other's tiles.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tiles = crate::cdp::session_dir()?.join(format!("preview-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&tiles).map_err(|e| format!("{}: {e}", tiles.display()))?;
    let result = (|| {
        // Landscape tiles 480 wide, portrait ones 270, so a sheet of nine stays readable
        // and small enough to send back as an image.
        let tile_vf = "scale=w='if(gt(iw,ih),480,270)':h=-2";
        for (i, t) in times.iter().enumerate() {
            ffmpeg_frame(
                &ffmpeg,
                mp4,
                *t,
                tile_vf,
                &tiles.join(format!("{i:02}.jpg")),
            )?;
        }
        let (c, r) = grid(times.len());
        let o = Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-start_number", "0", "-i"])
            .arg(tiles.join("%02d.jpg"))
            .args([
                "-vf",
                &format!("tile={c}x{r}:padding=6:margin=6:color=0x1c1c22"),
                "-frames:v",
                "1",
                "-q:v",
                "3",
            ])
            .arg(&sheet)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        if !o.status.success() || !sheet.exists() {
            return Err(format!(
                "ffmpeg could not tile the contact sheet: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        ffmpeg_frame(
            &ffmpeg,
            mp4,
            poster_time(marks, duration),
            "scale=w='min(iw,1280)':h=-2",
            &poster,
        )
    })();
    let _ = std::fs::remove_dir_all(&tiles);
    result.map(|()| (sheet, poster, times))
}

/// Marks as (t, kind), from the JSON a take keeps.
fn mark_pairs(marks: &[Value]) -> Vec<(f64, String)> {
    marks
        .iter()
        .filter_map(|m| Some((m["t"].as_f64()?, m["kind"].as_str()?.to_string())))
        .collect()
}

// ---------------------------------------------------------------------------------------
// The browser thread
// ---------------------------------------------------------------------------------------

enum Cmd {
    Open(Box<Args>, Option<String>),
    Op(Value),
    Close,
}

struct Job {
    cmd: Cmd,
    reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
}

/// The flags `kaviri mcp` was started with, then a tool's own on top. The parser reads them
/// in order, so the tool's win, the way a flag after `--preset` wins on the command line.
fn args_from(base: &[String], argv: Vec<String>) -> Result<Args, String> {
    let all = std::iter::once("mcp".to_string())
        .chain(base.iter().cloned())
        .chain(argv);
    match parse_argv_from(all)? {
        Parsed::Run(a) => Ok(*a),
        Parsed::Help(m) | Parsed::Error(m) => Err(m),
    }
}

fn handle(sess: &mut Option<Session>, cmd: Cmd, default: &Args) -> Result<Value, String> {
    match cmd {
        Cmd::Open(a, url) => {
            if sess.as_ref().is_some_and(|s| s.cdp.is_recording()) {
                return Err(
                    "a take is recording in this browser; call stop before opening another".into(),
                );
            }
            // The old browser goes first, so there is never a second one running.
            *sess = None;
            preflight()?;
            let s = sess.insert(launch_session(&a)?);
            let mut out = json!({"event": "opened", "viewport": [s.css_w, s.css_h],
                                 "video": [s.out_size.0, s.out_size.1],
                                 "frame": s.frame.as_ref().map(|f| f.os.name())});
            if let Some(u) = url {
                out["navigate"] = s.exec(&json!({"op": "navigate", "url": u}))?;
            }
            Ok(out)
        }
        Cmd::Close => {
            let Some(s) = sess.as_mut() else {
                return Ok(json!({"event": "closed", "note": "no browser was open"}));
            };
            // Closing over a take renders it rather than throwing the frames away.
            let rendered = if s.cdp.is_recording() {
                Some(s.exec(&json!({"op": "stop_recording"})))
            } else {
                None
            };
            *sess = None;
            match rendered {
                Some(Err(e)) => Err(format!("closed; the open take did not render: {e}")),
                Some(Ok(v)) => Ok(json!({"event": "closed", "rendered": v})),
                None => Ok(json!({"event": "closed"})),
            }
        }
        Cmd::Op(op) => {
            if sess.is_none() {
                preflight()?;
                *sess = Some(launch_session(default)?);
            }
            let s = sess.as_mut().expect("launched above");
            let stopping = op["op"] == "stop_recording";
            let mut v = s.exec(&op)?;
            if stopping {
                v["marks"] = s.marks_json();
            }
            Ok(v)
        }
    }
}

/// Own the browser for the life of the server. Ops arrive one at a time; between them the
/// capture clock keeps running, exactly as it does between lines under `kaviri serve`.
fn browser_loop(rx: mpsc::Receiver<Job>, default: Args) {
    let mut sess: Option<Session> = None;
    loop {
        if crate::cdp::shutting_down() {
            break;
        }
        match rx.recv_timeout(TICK) {
            Ok(job) => {
                let r = handle(&mut sess, job.cmd, &default);
                let _ = job.reply.send(r);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(s) = sess.as_mut() {
                    let _ = s.cdp.tick();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // The client went away mid-take: render what was filmed rather than drop it.
    if let Some(s) = sess.as_mut() {
        if s.cdp.is_recording() {
            match s.exec(&json!({"op": "stop_recording"})) {
                Ok(v) => eprintln!("kaviri: rendered the open take on exit: {v}"),
                Err(e) => eprintln!("kaviri: the open take did not render on exit: {e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------------------

#[derive(Clone)]
struct Kaviri {
    tx: mpsc::Sender<Job>,
    takes: Arc<Mutex<Takes>>,
    /// The interactive take in flight, between `start` and `stop`.
    current: Arc<Mutex<Option<String>>>,
    /// Held by a `record_demo` while its browser runs, so takes queue rather than race.
    slot: Arc<Mutex<()>>,
    dir: PathBuf,
    /// The flags the server was started with: the defaults under every browser it opens,
    /// including the ones `record_demo` runs.
    base: Arc<Vec<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn envelope(r: Result<Value, String>) -> CallToolResult {
    match r {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(
            json!({"ok": true, "result": v}).to_string(),
        )]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(
            json!({"ok": false, "error": e}).to_string(),
        )]),
    }
}

/// Where takes land: `$KAVIRI_TAKES_DIR`, or `kaviri-takes` in the directory the client
/// started the server in, which for a coding agent is the project.
fn takes_dir() -> PathBuf {
    let base = crate::env::var_os("TAKES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("kaviri-takes"));
    if base.is_absolute() {
        base
    } else {
        std::env::current_dir().unwrap_or_default().join(base)
    }
}

fn absolute(p: &str) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

impl Kaviri {
    async fn browser(&self, cmd: Cmd) -> Result<Value, String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Job { cmd, reply })
            .map_err(|_| "the browser thread has stopped".to_string())?;
        rx.await
            .map_err(|_| "the browser thread stopped mid-op".to_string())?
    }

    fn ensure_dir(&self, path: &Path) -> Result<(), String> {
        match path.parent() {
            Some(d) => std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display())),
            None => Ok(()),
        }
    }

    async fn start(&self, out: Option<String>) -> Result<Value, String> {
        if let Some(id) = lock(&self.current).clone() {
            return Err(format!(
                "already recording {id}; call stop to finish it before starting another"
            ));
        }
        let id = lock(&self.takes).create("session", &self.dir, out.as_deref().map(absolute));
        let path = lock(&self.takes)
            .get(&id)
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.ensure_dir(&path)?;
        let r = self
            .browser(Cmd::Op(json!({"op": "start_recording",
                                     "path": path.display().to_string()})))
            .await;
        match r {
            Ok(mut v) => {
                lock(&self.takes).set_running(&id);
                *lock(&self.current) = Some(id.clone());
                v["take_id"] = json!(id);
                v["path"] = json!(path.display().to_string());
                Ok(v)
            }
            Err(e) => {
                lock(&self.takes).finish(&id, false, Some(e.clone()));
                Err(e)
            }
        }
    }

    async fn stop(&self) -> Result<Value, String> {
        let r = self.browser(Cmd::Op(json!({"op": "stop_recording"}))).await;
        if let Some(id) = lock(&self.current).take() {
            lock(&self.takes).finish_session(&id, &r);
            return r.map(|mut v| {
                v["take_id"] = json!(id);
                v
            });
        }
        r
    }

    fn record_demo(
        &self,
        script: String,
        argv: Vec<String>,
        out: Option<String>,
    ) -> Result<Value, String> {
        let exe =
            std::env::current_exe().map_err(|e| format!("cannot find the kaviri binary: {e}"))?;
        let id = lock(&self.takes).create("record_demo", &self.dir, out.as_deref().map(absolute));
        let path = lock(&self.takes)
            .get(&id)
            .map(|t| t.path.clone())
            .unwrap_or_default();
        self.ensure_dir(&path)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let script_path = self.dir.join(format!("{id}.jsonl"));
        std::fs::write(&script_path, &script)
            .map_err(|e| format!("{}: {e}", script_path.display()))?;
        let takes = Arc::clone(&self.takes);
        let slot = Arc::clone(&self.slot);
        let tid = id.clone();
        let out_path = path.clone();
        let base = Arc::clone(&self.base);
        std::thread::spawn(move || {
            // One recording browser at a time. A second take queues behind the first rather
            // than doubling the memory and halving the frame rate of both.
            let _slot = lock(&slot);
            lock(&takes).set_running(&tid);
            let child = Command::new(exe)
                .arg("record")
                .arg("--script")
                .arg(&script_path)
                .arg("--out")
                .arg(&out_path)
                .args(base.iter())
                .args(&argv)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    lock(&takes).finish(
                        &tid,
                        false,
                        Some(format!("could not start kaviri record: {e}")),
                    );
                    return;
                }
            };
            let err_reader = child.stderr.take().map(|err| {
                let takes = Arc::clone(&takes);
                let tid = tid.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(err).lines().map_while(Result::ok) {
                        lock(&takes).log(&tid, line);
                    }
                })
            });
            if let Some(o) = child.stdout.take() {
                for line in BufReader::new(o).lines().map_while(Result::ok) {
                    lock(&takes).absorb(&tid, &line);
                }
            }
            if let Some(h) = err_reader {
                let _ = h.join();
            }
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            lock(&takes).finish(&tid, ok, None);
        });
        Ok(
            json!({"event": "take_queued", "take_id": id, "path": path.display().to_string(),
                  "ops": check_script(&script).unwrap_or(0),
                  "next": "poll get_take with this id until status is done or failed"}),
        )
    }

    fn get_take(&self, id: Option<String>) -> Result<Value, String> {
        let t = lock(&self.takes);
        match id {
            None => Ok(json!({"takes": t.list()})),
            Some(id) => t
                .view(&id)
                .ok_or_else(|| format!("no take {id}; this server knows {}", t.list())),
        }
    }

    async fn preview(&self, id: Option<String>, frames: usize) -> CallToolResult {
        let found = {
            let t = lock(&self.takes);
            let take = match &id {
                Some(id) => t.get(id),
                None => t.latest_done(),
            };
            take.map(|k| {
                (
                    k.id.clone(),
                    k.status,
                    k.path.clone(),
                    {
                        // Marks are on the take's clock; an intro title moves the take later
                        // in the finished video by this much.
                        let lead = k
                            .rendered
                            .as_ref()
                            .and_then(|r| r["motion"]["take_starts_at"].as_f64())
                            .unwrap_or(0.0);
                        mark_pairs(&k.marks)
                            .into_iter()
                            .map(|(t, kind)| (t + lead, kind))
                            .collect::<Vec<_>>()
                    },
                    k.rendered.as_ref().and_then(|r| r["duration"].as_f64()),
                )
            })
        };
        let Some((id, status, path, marks, duration)) = found else {
            return envelope(Err(match id {
                Some(id) => format!("no take {id}"),
                None => "no finished take yet; record one first".into(),
            }));
        };
        if status != Status::Done {
            return envelope(Err(format!(
                "take {id} is {}; preview needs a finished take",
                status.name()
            )));
        }
        let duration = duration.unwrap_or(0.0);
        let p = path.clone();
        let made = tokio::task::spawn_blocking(move || make_preview(&p, &marks, duration, frames))
            .await
            .map_err(|e| format!("preview: {e}"))
            .and_then(|r| r);
        let (sheet, poster, times) = match made {
            Ok(x) => x,
            Err(e) => return envelope(Err(e)),
        };
        let b64 = |p: &Path| -> Result<String, String> {
            use base64::Engine;
            std::fs::read(p)
                .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        let (s64, p64) = match (b64(&sheet), b64(&poster)) {
            (Ok(s), Ok(p)) => (s, p),
            (Err(e), _) | (_, Err(e)) => return envelope(Err(e)),
        };
        let (c, r) = grid(times.len());
        let info = json!({"ok": true, "result": {
            "event": "preview", "take_id": id,
            "sheet": sheet.display().to_string(), "poster": poster.display().to_string(),
            "grid": [c, r],
            "times": times.iter().map(|t| (t * 100.0).round() / 100.0).collect::<Vec<_>>(),
            "note": "tiles are left to right, top to bottom, each taken just after an \
                     interaction landed; the second image is the poster"}});
        CallToolResult::success(vec![
            ContentBlock::text(info.to_string()),
            ContentBlock::image(s64, "image/jpeg"),
            ContentBlock::image(p64, "image/jpeg"),
        ])
    }

    async fn call(&self, tool: &str, args: &Value) -> Result<CallToolResult, ErrorData> {
        let action = match route(tool, args) {
            Ok(a) => a,
            Err(e) if e.starts_with("unknown tool") => {
                return Err(ErrorData::invalid_params(e, None));
            }
            Err(e) => return Ok(envelope(Err(e))),
        };
        Ok(match action {
            Action::Open { argv, url } => match args_from(&self.base, argv) {
                Ok(a) => envelope(self.browser(Cmd::Open(Box::new(a), url)).await),
                Err(e) => envelope(Err(e)),
            },
            Action::Op(op) => envelope(self.browser(Cmd::Op(op)).await),
            Action::Start { out } => envelope(self.start(out).await),
            Action::Stop => envelope(self.stop().await),
            Action::Close => {
                let r = self.browser(Cmd::Close).await;
                if let Some(id) = lock(&self.current).take() {
                    let rendered = r
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("rendered").cloned())
                        .ok_or_else(|| "closed before it rendered".to_string());
                    lock(&self.takes).finish_session(&id, &rendered);
                }
                envelope(r)
            }
            Action::RecordDemo { script, argv, out } => {
                // Flags are checked here, by the real parser, so a bad preset is this call's
                // error and not a failed take found on the next poll.
                if let Err(e) = args_from(&self.base, argv.clone()) {
                    return Ok(envelope(Err(e)));
                }
                envelope(self.record_demo(script, argv, out))
            }
            Action::GetTake(id) => envelope(self.get_take(id)),
            Action::Preview { id, frames } => self.preview(id, frames).await,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------------------

fn schema(props: Value, required: &[&str]) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!("object"));
    m.insert("properties".into(), props);
    if !required.is_empty() {
        m.insert("required".into(), json!(required));
    }
    m
}

fn ms(desc: &str) -> Value {
    json!({"type": "number", "minimum": 0, "description": desc})
}

/// Every tool, with the description an agent decides from. Kept next to `route`, and the
/// tests check the two agree.
pub fn tools() -> Vec<Tool> {
    let sel =
        json!({"type": "string", "description": "CSS selector; inspect lists ones that work"});
    let x =
        json!({"type": "number", "description": "viewport x in CSS pixels, instead of a selector"});
    let y =
        json!({"type": "number", "description": "viewport y in CSS pixels, instead of a selector"});
    let mut frame_props = Map::new();
    for (f, flag) in crate::device::FRAME_OP_FIELDS {
        frame_props.insert(
            f.to_string(),
            json!({"type": ["string", "number", "boolean"], "description": format!("as {flag}")}),
        );
    }
    frame_props.insert(
        "platform".into(),
        json!({"type": "string", "enum": ["macos", "windows", "linux", "ios", "android",
                                          "android-emulator", "ios-simulator", "none"],
               "description": "the device the take is filmed on"}),
    );
    let defs: Vec<(&str, &str, Map<String, Value>)> = vec![
        ("open",
         "Launch the browser (or relaunch it with new settings) and optionally go to a URL. \
          Settings are fixed for the life of the browser: preset sets viewport and video size \
          (desktop, tiktok, square, landscape, readme, phone), frame films it on a device. Other \
          tools open a default desktop browser if this was never called.",
         schema(json!({
            "url": {"type": "string", "description": "page to load; a bare path becomes file://"},
            "preset": {"type": "string", "description": "desktop, tiktok, reels, shorts, square, landscape, readme or phone"},
            "width": {"type": "integer", "description": "viewport width in CSS px"},
            "height": {"type": "integer", "description": "viewport height in CSS px"},
            "scale": {"type": "number", "description": "capture scale, 0.5 to 4 (default 2)"},
            "frame": {"type": "string", "description": "macos, windows, linux, ios, android, android-emulator, ios-simulator"},
            "frame_style": {"type": "string", "description": "browser, app or recording"},
            "background": {"type": "string", "description": "backdrop name, auto, none, or an image path"},
            "cursor": {"type": "string", "description": "auto, arrow, hand, text, touch or none"},
            "slowmo": {"type": "number", "description": "1 to 16; for canvas-heavy pages that film choppy"},
            "audio": {"type": "boolean", "description": "record what the page plays"},
            "tts": {"type": "string", "description": "piper (local, default), none (silent timing only), openai, elevenlabs, cloudflare, command (KAVIRI_TTS_COMMAND). A say op's voice can also name its engine: \"cloudflare:arcas\""},
            "voice": {"type": "string", "description": "default voice for say"},
            "voiceover": {"type": "string", "description": "audio file laid under the take"},
            "voiceover_gain": {"type": "number"},
            "music": {"type": "string", "description": "music bed file, looped, faded, ducked under speech"},
            "music_volume": {"type": "number", "description": "dB, default -18"},
            "music_fade": {"type": "number", "description": "ms, default 1500"},
            "music_loop": {"type": "boolean"},
            "duck": {"type": "boolean"},
            "page_gain": {"type": "number"},
            "narration_gain": {"type": "number"},
            "loudness": {"type": ["number", "string"], "description": "LUFS target, default -16, or off"},
            "captions": {"type": "string", "description": "auto, off, file (.srt and .vtt) or burn"},
            "caption_style": {"type": "string", "description": "pill, bold or plain"}
         }), &[])),
        ("navigate", "Load a URL and wait for it. A bare path becomes a file:// URL.",
         schema(json!({"url": {"type": "string"},
                       "timeout_ms": ms("load budget, default 25000")}), &["url"])),
        ("inspect",
         "List the visible actionable elements on the page: role, accessible name, a CSS \
          selector that matches only that element, and its box [x, y, w, h] in CSS px. \
          covered names an element on top of it, which click would refuse. Call this before \
          writing selectors. all: true includes elements scrolled out of view.",
         schema(json!({"limit": {"type": "integer", "minimum": 1, "description": "default 150"},
                       "all": {"type": "boolean"}}), &[])),
        ("start",
         "Start filming. Send it after the wait that proves the page is ready. Returns the \
          take id and the MP4 path the take will render to.",
         schema(json!({"out": {"type": "string", "description": "MP4 path; default a new file in kaviri-takes/"}}), &[])),
        ("click", "Click an element (selector) or a point (x, y). The camera zooms onto it.",
         schema(json!({"selector": sel, "x": x, "y": y}), &[])),
        ("type",
         "Type text, into selector (clicked first) or into whatever has focus. The camera \
          follows the caret.",
         schema(json!({"selector": sel, "text": {"type": "string"},
                       "typewriter_ms": ms("per character, default 18")}), &["text"])),
        ("hover",
         "Glide the mouse onto an element or point, so hover effects play. at aims at a \
          fraction [fx, fy] of the element's box.",
         schema(json!({"selector": sel, "x": x, "y": y,
                       "at": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
                       "ms": ms("glide time, default 500")}), &[])),
        ("press",
         "Press a key or chord on whatever has focus: Enter, ArrowDown, ?, Meta+K, Meta+Shift+P.",
         schema(json!({"key": {"type": "string"}, "repeat": {"type": "integer", "minimum": 1},
                       "interval_ms": ms("between repeats"), "hold_ms": ms("how long it is held")}),
                &["key"])),
        ("scroll", "Scroll the page to an absolute y offset in CSS px (not a delta).",
         schema(json!({"y": {"type": "number", "minimum": 0}, "smooth": {"type": "boolean"}}), &["y"])),
        ("wait",
         "Pause for ms, or wait for selector to become visible (bounded by timeout_ms, default \
          20000). Never both. visible: false waits for presence only.",
         schema(json!({"ms": ms("a fixed pause"), "selector": sel,
                       "timeout_ms": ms("selector budget"), "visible": {"type": "boolean"}}), &[])),
        ("mark", "Put a label in the take's telemetry. Nothing is drawn.",
         schema(json!({"label": {"type": "string"}}), &["label"])),
        ("frame",
         "Film the take on a device. Must come before start. A phone platform also switches \
          the browser to that phone's viewport and user agent unless open set a size.",
         schema(Value::Object(frame_props), &["platform"])),
        ("say",
         "Say a line of narration at this point in the take. It returns once the line is \
          synthesised; the next wait stretches until it has been said, so the action after it \
          lands after the words. wait: true holds here until it is said. Returns secs and \
          until (the take time it ends). Under captions it becomes caption cues.",
         schema(json!({"text": {"type": "string"},
                       "voice": {"type": "string", "description": "engine voice; default the server's"},
                       "wait": {"type": "boolean"},
                       "pad_ms": ms("extra pause after the line when wait is true, default 150"),
                       "gain_db": {"type": "number", "description": "-60 to 24"}}), &["text"])),
        ("voice",
         "Drop a pre-recorded clip (wav, mp3, m4a) into the take at this point. text, if given, \
          is what it says, for captions.",
         schema(json!({"file": {"type": "string"}, "text": {"type": "string"},
                       "wait": {"type": "boolean"},
                       "gain_db": {"type": "number", "description": "-60 to 24"}}), &["file"])),
        ("title",
         "Add a full-screen animated title card. Sent before start it is an intro; at: \"end\" \
          makes it an outro. Styles: launch (words blur in over a glow, default), bold (big words \
          pop in on a solid colour), terminal (typed at a prompt). html is your own animated page \
          instead: CSS animations and window.kvFrame(t) are rendered frame by frame.",
         schema(json!({"text": {"type": "string"}, "sub": {"type": "string"},
                       "kicker": {"type": "string", "description": "small line above, e.g. Introducing"},
                       "style": {"type": "string", "enum": ["launch", "bold", "terminal"]},
                       "theme": {"type": "string", "enum": ["dark", "light"]},
                       "accent": {"type": "string", "description": "CSS colour"},
                       "at": {"type": "string", "enum": ["start", "end"]},
                       "ms": ms("how long the card lasts, 600 to 15000, default 2600"),
                       "html": {"type": "string"}}), &[])),
        ("callout",
         "Draw an animated callout over the take from this moment, during a take only. Styles: \
          lower-third (default), badge (a pill at the top), big (one line over a dimmed frame). \
          It does not pause the script: follow it with a wait as long as you want it seen.",
         schema(json!({"text": {"type": "string"}, "sub": {"type": "string"},
                       "style": {"type": "string", "enum": ["lower-third", "badge", "big"]},
                       "accent": {"type": "string", "description": "CSS colour"},
                       "ms": ms("how long it shows, default 2400"),
                       "html": {"type": "string"}}), &[])),
        ("stop",
         "Stop filming and render the MP4. Returns path, duration, zoom_events (0 means nothing \
          was framed: a broken script), the marks and the take id for preview.",
         schema(json!({}), &[])),
        ("close", "Close the browser. An open take is rendered first.", schema(json!({}), &[])),
        ("record_demo",
         "Film a whole script in the background and return a take id at once. script_jsonl is \
          one op per line, as for kaviri record: {\"op\":\"navigate\",\"url\":\"...\"}, \
          {\"op\":\"wait\",\"selector\":\"#app\"}, {\"op\":\"start_recording\"}, click, type, \
          hover, press, scroll, mark, say (narration), and a final {\"op\":\"wait\",\"ms\":1600}. A first line \
          {\"op\":\"frame\",\"platform\":\"ios\"} films it on a device. Poll get_take. Takes run \
          one at a time; later ones queue.",
         schema(json!({
            "script_jsonl": {"type": "string"},
            "preset": {"type": "string", "description": "desktop, tiktok, square, landscape, readme, phone"},
            "frame": {"type": "string", "description": "device frame, as open"},
            "frame_style": {"type": "string"},
            "background": {"type": "string"},
            "slowmo": {"type": "number"},
            "audio": {"type": "boolean"},
            "tts": {"type": "string", "description": "piper (local, default), none (silent timing only), openai, elevenlabs, cloudflare, command (KAVIRI_TTS_COMMAND). A say op's voice can also name its engine: \"cloudflare:arcas\""},
            "voice": {"type": "string", "description": "default voice for say"},
            "voiceover": {"type": "string", "description": "audio file laid under the take"},
            "voiceover_gain": {"type": "number"},
            "music": {"type": "string", "description": "music bed file, looped, faded, ducked under speech"},
            "music_volume": {"type": "number", "description": "dB, default -18"},
            "music_fade": {"type": "number", "description": "ms, default 1500"},
            "music_loop": {"type": "boolean"},
            "duck": {"type": "boolean"},
            "page_gain": {"type": "number"},
            "narration_gain": {"type": "number"},
            "loudness": {"type": ["number", "string"], "description": "LUFS target, default -16, or off"},
            "captions": {"type": "string", "description": "auto, off, file (.srt and .vtt) or burn"},
            "caption_style": {"type": "string", "description": "pill, bold or plain"},
            "out": {"type": "string", "description": "MP4 path; default kaviri-takes/<id>.mp4"}
         }), &["script_jsonl"])),
        ("get_take",
         "Status of a take: queued, running, done or failed, with the MP4 path, duration, \
          zoom_events and marks. A failed take says why and whether a partial video exists. \
          With no id, lists every take.",
         schema(json!({"id": {"type": "string"}}), &[])),
        ("preview",
         "A contact sheet of keyframes taken just after each interaction landed, plus a \
          poster frame, as images, so you can check the camera framed what you meant before \
          handing the take on. Defaults to the newest finished take.",
         schema(json!({"id": {"type": "string"},
                       "frames": {"type": "integer", "minimum": 1, "maximum": 16, "description": "tiles, default 9"}}),
                &[])),
    ];
    defs.into_iter()
        .map(|(n, d, s)| Tool::new(n, d, Arc::new(s)))
        .collect()
}

impl ServerHandler for Kaviri {
    fn get_info(&self) -> InitializeResult {
        let mut me = Implementation::new("kaviri", env!("CARGO_PKG_VERSION"));
        me.title = Some("kaviri".into());
        me.website_url = Some("https://kaviri.dev".into());
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(me)
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        self.call(&request.name, &args).await.map(Into::into)
    }
}

/// Give the protocol its own copy of stdout, and point fd 1 at stderr for everything else.
#[cfg(unix)]
fn private_stdout() -> Result<tokio::fs::File, String> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: dup and dup2 on the process's own standard descriptors. The duplicate is
    // owned by the File from here on and closed exactly once, when it drops.
    unsafe {
        let fd = libc::dup(1);
        if fd < 0 {
            return Err(format!("dup stdout: {}", std::io::Error::last_os_error()));
        }
        if libc::dup2(2, 1) < 0 {
            return Err(format!("dup2 stderr: {}", std::io::Error::last_os_error()));
        }
        Ok(tokio::fs::File::from_std(std::fs::File::from_raw_fd(fd)))
    }
}

pub fn run(a: &Args) -> Result<(), String> {
    if a.port.is_some() {
        return Err("kaviri mcp speaks over stdio only; drop --port".into());
    }
    if a.script.is_some() {
        return Err("kaviri mcp takes its scripts from record_demo; drop --script".into());
    }
    // Everything after `mcp` on the command line, so a `record_demo` child and every
    // browser opened later start from the same flags this one was given.
    let base: Vec<String> = std::env::args().skip(2).collect();
    // Passed on to every `record_demo` child after its own `--out`, a server-wide one
    // would win and send every take to the same file. Each take names its own.
    if base.iter().any(|f| f == "--out") {
        return Err("kaviri mcp names each take's file itself; drop --out \
                    (use $KAVIRI_TAKES_DIR, or out on start and record_demo)"
            .into());
    }
    // Before anything can print: from here on, stdout belongs to the protocol.
    let out = private_stdout()?;
    let default = a.clone();
    let (tx, rx) = mpsc::channel::<Job>();
    let browser = std::thread::spawn(move || browser_loop(rx, default));
    let server = Kaviri {
        tx,
        takes: Arc::new(Mutex::new(Takes::default())),
        current: Arc::new(Mutex::new(None)),
        slot: Arc::new(Mutex::new(())),
        dir: takes_dir(),
        base: Arc::new(base),
    };
    eprintln!(
        "kaviri {}: MCP server on stdio; takes go to {}",
        env!("CARGO_PKG_VERSION"),
        server.dir.display()
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("tokio: {e}"))?;
    let r = rt.block_on(async move {
        let running = server
            .serve((tokio::io::stdin(), out))
            .await
            .map_err(|e| format!("mcp: {e}"))?;
        // A signal sets a flag rather than killing the process (every cleanup is a Drop),
        // so watch for it alongside the transport closing.
        let watch = async {
            while !crate::cdp::shutting_down() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        };
        tokio::select! {
            r = running.waiting() => r.map(|_| ()).map_err(|e| format!("mcp: {e}")),
            _ = watch => Ok(()),
        }
    });
    // The server held the only sender; with it gone the browser thread renders any open
    // take, closes the browser and returns.
    rt.shutdown_timeout(Duration::from_secs(1));
    let _ = browser.join();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_op_tool_sends_its_op_with_the_fields_untouched() {
        let a = route("click", &json!({"selector": "#buy"})).unwrap();
        assert_eq!(a, Action::Op(json!({"op": "click", "selector": "#buy"})));
        let a = route("wait", &json!({"selector": ".done", "timeout_ms": 60000})).unwrap();
        assert_eq!(
            a,
            Action::Op(json!({"op": "wait", "selector": ".done", "timeout_ms": 60000}))
        );
        let a = route("inspect", &Value::Null).unwrap();
        assert_eq!(a, Action::Op(json!({"op": "inspect"})));
        for (tool, op) in OP_TOOLS {
            match route(tool, &json!({})).unwrap() {
                Action::Op(v) => assert_eq!(v["op"], json!(op)),
                other => panic!("{tool} routed to {other:?}"),
            }
        }
    }

    #[test]
    fn an_op_field_in_the_arguments_is_refused_not_obeyed() {
        // Otherwise "click" with {"op":"stop_recording"} would be a different op entirely.
        let e = route("click", &json!({"op": "navigate", "url": "file:///etc"})).unwrap_err();
        assert!(e.contains("leave \"op\" out"), "{e}");
        assert!(route("click", &json!(["#buy"])).is_err());
    }

    #[test]
    fn start_and_stop_are_the_recording_ops_and_the_path_is_the_servers() {
        assert_eq!(
            route("start", &json!({})).unwrap(),
            Action::Start { out: None }
        );
        assert_eq!(
            route("start", &json!({"out": "demo.mp4"})).unwrap(),
            Action::Start {
                out: Some("demo.mp4".into())
            }
        );
        assert!(route("start", &json!({"path": "/etc/passwd"})).is_err());
        assert!(route("start", &json!({"out": "/etc/passwd"})).is_err());
        assert_eq!(route("stop", &Value::Null).unwrap(), Action::Stop);
    }

    #[test]
    fn open_turns_its_fields_into_the_same_flags_record_takes() {
        let a = route(
            "open",
            &json!({"url": "http://127.0.0.1:8099/", "preset": "tiktok", "frame": "ios",
                    "slowmo": 4, "audio": true}),
        )
        .unwrap();
        let Action::Open { argv, url } = a else {
            panic!("not an open")
        };
        assert_eq!(url.as_deref(), Some("http://127.0.0.1:8099/"));
        let pairs: Vec<&str> = argv.iter().map(String::as_str).collect();
        for want in [
            ["--preset", "tiktok"],
            ["--frame", "ios"],
            ["--slowmo", "4"],
        ] {
            assert!(pairs.windows(2).any(|w| w == want), "{want:?} in {pairs:?}");
        }
        assert!(pairs.contains(&"--audio"));
        // And the real parser accepts them.
        let parsed = args_from(&[], argv).unwrap();
        assert_eq!((parsed.width, parsed.height), (432, 768));
        assert!(parsed.frame.is_some());
        let e = route("open", &json!({"chromium": "/tmp/evil"})).unwrap_err();
        assert!(e.contains("no field \"chromium\""), "{e}");
        assert!(args_from(&[], vec!["--preset".into(), "nope".into()]).is_err());
        // The server's own flags are the base; a tool's flags win over them.
        let base = vec!["--preset".to_string(), "square".to_string()];
        assert_eq!(args_from(&base, vec![]).unwrap().width, 540);
        let a = args_from(&base, vec!["--width".into(), "800".into()]).unwrap();
        assert_eq!((a.width, a.height), (800, 540));
    }

    #[test]
    fn record_demo_checks_the_script_before_anything_runs() {
        let ok = "# a comment\n{\"op\":\"navigate\",\"url\":\"examples/demo.html\"}\n\n\
                  {\"op\":\"wait\",\"ms\":1600}\n";
        let a = route(
            "record_demo",
            &json!({"script_jsonl": ok, "preset": "square"}),
        )
        .unwrap();
        let Action::RecordDemo { argv, out, .. } = a else {
            panic!("not a record_demo")
        };
        assert_eq!(argv, vec!["--preset", "square"]);
        assert_eq!(out, None);
        assert_eq!(check_script(ok), Ok(2));
        let e = route(
            "record_demo",
            &json!({"script_jsonl": "{\"op\":\"click\"\n"}),
        )
        .unwrap_err();
        assert!(e.contains("script line 1"), "{e}");
        let e = check_script("{\"selector\":\"#a\"}").unwrap_err();
        assert!(e.contains("\"op\""), "{e}");
        assert!(check_script("# nothing\n\n").is_err());
        assert!(route("record_demo", &json!({})).is_err());
        assert!(route("record_demo", &json!({"script_jsonl": ok, "port": 9})).is_err());
    }

    #[test]
    fn record_demo_takes_the_soundtrack_as_the_same_flags_record_does() {
        let a = route(
            "record_demo",
            &json!({"script_jsonl": "{\"op\":\"say\",\"text\":\"hi\"}",
                    "tts": "none", "music": "bed.mp3", "music_volume": -20,
                    "duck": false, "captions": "burn", "loudness": "off"}),
        )
        .unwrap();
        let Action::RecordDemo { argv, .. } = a else {
            panic!("not a record_demo")
        };
        let pairs: Vec<&str> = argv.iter().map(String::as_str).collect();
        for want in [
            ["--tts", "none"],
            ["--music", "bed.mp3"],
            ["--music-volume", "-20"],
            ["--duck", "off"],
            ["--captions", "burn"],
            ["--loudness", "off"],
        ] {
            assert!(pairs.windows(2).any(|w| w == want), "{want:?} in {pairs:?}");
        }
        let parsed = args_from(&[], argv).unwrap();
        assert_eq!(parsed.sound.tts, "none");
        assert!(!parsed.sound.duck);
        assert_eq!(parsed.sound.loudness, None);
        assert_eq!(parsed.sound.captions, crate::captions::Mode::Burn);
        let a = route("say", &json!({"text": "Watch this", "wait": true})).unwrap();
        assert_eq!(
            a,
            Action::Op(json!({"op": "say", "text": "Watch this", "wait": true}))
        );
    }

    #[test]
    fn an_unknown_tool_is_named() {
        assert_eq!(
            route("teleport", &json!({})).unwrap_err(),
            "unknown tool: teleport"
        );
    }

    #[test]
    fn every_listed_tool_routes_and_every_routed_tool_is_listed() {
        let listed: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        for name in &listed {
            let args = match name.as_str() {
                "record_demo" => json!({"script_jsonl": "{\"op\":\"wait\",\"ms\":1}"}),
                _ => json!({}),
            };
            assert!(
                route(name, &args).is_ok(),
                "{name} is listed but does not route"
            );
        }
        for (tool, _) in OP_TOOLS {
            assert!(
                listed.iter().any(|n| n == tool),
                "{tool} routes but is not listed"
            );
        }
        for t in tools() {
            assert_eq!(t.input_schema["type"], json!("object"), "{}", t.name);
            assert!(t
                .description
                .as_deref()
                .is_some_and(|d| !d.contains('\u{2014}')));
        }
    }

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn a_take_collects_its_marks_and_render_from_record_output() {
        let mut t = Takes::default();
        let id = t.create("record_demo", Path::new("/takes"), None);
        assert_eq!(
            t.get(&id).unwrap().path,
            Path::new("/takes").join(format!("{id}.mp4"))
        );
        assert_eq!(t.view(&id).unwrap()["status"], "queued");
        t.set_running(&id);
        // Marks before start_recording are not in the video and are not kept.
        t.absorb(
            &id,
            &line(
                json!({"ok": true, "result": {"event": "mark", "kind": "navigate",
                                    "t": 0.0, "label": "x", "recording": false}}),
            ),
        );
        t.absorb(
            &id,
            &line(json!({"ok": true, "result": {"event": "recording_started"}})),
        );
        t.absorb(
            &id,
            &line(
                json!({"ok": true, "result": {"event": "mark", "kind": "click",
                                    "t": 1.5, "label": "#go", "box": [1, 2, 3, 4],
                                    "recording": true}}),
            ),
        );
        t.absorb(&id, "kaviri: not json at all");
        t.absorb(
            &id,
            &line(json!({"ok": true, "result": {"event": "recording_rendered",
                                    "path": "x.mp4", "duration": 6.2, "zoom_events": 1,
                                    "frames": 120}})),
        );
        t.finish(&id, true, None);
        let v = t.view(&id).unwrap();
        assert_eq!(v["status"], "done");
        assert_eq!(v["marks"].as_array().unwrap().len(), 1);
        assert_eq!(v["marks"][0]["label"], "#go");
        assert_eq!(v["duration"], 6.2);
        assert_eq!(v["zoom_events"], 1);
        assert!(v.get("error").is_none());
        assert_eq!(t.latest_done().unwrap().id, id);
    }

    #[test]
    fn a_failed_op_fails_the_take_and_says_which() {
        let mut t = Takes::default();
        let id = t.create("record_demo", Path::new("/takes"), None);
        t.absorb(
            &id,
            &line(json!({"ok": false, "error": "selector not found: #go",
                                    "op": "click", "index": 4})),
        );
        t.absorb(
            &id,
            &line(json!({"ok": true, "result": {"event": "recording_rendered",
                                    "duration": 2.0, "zoom_events": 0}})),
        );
        t.finish(&id, false, None);
        let v = t.view(&id).unwrap();
        assert_eq!(v["status"], "failed");
        assert_eq!(v["error"], "op 4 (click): selector not found: #go");
        assert_eq!(v["video_exists"], false);
        assert!(t.latest_done().is_none());

        // A process that dies without a word is failed with its last log line.
        let id2 = t.create("record_demo", Path::new("/takes"), None);
        for i in 0..(LOG_TAIL + 5) {
            t.log(&id2, format!("line {i}"));
        }
        t.finish(&id2, false, None);
        let v = t.view(&id2).unwrap();
        assert_eq!(v["log"].as_array().unwrap().len(), LOG_TAIL);
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains(&format!("line {}", LOG_TAIL + 4)));
        // Newest first.
        assert_eq!(t.list()[0]["id"], json!(id2));
    }

    #[test]
    fn a_session_take_is_closed_by_what_stop_returned() {
        let mut t = Takes::default();
        let id = t.create(
            "session",
            Path::new("/takes"),
            Some(PathBuf::from("/x/demo.mp4")),
        );
        t.set_running(&id);
        t.finish_session(
            &id,
            &Ok(
                json!({"event": "recording_rendered", "duration": 3.0, "zoom_events": 2,
                       "marks": [{"t": 1.0, "kind": "click", "label": "#a", "box": null}]}),
            ),
        );
        let v = t.view(&id).unwrap();
        assert_eq!(v["status"], "done");
        assert_eq!(v["path"], "/x/demo.mp4");
        assert_eq!(v["marks"][0]["kind"], "click");
        let id2 = t.create("session", Path::new("/takes"), None);
        t.finish_session(&id2, &Err("no frames were captured".into()));
        assert_eq!(t.view(&id2).unwrap()["status"], "failed");
        assert!(t.view("take-nope").is_none());
    }

    fn marks(v: &[(f64, &str)]) -> Vec<(f64, String)> {
        v.iter().map(|(t, k)| (*t, k.to_string())).collect()
    }

    #[test]
    fn keyframes_land_after_each_interaction_and_skip_the_waits() {
        let m = marks(&[(1.0, "click"), (2.0, "wait"), (3.0, "type"), (3.1, "press")]);
        let t = keyframe_times(&m, 10.0, 2);
        assert_eq!(t, vec![1.7, 3.7]);
        // Never past the end of the video.
        let t = keyframe_times(&marks(&[(9.9, "click")]), 10.0, 1);
        assert_eq!(t, vec![9.95]);
    }

    #[test]
    fn a_sparse_take_is_topped_up_and_a_busy_one_thinned_evenly() {
        let t = keyframe_times(&marks(&[(1.0, "click")]), 12.0, 9);
        assert_eq!(t.len(), 6);
        assert!(t.windows(2).all(|w| w[0] < w[1]));
        assert!(t.contains(&1.7));
        let busy: Vec<(f64, String)> = (0..30).map(|i| (i as f64, "click".to_string())).collect();
        let t = keyframe_times(&busy, 40.0, 9);
        assert_eq!(t.len(), 9);
        assert_eq!(t[0], 0.7);
        assert_eq!(t[8], 29.7);
    }

    #[test]
    fn the_poster_is_the_last_interaction_landed() {
        let m = marks(&[(1.0, "click"), (4.0, "type"), (5.0, "wait")]);
        assert_eq!(poster_time(&m, 10.0), 5.0);
        assert_eq!(poster_time(&marks(&[(2.0, "wait")]), 10.0), 5.0);
    }

    #[test]
    fn the_sheet_grid_holds_every_tile() {
        for n in 1..=16 {
            let (c, r) = grid(n);
            assert!(c * r >= n, "{n} in {c}x{r}");
            assert!(c * (r - 1) < n, "{n} in {c}x{r} leaves an empty row");
        }
    }
}
