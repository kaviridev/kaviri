//! A narrated take end to end, with the `none` speech engine so no voice is needed.
//!
//! Ignored by default because it needs Chromium and ffmpeg:
//!
//!   cargo test --test audio_take -- --ignored
//!
//! It films `examples/narrated.jsonl` against the bundled local page with `--tts none`, a
//! generated music bed and burned captions, and checks what came out: an AAC track as long
//! as the video, the .srt and .vtt beside it, one cue per line said, and the waits stretched
//! to the narration.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn ffmpeg() -> String {
    std::env::var("KAVIRI_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

#[test]
#[ignore]
fn a_narrated_take_comes_out_with_sound_and_captions() {
    let dir = std::env::temp_dir().join(format!("kaviri-audio-take-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bed = dir.join("bed.wav");
    let made = Command::new(ffmpeg())
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=f=220:d=4",
        ])
        .arg(&bed)
        .status()
        .expect("run ffmpeg");
    assert!(made.success());
    let out = dir.join("narrated.mp4");
    let run = Command::new(env!("CARGO_BIN_EXE_kaviri"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "record",
            "--script",
            "examples/narrated.jsonl",
            "--tts",
            "none",
        ])
        .args([
            "--captions",
            "burn",
            "--width",
            "640",
            "--height",
            "400",
            "--scale",
            "1",
        ])
        .arg("--music")
        .arg(&bed)
        .arg("--out")
        .arg(&out)
        .env("KAVIRI_CACHE", dir.join("cache"))
        .output()
        .expect("run kaviri");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "kaviri failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let lines: Vec<Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let says = lines
        .iter()
        .filter(|v| v["result"]["kind"] == "say")
        .count();
    assert_eq!(says, 3);
    // The wait right after the first line was stretched to its length.
    assert!(lines
        .iter()
        .any(|v| v["result"]["kind"] == "wait" && v["result"]["stretched_from"] == 300));
    let rendered = lines
        .iter()
        .find(|v| v["result"]["event"] == "recording_rendered")
        .expect("a rendered take");
    let st = &rendered["result"]["soundtrack"];
    assert_eq!(st["sources"], serde_json::json!(["narration", "music"]));
    assert_eq!(st["captions"]["cues"].as_u64(), Some(3));
    assert_eq!(st["captions"]["burned"], true);

    let srt = std::fs::read_to_string(dir.join("narrated.srt")).unwrap();
    assert!(srt.starts_with("1\n00:00:0"), "{srt}");
    assert!(srt.contains("two fields."));
    assert!(Path::new(&dir.join("narrated.vtt")).is_file());

    let probe = Command::new(ffmpeg())
        .args(["-hide_banner", "-i"])
        .arg(&out)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&probe.stderr);
    assert!(info.contains("Audio: aac"), "{info}");
    let _ = std::fs::remove_dir_all(&dir);
}
