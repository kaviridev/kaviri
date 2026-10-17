//! Text to speech for the `say` op: one trait, several engines, one cache.
//!
//! The default engine is Piper, run as a separate process the way ffmpeg and Chromium are.
//! kaviri links no speech code and ships no voice: `kaviri doctor --fix` downloads the Piper
//! release and a voice into `~/.cache/kaviri`, checked against the SHA-256 pinned here. The
//! Piper release carries espeak-ng, which is GPL-3.0, so it stays exactly where ffmpeg does:
//! the user's own software, fetched only when asked, never in kaviri's archive.
//!
//! Every voice in `VOICES` was checked for its licence before it was listed. Each is trained
//! on public-domain recordings and none descends from `lessac`, so nothing about it restricts a
//! commercial take.
//! Voices fine-tuned from Piper's `lessac` checkpoint are left out on purpose: the Blizzard
//! 2013 data under that one is not licensed for commercial use. THIRD-PARTY.md has the table.
//!
//! Cloud engines (OpenAI, ElevenLabs, Cloudflare Workers AI) are there for people who already
//! pay for one. They are never chosen unless named, and read the key from the environment.
//! `none` synthesises nothing: it writes silence as long as the line would take to say, so a
//! script's timing, captions and mix can be tested on a CI runner with no engine at all.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

/// Every clip kaviri mixes is this shape: mono, 16-bit, 48 kHz.
pub const RATE: u32 = 48_000;

/// Bumped when the cached shape of a line changes, so an old cache is not trusted.
const CACHE_VERSION: &str = "1";

/// One word and when it is said, in seconds from the start of its clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// A line, synthesised and ready to place.
#[derive(Clone, Debug)]
pub struct Speech {
    /// Mono 16-bit `RATE` WAV.
    pub wav: PathBuf,
    pub secs: f64,
    pub words: Vec<Word>,
    /// Whether `words` came from the engine, rather than being spread evenly.
    pub timed: bool,
}

/// What an engine hands back: audio in any format ffmpeg reads, and word timings if it
/// knows them.
pub struct Raw {
    pub audio: PathBuf,
    pub words: Option<Vec<Word>>,
}

pub trait Engine: Send + Sync {
    fn name(&self) -> &'static str;
    fn default_voice(&self) -> String;
    /// What the cache keys a line by, besides voice and text: the engine, and the model
    /// where one can be chosen, so a line made by another model is not handed back.
    fn cache_tag(&self) -> String {
        self.name().to_string()
    }
    /// Synthesise `text` into a file inside `dir`.
    fn synth(&self, text: &str, voice: &str, dir: &Path) -> Result<Raw, String>;
}

// ---------------------------------------------------------------------------------------
// The voices kaviri knows how to fetch
// ---------------------------------------------------------------------------------------

/// A Piper voice whose licence has been read.
pub struct Voice {
    pub name: &'static str,
    /// Path inside the piper-voices repository.
    pub dir: &'static str,
    pub onnx_sha256: &'static str,
    pub json_sha256: &'static str,
    pub about: &'static str,
}

/// The piper-voices revision the hashes below were taken from. Pinned, so a voice that is
/// retrained upstream does not quietly change what a take sounds like.
pub const VOICES_REV: &str = "c10ece1aade47bb51c153c893d14e5bf8e5b7117";

pub const DEFAULT_VOICE: &str = "en_US-ljspeech-medium";

pub const VOICES: &[Voice] = &[
    Voice {
        name: "en_US-ljspeech-medium",
        dir: "en/en_US/ljspeech/medium",
        onnx_sha256: "6f52a751e2349abe7a76735eb09dc1875298c77ea2342ffd2fef79ff81b87f22",
        json_sha256: "141d612cc0a95ed7efc1ca936b845c2364967f2e9217c5dbfcf69fc4d6c65860",
        about: "US English, female; LJ Speech, public domain (the default)",
    },
    Voice {
        name: "en_US-kristin-medium",
        dir: "en/en_US/kristin/medium",
        onnx_sha256: "5849957f929cbf720c258f8458692d6103fff2f0e3d3b19c8259474bb06a18d4",
        json_sha256: "5681426d4aead22195de70531eeeeddb46493cfaffc5764b2ea3db73428b651c",
        about: "US English, female; LibriVox, public domain",
    },
    Voice {
        name: "en_US-norman-medium",
        dir: "en/en_US/norman/medium",
        onnx_sha256: "b9739443232a80a59c7d18810dd856899bf16a7964725f5ab81ea49b1351cb71",
        json_sha256: "6c2db7f558a4a8deb9fe822583c1c5105f6c4e834dd0f9de8ad17a888ee9fe1d",
        about: "US English, male; LibriVox, public domain",
    },
    Voice {
        name: "en_US-john-medium",
        dir: "en/en_US/john/medium",
        onnx_sha256: "789c6c875726e627ddee93d51d8727859abe9c091c3d141591f4b83c2072e988",
        json_sha256: "af60f177b6b550f3d7a302720c0fb89e7f94a82b5dca464775ef63b1c69ba09a",
        about: "US English, male; LibriVox, public domain",
    },
    Voice {
        name: "en_GB-cori-medium",
        dir: "en/en_GB/cori/medium",
        onnx_sha256: "1899f98e5fb8310154f3c2973f4b8a929ba7245e722b3d3a85680b833d95f10d",
        json_sha256: "e262c16d7f192f69d4edd6b4ef8a5915379e67495fcc402f1ab15eeb33da3d36",
        about: "British English, female; LibriVox, public domain",
    },
];

