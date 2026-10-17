//! `kaviri mcp` end to end, over real stdio JSON-RPC, against a real browser.
//!
//! Ignored by default because it needs Chromium and ffmpeg and takes about a minute:
//!
//!   cargo test --test mcp_stdio -- --ignored
//!
//! It runs one browser at a time on purpose: the interactive one is closed before the
//! whole-take tool launches its own.

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(takes: &std::path::Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kaviri"))
            .arg("mcp")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("KAVIRI_TAKES_DIR", takes)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn kaviri mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            next: 0,
        }
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server hung up"
            );
            // Every line on stdout must be JSON-RPC: the protocol channel is private.
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON on the protocol channel ({e}): {line}"));
            if v["id"] == json!(id) {
                assert!(v.get("error").is_none(), "{method} failed: {v}");
                return v["result"].clone();
            }
        }
    }

    /// Call a tool; returns its envelope and the full result.
    fn call(&mut self, name: &str, args: Value) -> (Value, Value) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let text = r["content"][0]["text"].as_str().expect("text content");
        let env: Value = serde_json::from_str(text).unwrap();
        (env, r)
    }

    fn ok(&mut self, name: &str, args: Value) -> Value {
        let (env, _) = self.call(name, args);
        assert_eq!(env["ok"], json!(true), "{name}: {env}");
        env["result"].clone()
    }
}

#[test]
#[ignore = "needs chromium and ffmpeg; run with --ignored"]
fn an_agent_can_explore_film_and_look_at_a_take_over_mcp() {
    let takes = std::env::temp_dir().join(format!("kaviri-mcp-test-{}", std::process::id()));
    let mut c = Client::start(&takes);

    let init = c.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "kaviri-test", "version": "0"}}),
    );
    assert_eq!(init["serverInfo"]["name"], "kaviri");
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let tools = c.request("tools/list", json!({}));
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for want in [
        "open",
        "inspect",
        "click",
        "record_demo",
        "get_take",
        "preview",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }

    // Explore: the selectors come from the page, not from the test.
    c.ok(
        "open",
        json!({"url": "examples/demo.html", "preset": "readme"}),
    );
    let seen = c.ok("inspect", json!({}));
    let els = seen["elements"].as_array().unwrap();
    let find = |role: &str, name: &str| {
        els.iter()
            .find(|e| e["role"] == role && e["name"] == name)
            .unwrap_or_else(|| panic!("no {role} named {name} in {seen}"))
            .clone()
    };
    let field = find("textbox", "Full name");
    assert_eq!(field["selector"], "#name");
    assert_eq!(field["in_view"], true);
    let b = field["box"].as_array().unwrap();
    assert!(b[2].as_f64().unwrap() > 10.0 && b[3].as_f64().unwrap() > 5.0);
    let button = find("button", "Sign up");

    // Film it step by step.
    let started = c.ok("start", json!({}));
    let take = started["take_id"].as_str().unwrap().to_string();
    c.ok("wait", json!({"ms": 400}));
    c.ok(
        "type",
        json!({"selector": field["selector"], "text": "Ada"}),
    );
    c.ok("click", json!({"selector": button["selector"]}));
    c.ok("wait", json!({"ms": 1600}));
    let (env, _) = c.call("click", json!({"selector": "#nope"}));
    assert_eq!(
        env["ok"],
        json!(false),
        "a missing selector is an in-band error"
    );
    let stopped = c.ok("stop", json!({}));
    assert_eq!(stopped["take_id"], json!(take));
    assert!(stopped["zoom_events"].as_u64().unwrap() >= 1, "{stopped}");
    assert!(std::path::Path::new(stopped["path"].as_str().unwrap()).exists());

    let (env, full) = c.call("preview", json!({"id": take, "frames": 4}));
    assert_eq!(env["ok"], json!(true), "{env}");
    let images: Vec<&Value> = full["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "image")
        .collect();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0]["mimeType"], "image/jpeg");
    c.ok("close", json!({}));

    // The whole-take tool answers at once and is polled.
    let script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/form.jsonl"),
    )
    .unwrap();
    let queued = c.ok(
        "record_demo",
        json!({"script_jsonl": script, "preset": "readme"}),
    );
    let id = queued["take_id"].as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(300);
    let done = loop {
        let t = c.ok("get_take", json!({"id": id}));
        match t["status"].as_str().unwrap() {
            "done" => break t,
            "failed" => panic!("take failed: {t}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "take still {t}");
        std::thread::sleep(Duration::from_secs(2));
    };
    assert!(done["zoom_events"].as_u64().unwrap() >= 1, "{done}");
    assert!(done["marks"].as_array().unwrap().len() >= 4, "{done}");
    assert!(std::path::Path::new(done["path"].as_str().unwrap()).exists());

    drop(c.stdin);
    let _ = c.child.wait();
    let _ = std::fs::remove_dir_all(&takes);
}