pub fn voice(name: &str) -> Option<&'static Voice> {
    VOICES.iter().find(|v| v.name == name)
}

/// The Piper release `doctor --fix` installs, per platform: (asset, sha256).
pub const PIPER_REL: &str = "2023.11.14-2";

pub fn piper_asset() -> Option<(&'static str, &'static str)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    Some(match (os, arch) {
        ("linux", "x86_64") => (
            "piper_linux_x86_64.tar.gz",
            "a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992",
        ),
        ("linux", "aarch64") => (
            "piper_linux_aarch64.tar.gz",
            "fea0fd2d87c54dbc7078d0f878289f404bd4d6eea6e7444a77835d1537ab88eb",
        ),
        ("macos", "aarch64") => (
            "piper_macos_aarch64.tar.gz",
            "6b1eb03b3735946cb35216e063e7eebcc33a6bbf5dd96ec0217959bf1cdcb0cc",
        ),
        ("macos", "x86_64") => (
            "piper_macos_x64.tar.gz",
            "ced85c0a3df13945b1e623b878a48fdc2854d5c485b4b67f62857cf551deaf8b",
        ),
        _ => return None,
    })
}

pub fn help() -> String {
    let mut s = String::from(
        "Engines (--tts):\n  \
         piper       local and free; kaviri doctor --fix installs it and the default voice\n  \
         none        silence as long as each line would take to say; for tests and CI\n  \
         openai      your key in OPENAI_API_KEY; voices alloy, nova, onyx, ...\n  \
         elevenlabs  your key in ELEVENLABS_API_KEY; voice is a voice id; word timings\n  \
         cloudflare  CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN; Workers AI MeloTTS\n\n\
         Piper voices kaviri can fetch (--voice, or \"voice\" on a say op):\n",
    );
    for v in VOICES {
        s.push_str(&format!("  {:<24} {}\n", v.name, v.about));
    }
    s.push_str("  or a path to any Piper .onnx voice, whose licence is then yours to check");
    s
}

// ---------------------------------------------------------------------------------------
// Where things live
// ---------------------------------------------------------------------------------------

/// `$KAVIRI_CACHE`, else `$XDG_CACHE_HOME/kaviri`, else `~/.cache/kaviri`.
pub fn cache_root() -> PathBuf {
    if let Some(p) = crate::env::var_os("CACHE") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("kaviri");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cache").join("kaviri")
}

pub fn voices_dir() -> PathBuf {
    cache_root().join("voices")
}

/// The Piper binary: `$KAVIRI_PIPER`, the one `doctor --fix` installed, or `piper` on PATH.
pub fn find_piper() -> Option<PathBuf> {
    if let Some(p) = crate::env::var_os("PIPER") {
        return Some(PathBuf::from(p));
    }
    let ours = cache_root().join("piper").join("piper");
    if ours.is_file() {
        return Some(ours);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("piper"))
        .find(|p| p.is_file())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(p: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Fetch `url` to `dest` with curl, and refuse it unless it hashes to `sha256`.
pub fn download(url: &str, dest: &Path, sha256: &str) -> Result<(), String> {
    if dest.is_file() && sha256_file(dest)? == sha256 {
        return Ok(());
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let part = dest.with_extension("part");
    eprintln!("kaviri: fetching {url}");
    let out = Command::new("curl")
        .args(["-fL", "--retry", "2", "--silent", "--show-error", "-o"])
        .arg(&part)
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl is needed to download, and did not run: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "download failed: {url}\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let got = sha256_file(&part)?;
    if got != sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{url} did not match its pinned checksum (got {got}, want {sha256}); not installed"
        ));
    }
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Install Piper and the named voices into the cache. What `kaviri doctor --fix` runs.
pub fn install(voices: &[&str]) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let root = cache_root();
    if find_piper().is_none() {
        let (asset, sha) = piper_asset().ok_or_else(|| {
            format!(
                "there is no Piper release for {} {}; install piper yourself and set KAVIRI_PIPER",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
        let tgz = root.join("downloads").join(asset);
        download(
            &format!("https://github.com/rhasspy/piper/releases/download/{PIPER_REL}/{asset}"),
            &tgz,
            sha,
        )?;
        let out = Command::new("tar")
            .arg("xzf")
            .arg(&tgz)
            .arg("-C")
            .arg(&root)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("tar: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "could not unpack {}: {}",
                tgz.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let _ = std::fs::remove_file(&tgz);
        done.push(format!(
            "piper {PIPER_REL} in {} (MIT, with espeak-ng under GPL-3.0)",
            root.join("piper").display()
        ));
    }
    for name in voices {
        let v = voice(name)
            .ok_or_else(|| format!("kaviri does not know a voice called {name}\n\n{}", help()))?;
        let base = format!(
            "https://huggingface.co/rhasspy/piper-voices/resolve/{VOICES_REV}/{}/{}",
            v.dir, v.name
        );
        let dir = voices_dir();
        download(
            &format!("{base}.onnx"),
            &dir.join(format!("{}.onnx", v.name)),
            v.onnx_sha256,
        )?;
        download(
            &format!("{base}.onnx.json"),
            &dir.join(format!("{}.onnx.json", v.name)),
            v.json_sha256,
        )?;
        done.push(format!("voice {} ({})", v.name, v.about));
    }
    Ok(done)
}

// ---------------------------------------------------------------------------------------
// Choosing an engine
// ---------------------------------------------------------------------------------------

/// The engine `--tts` names. `env` is how keys are read, so the choice is testable without
/// touching the real environment.
pub fn select(name: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Arc<dyn Engine>, String> {
    let key = |var: &str, who: &str| -> Result<String, String> {
        env(var).filter(|k| !k.trim().is_empty()).ok_or_else(|| {
            format!("--tts {who} needs your key in {var}; nothing is sent without one")
        })
    };
    Ok(match name {
        "piper" => Arc::new(Piper),
        "none" => Arc::new(Silent),
        "openai" => Arc::new(OpenAi {
            key: key("OPENAI_API_KEY", "openai")?,
            model: env("KAVIRI_OPENAI_TTS_MODEL").unwrap_or_else(|| "gpt-4o-mini-tts".into()),
        }),
        "elevenlabs" => Arc::new(ElevenLabs {
            key: key("ELEVENLABS_API_KEY", "elevenlabs")?,
            model: env("KAVIRI_ELEVENLABS_MODEL")
                .unwrap_or_else(|| "eleven_multilingual_v2".into()),
        }),
        "cloudflare" => Arc::new(Cloudflare {
            account: key("CLOUDFLARE_ACCOUNT_ID", "cloudflare")?,
            token: key("CLOUDFLARE_API_TOKEN", "cloudflare")?,
        }),
        other => {
            return Err(format!(
                "--tts must be piper, none, openai, elevenlabs or cloudflare, got {other}"
            ))
        }
    })
}

pub fn env_lookup(var: &str) -> Option<String> {
    std::env::var(var).ok()
}

// ---------------------------------------------------------------------------------------
// Text into sentences and words
// ---------------------------------------------------------------------------------------

/// Split on sentence ends, so an engine that knows nothing of words still tells us where
/// each sentence starts. A full stop inside a number or a domain (`3.5`, `kaviri.dev`) is
/// not an end: an end is punctuation followed by whitespace.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
            continue;
        }
        cur.push(*c);
        let end = matches!(c, '.' | '!' | '?' | ';')
            && chars.get(i + 1).map_or(true, |n| n.is_whitespace());
        if end && !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Spread the words of `text` over `start..end`, each taking time in proportion to its
/// length, with a breath after a comma or a full stop. Used when the engine gives no timings.
pub fn spread_words(text: &str, start: f64, end: f64) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || end <= start {
        return Vec::new();
    }
    let weight = |w: &str| -> (f64, f64) {
        let len = w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64 + 1.5;
        let pause = match w.chars().last() {
            Some(',' | ';' | ':') => 2.5,
            Some('.' | '!' | '?') => 4.0,
            _ => 0.0,
        };
        (len, pause)
    };
    let total: f64 = words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let (l, p) = weight(w);
            if i + 1 == words.len() {
                l
            } else {
                l + p
            }
        })
        .sum();
    let unit = (end - start) / total;
    let mut t = start;
    let mut out = Vec::with_capacity(words.len());
    for w in &words {
        let (l, p) = weight(w);
        out.push(Word {
            text: w.to_string(),
            start: t,
            end: t + l * unit,
        });
        t += (l + p) * unit;
    }
    out
}

/// How long a line takes to say, for an engine that says nothing: 2.6 words a second,
/// which is a brisk narrator, plus a pause per sentence.
pub fn estimate_secs(text: &str) -> f64 {
    let words = text.split_whitespace().count() as f64;
    let breaks = sentences(text).len().max(1) as f64;
    (words / 2.6 + 0.25 * breaks).max(0.6)
}

// ---------------------------------------------------------------------------------------
// WAV in and out
// ---------------------------------------------------------------------------------------

/// Mono samples and their rate, from a 16-bit PCM WAV. Stereo is folded to mono.
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_wav(&b).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse_wav(b: &[u8]) -> Result<(Vec<f32>, u32), String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut i = 12;
    let (mut rate, mut chans, mut bits) = (0u32, 0u16, 0u16);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            chans = u16::from_le_bytes([b[body + 2], b[body + 3]]);
            rate = u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
            bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
        } else if id == b"data" {
            if bits != 16 || chans == 0 || rate == 0 {
                return Err(format!(
                    "only 16-bit PCM is read here, got {bits}-bit, {chans} channel(s)"
                ));
            }
            // A streamed WAV says 0 or 0xFFFFFFFF here and means "to the end of the file".
            let end = if len == 0 || body + len > b.len() {
                b.len()
            } else {
                body + len
            };
            let ch = chans as usize;
            let s: Vec<f32> = b[body..end]
                .chunks_exact(2 * ch)
                .map(|f| {
                    (0..ch)
                        .map(|c| i16::from_le_bytes([f[2 * c], f[2 * c + 1]]) as f32 / 32768.0)
                        .sum::<f32>()
                        / ch as f32
                })
                .collect();
            return Ok((s, rate));
        }
        i = body + len + (len & 1);
    }
    Err("WAV file has no data chunk".into())
}

/// A mono 16-bit WAV.
pub fn wav_bytes(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// Any audio file ffmpeg reads, into the one shape kaviri mixes.
pub fn to_clip_wav(input: &Path, out: &Path) -> Result<(), String> {
    let ffmpeg = crate::zoom::find_ffmpeg()?;
    let o = Command::new(&ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(crate::zoom::arg_path(&input.display().to_string()))
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            &RATE.to_string(),
            "-c:a",
            "pcm_s16le",
        ])
        .arg(crate::zoom::arg_path(&out.display().to_string()))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "ffmpeg could not read {}: {}",
            input.display(),
            crate::zoom::stderr_tail(&o.stderr)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The cache, and the one entry point
// ---------------------------------------------------------------------------------------

/// Synthesise a line, or find it already synthesised. The second take of a script costs
/// nothing: lines are kept by engine (and model), voice and text in `~/.cache/kaviri/tts`.
pub fn speak(engine: &dyn Engine, text: &str, voice: &str) -> Result<Speech, String> {
    let key =
        sha256_hex(format!("{CACHE_VERSION}\n{}\n{voice}\n{text}", engine.cache_tag()).as_bytes());
    let dir = cache_root().join("tts");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wav = dir.join(format!("{}.wav", &key[..32]));
    let meta = dir.join(format!("{}.json", &key[..32]));
    if let (Ok(m), true) = (std::fs::read_to_string(&meta), wav.is_file()) {
        if let Some(s) = speech_from_meta(&m, &wav) {
            return Ok(s);
        }
    }
    let work = dir.join(format!("work-{}-{}", std::process::id(), &key[..12]));
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let made = (|| {
        let raw = engine.synth(text, voice, &work)?;
        to_clip_wav(&raw.audio, &wav)?;
        let (s, rate) = read_wav(&wav)?;
        let secs = s.len() as f64 / rate as f64;
        let (words, timed) = match raw.words {
            Some(w) if !w.is_empty() => (w, true),
            _ => (spread_words(text, 0.0, secs), false),
        };
        Ok::<_, String>(Speech {
            wav: wav.clone(),
            secs,
            words,
            timed,
        })
    })();
    let _ = std::fs::remove_dir_all(&work);
    let s = made?;
    let m = json!({
        "secs": s.secs, "timed": s.timed,
        "words": s.words.iter().map(|w| json!([w.text, w.start, w.end])).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&meta, m.to_string());
    Ok(s)
}

/// A recorded clip, for the `voice` op: decoded into the cache once per file version, with
/// its words spread over it when the op says what it says.
pub fn clip_from_file(path: &Path, text: Option<&str>) -> Result<Speech, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("voice: {}: {e}", path.display()))?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = sha256_hex(
        format!(
            "{CACHE_VERSION}\nfile\n{}\n{}\n{stamp}",
            abs.display(),
            meta.len()
        )
        .as_bytes(),
    );
    let dir = cache_root().join("clips");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wav = dir.join(format!("{}.wav", &key[..32]));
    if !wav.is_file() {
        to_clip_wav(path, &wav)?;
    }
    let (s, rate) = read_wav(&wav)?;
    let secs = s.len() as f64 / rate as f64;
    Ok(Speech {
        wav,
        secs,
        words: text.map(|t| spread_words(t, 0.0, secs)).unwrap_or_default(),
        timed: false,
    })
}

fn speech_from_meta(m: &str, wav: &Path) -> Option<Speech> {
    let v: Value = serde_json::from_str(m).ok()?;
    let words = v["words"]
        .as_array()?
        .iter()
        .filter_map(|w| {
            Some(Word {
                text: w[0].as_str()?.to_string(),
                start: w[1].as_f64()?,
                end: w[2].as_f64()?,
            })
        })
        .collect();
    Some(Speech {
        wav: wav.to_path_buf(),
        secs: v["secs"].as_f64()?,
        words,
        timed: v["timed"].as_bool().unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------------------
// Engines
// ---------------------------------------------------------------------------------------

/// Writes silence as long as the line would take to say, and words spread over it.
pub struct Silent;

impl Engine for Silent {
    fn name(&self) -> &'static str {
        "none"
    }
    fn default_voice(&self) -> String {
        "none".into()
    }
    fn synth(&self, text: &str, _voice: &str, dir: &Path) -> Result<Raw, String> {
        let secs = estimate_secs(text);
        let out = dir.join("line.wav");
        let n = (secs * RATE as f64).round() as usize;
        std::fs::write(&out, wav_bytes(&vec![0.0; n], RATE))
            .map_err(|e| format!("{}: {e}", out.display()))?;
        // Sentence by sentence, with the same pauses the estimate allowed for.
        let mut words = Vec::new();
        let mut t = 0.0;
        for s in sentences(text) {
            let d = s.split_whitespace().count() as f64 / 2.6;
            words.extend(spread_words(&s, t, t + d));
            t += d + 0.25;
        }
        Ok(Raw {
            audio: out,
            words: Some(words),
        })
    }
}

pub struct Piper;

/// Seconds of silence Piper leaves after each sentence.
const PIPER_GAP: f64 = 0.25;

impl Piper {
    fn model(voice: &str) -> Result<PathBuf, String> {
        let p = Path::new(voice);
        if voice.ends_with(".onnx") {
            return if p.is_file() {
                Ok(p.to_path_buf())
            } else {
                Err(format!("no Piper voice at {voice}"))
            };
        }
        let installed = voices_dir().join(format!("{voice}.onnx"));
        if installed.is_file() {
            return Ok(installed);
        }
        Err(match self::voice(voice) {
            Some(_) => format!(
                "the voice {voice} is not installed; run kaviri doctor --fix --voice {voice}"
            ),
            None => format!("kaviri does not know a voice called {voice}\n\n{}", help()),
        })
    }
}

impl Engine for Piper {
    fn name(&self) -> &'static str {
        "piper"
    }
    fn default_voice(&self) -> String {
        DEFAULT_VOICE.into()
    }
    /// One process for the whole line, one WAV per sentence, so each sentence's start is
    /// known exactly and only the words inside it are spread.
    fn synth(&self, text: &str, voice: &str, dir: &Path) -> Result<Raw, String> {
        let bin = find_piper().ok_or(
            "Piper is not installed; run kaviri doctor --fix, set KAVIRI_PIPER, \
             or record with --tts none",
        )?;
        let model = Self::model(voice)?;
        let parts = sentences(text);
        let mut input = String::new();
        let files: Vec<PathBuf> = (0..parts.len())
            .map(|i| dir.join(format!("s{i:03}.wav")))
            .collect();
        for (s, f) in parts.iter().zip(&files) {
            input.push_str(&json!({"text": s, "output_file": f.display().to_string()}).to_string());
            input.push('\n');
        }
        let mut child = Command::new(&bin)
            .arg("--quiet")
            .arg("--model")
            .arg(&model)
            .args(["--json-input", "--sentence_silence", &PIPER_GAP.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run piper at {}: {e}", bin.display()))?;
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().ok_or("piper stdin")?;
            stdin
                .write_all(input.as_bytes())
                .map_err(|e| format!("piper: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("piper: {e}"))?;
        if !out.status.success() || files.iter().any(|f| !f.is_file()) {
            return Err(format!(
                "piper failed ({}): {}",
                out.status,
                crate::zoom::stderr_tail(&out.stderr)
            ));
        }
        let mut all: Vec<f32> = Vec::new();
        let mut rate = 0;
        let mut words = Vec::new();
        for (s, f) in parts.iter().zip(&files) {
            let (pcm, r) = read_wav(f)?;
            if rate != 0 && r != rate {
                return Err("piper changed sample rate between sentences".into());
            }
            rate = r;
            let t0 = all.len() as f64 / r as f64;
            let d = pcm.len() as f64 / r as f64;
            words.extend(spread_words(s, t0 + 0.05, t0 + (d - PIPER_GAP).max(0.1)));
            all.extend(pcm);
        }
        let line = dir.join("line.wav");
        std::fs::write(&line, wav_bytes(&all, rate.max(1)))
            .map_err(|e| format!("{}: {e}", line.display()))?;
        Ok(Raw {
            audio: line,
            words: Some(words),
        })
    }
}

/// POST `body` to `url` with curl. Secret headers go in on stdin as curl config, so a key
/// never appears in the process list.
fn post(url: &str, headers: &[(&str, String)], body: &[u8], dir: &Path) -> Result<Vec<u8>, String> {
    let req = dir.join("request.json");
    let resp = dir.join("response.bin");
    std::fs::write(&req, body).map_err(|e| format!("{}: {e}", req.display()))?;
    let mut config = String::new();
    for (k, v) in headers {
        let line = format!("{k}: {v}");
        config.push_str(&format!("header = \"{}\"\n", curl_quote(&line)));
    }
    let mut child = Command::new("curl")
        .args(["-sS", "-X", "POST", "-w", "%{http_code}", "-o"])
        .arg(&resp)
        .arg("--data-binary")
        .arg(format!("@{}", req.display()))
        .args(["--config", "-"])
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl is needed for a cloud voice, and did not run: {e}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().ok_or("curl stdin")?;
        stdin
            .write_all(config.as_bytes())
            .map_err(|e| format!("curl: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let bytes = std::fs::read(&resp).unwrap_or_default();
    if !out.status.success() || !code.starts_with('2') {
        let msg = String::from_utf8_lossy(&bytes[..bytes.len().min(400)]).to_string();
        return Err(format!(
            "{url} answered {code}: {}{}",
            msg.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(bytes)
}

fn curl_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub struct OpenAi {
    key: String,
    model: String,
}

pub fn openai_body(model: &str, text: &str, voice: &str) -> Value {
    json!({"model": model, "input": text, "voice": voice, "response_format": "wav"})
}

impl Engine for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }
    fn cache_tag(&self) -> String {
        format!("{}/{}", self.name(), self.model)
    }
    fn default_voice(&self) -> String {
        "alloy".into()
    }
    fn synth(&self, text: &str, voice: &str, dir: &Path) -> Result<Raw, String> {
        let body = openai_body(&self.model, text, voice).to_string();
        let audio = post(
            "https://api.openai.com/v1/audio/speech",
            &[
                ("Authorization", format!("Bearer {}", self.key)),
                ("Content-Type", "application/json".into()),
            ],
            body.as_bytes(),
            dir,
        )?;
        let out = dir.join("line.wav");
        std::fs::write(&out, audio).map_err(|e| format!("{}: {e}", out.display()))?;
        Ok(Raw {
            audio: out,
            words: None,
        })
    }
}

pub struct ElevenLabs {
    key: String,
    model: String,
}

/// ElevenLabs times every character. Words are the runs between spaces.
pub fn words_from_chars(chars: &[String], starts: &[f64], ends: &[f64]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut cur: Option<Word> = None;
    for i in 0..chars.len().min(starts.len()).min(ends.len()) {
        let c = &chars[i];
        if c.trim().is_empty() {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            continue;
        }
        match cur.as_mut() {
            Some(w) => {
                w.text.push_str(c);
                w.end = ends[i];
            }
            None => {
                cur = Some(Word {
                    text: c.clone(),
                    start: starts[i],
                    end: ends[i],
                })
            }
        }
    }
    out.extend(cur);
    out
}

impl Engine for ElevenLabs {
    fn name(&self) -> &'static str {
        "elevenlabs"
    }
    fn cache_tag(&self) -> String {
        format!("{}/{}", self.name(), self.model)
    }
    fn default_voice(&self) -> String {
        // "Rachel", one of the premade voices every account has.
        "21m00Tcm4TlvDq8ikWAM".into()
    }
    fn synth(&self, text: &str, voice: &str, dir: &Path) -> Result<Raw, String> {
        if voice.is_empty() || !voice.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("an ElevenLabs voice is a voice id, got {voice:?}"));
        }
        let body = json!({"text": text, "model_id": self.model}).to_string();
        let resp = post(
            &format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{voice}/with-timestamps?output_format=pcm_44100"
            ),
            &[
                ("xi-api-key", self.key.clone()),
                ("Content-Type", "application/json".into()),
            ],
            body.as_bytes(),
            dir,
        )?;
        let v: Value = serde_json::from_slice(&resp)
            .map_err(|e| format!("ElevenLabs answered something that is not JSON: {e}"))?;
        use base64::Engine as _;
        let pcm = base64::engine::general_purpose::STANDARD
            .decode(v["audio_base64"].as_str().unwrap_or(""))
            .map_err(|e| format!("ElevenLabs audio: {e}"))?;
        let samples: Vec<f32> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        let out = dir.join("line.wav");
        std::fs::write(&out, wav_bytes(&samples, 44_100))
            .map_err(|e| format!("{}: {e}", out.display()))?;
        let a = &v["alignment"];
        let strs = |k: &str| -> Vec<String> {
            a[k].as_array()
                .map(|x| {
                    x.iter()
                        .filter_map(|c| c.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let nums = |k: &str| -> Vec<f64> {
            a[k].as_array()
                .map(|x| x.iter().filter_map(|c| c.as_f64()).collect())
                .unwrap_or_default()
        };
        let words = words_from_chars(
            &strs("characters"),
            &nums("character_start_times_seconds"),
            &nums("character_end_times_seconds"),
        );
        Ok(Raw {
            audio: out,
            words: Some(words),
        })
    }
}

pub struct Cloudflare {
    account: String,
    token: String,
}

impl Engine for Cloudflare {
    fn name(&self) -> &'static str {
        "cloudflare"
    }
    fn default_voice(&self) -> String {
        "en".into()
    }
    /// MeloTTS on Workers AI. Its "voice" is a language code.
    fn synth(&self, text: &str, voice: &str, dir: &Path) -> Result<Raw, String> {
        if !self.account.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("CLOUDFLARE_ACCOUNT_ID is not an account id".into());
        }
        let body = json!({"prompt": text, "lang": voice}).to_string();
        let resp = post(
            &format!(
                "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/@cf/myshell-ai/melotts",
                self.account
            ),
            &[
                ("Authorization", format!("Bearer {}", self.token)),
                ("Content-Type", "application/json".into()),
            ],
            body.as_bytes(),
            dir,
        )?;
        // JSON with the audio as base64, or the audio itself, depending on the model.
        let audio = match serde_json::from_slice::<Value>(&resp) {
            Ok(v) => {
                let b = v["result"]["audio"]
                    .as_str()
                    .or_else(|| v["audio"].as_str())
                    .ok_or("Workers AI answered without audio")?;
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| format!("Workers AI audio: {e}"))?
            }
            Err(_) => resp,
        };
        let out = dir.join("line.mp3");
        std::fs::write(&out, audio).map_err(|e| format!("{}: {e}", out.display()))?;
        Ok(Raw {
            audio: out,
            words: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string())
    }

    #[test]
    fn the_engine_is_the_one_named_and_a_cloud_one_needs_its_key() {
        let none = env_of(&[]);
        assert_eq!(select("piper", &none).unwrap().name(), "piper");
        assert_eq!(select("none", &none).unwrap().name(), "none");
        let e = select("openai", &none).err().unwrap();
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
        let e = select("elevenlabs", &env_of(&[("ELEVENLABS_API_KEY", "  ")]))
            .err()
            .unwrap();
        assert!(e.contains("ELEVENLABS_API_KEY"), "{e}");
        let e = select("cloudflare", &env_of(&[("CLOUDFLARE_ACCOUNT_ID", "abc")]))
            .err()
            .unwrap();
        assert!(e.contains("CLOUDFLARE_API_TOKEN"), "{e}");
        let keys = env_of(&[
            ("OPENAI_API_KEY", "sk-x"),
            ("ELEVENLABS_API_KEY", "x"),
            ("CLOUDFLARE_ACCOUNT_ID", "abc"),
            ("CLOUDFLARE_API_TOKEN", "t"),
        ]);
        for n in ["openai", "elevenlabs", "cloudflare"] {
            assert_eq!(select(n, &keys).unwrap().name(), n);
        }
        assert!(select("espeak", &keys).is_err());
        // A line is cached per model, so changing the model does not hand back the old one.
        assert_eq!(
            select("openai", &keys).unwrap().cache_tag(),
            "openai/gpt-4o-mini-tts"
        );
        assert_eq!(select("piper", &none).unwrap().cache_tag(), "piper");
    }

    #[test]
    fn every_listed_voice_is_pinned_and_the_default_is_listed() {
        assert!(voice(DEFAULT_VOICE).is_some());
        for v in VOICES {
            assert_eq!(v.onnx_sha256.len(), 64, "{}", v.name);
            assert_eq!(v.json_sha256.len(), 64, "{}", v.name);
            assert!(v.dir.ends_with("/medium"));
            // Nothing fine-tuned from lessac, whose data is not licensed for commercial use.
            assert!(!v.about.contains("lessac"));
        }
    }

    #[test]
    fn sentences_end_at_punctuation_followed_by_space_not_inside_a_number() {
        let s = sentences("Version 3.5 is out. Visit kaviri.dev! Then\nrecord");
        assert_eq!(
            s,
            vec!["Version 3.5 is out.", "Visit kaviri.dev!", "Then", "record"]
        );
        assert!(sentences("   ").is_empty());
    }

    #[test]
    fn spread_words_fill_the_span_in_order_and_longer_words_take_longer() {
        let w = spread_words("a wonderful, day.", 1.0, 3.0);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].start, 1.0);
        assert!((w[2].end - 3.0).abs() < 1e-9);
        assert!(w.windows(2).all(|p| p[0].end <= p[1].start));
        assert!(w[1].end - w[1].start > w[0].end - w[0].start);
        assert!(spread_words("", 0.0, 1.0).is_empty());
        assert!(spread_words("x", 1.0, 1.0).is_empty());
    }

    #[test]
    fn the_silent_engine_lasts_as_long_as_the_line_would() {
        let dir = std::env::temp_dir().join(format!("kaviri-tts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "This is kaviri. It films your app while you talk about it.";
        let r = Silent.synth(text, "none", &dir).unwrap();
        let (s, rate) = read_wav(&r.audio).unwrap();
        let secs = s.len() as f64 / rate as f64;
        assert!((secs - estimate_secs(text)).abs() < 0.01);
        let words = r.words.unwrap();
        assert_eq!(words.len(), 12);
        assert!(words.last().unwrap().end <= secs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wav_survives_the_round_trip_and_stereo_is_folded() {
        let w = wav_bytes(&[0.0, 0.5, -0.5], 22_050);
        let (s, r) = parse_wav(&w).unwrap();
        assert_eq!(r, 22_050);
        assert_eq!(s.len(), 3);
        assert!((s[1] - 0.5).abs() < 1e-3);
        assert!(parse_wav(b"RIFF....WAVE").is_err());
        assert!(parse_wav(b"nope").is_err());
    }

    #[test]
    fn elevenlabs_character_timings_become_words() {
        let chars: Vec<String> = "Hi you".chars().map(|c| c.to_string()).collect();
        let starts = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5];
        let ends = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let w = words_from_chars(&chars, &starts, &ends);
        assert_eq!(
            w,
            vec![
                Word {
                    text: "Hi".into(),
                    start: 0.0,
                    end: 0.2
                },
                Word {
                    text: "you".into(),
                    start: 0.3,
                    end: 0.6
                },
            ]
        );
    }

    #[test]
    fn a_cloud_request_carries_the_text_and_the_key_stays_out_of_argv() {
        let b = openai_body("gpt-4o-mini-tts", "Hello", "nova");
        assert_eq!(b["input"], "Hello");
        assert_eq!(b["voice"], "nova");
        assert_eq!(b["response_format"], "wav");
        assert_eq!(curl_quote("a\"b\\c"), "a\\\"b\\\\c");
    }
}
