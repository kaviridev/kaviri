# kaviri as an MCP server

`kaviri mcp` is the op protocol as a [Model Context Protocol](https://modelcontextprotocol.io)
server on stdio. An agent in Claude Code, Claude Desktop, Cursor or any other MCP client can
open a page, find out what is on it, film it and look at the result, without writing a file
or reading NDJSON off a pipe.

It is the same recorder. Each interactive tool is one op, run through the same session as
`kaviri serve`, against one browser that stays open across calls. The whole-take tool runs
`kaviri record` itself. A script filmed through MCP and the same script filmed on the command
line are the same take.

Everything runs on your machine. There is no account, no hosted service and nothing to pay
for: the browser and ffmpeg are the ones `kaviri doctor` finds.

## Install

Build kaviri (see the README; the MCP server needs Rust 1.88 or newer, because the official
Rust SDK, `rmcp`, does), put the binary on your PATH, then:

```sh
claude mcp add kaviri -- kaviri mcp
```

Or check it into a project as `.mcp.json`, so everyone working on the repo gets it:

```json
{
  "mcpServers": {
    "kaviri": {
      "command": "kaviri",
      "args": ["mcp"],
      "env": { "KAVIRI_TAKES_DIR": "demos/takes" }
    }
  }
}
```

Any client that launches a stdio server takes the same command and arguments. `kaviri mcp`
accepts the browser flags `serve` does (`--chromium`, `--spool-dir`, `--keep-temp` and the
rest) as defaults for the browser it opens; `--port` is refused, because MCP here is stdio
only, and so is `--out`, because each take names its own file (`out` on `start` and
`record_demo`, or `$KAVIRI_TAKES_DIR`).

Takes land in `kaviri-takes/` under the directory the client started the server in, which for
a coding agent is the project. `$KAVIRI_TAKES_DIR` moves them. Add the directory to
`.gitignore`; kaviri's own does.

## Two ways to film

**A whole take, in the background.** Write the script as JSONL, exactly as for
`kaviri record`, and call `record_demo`. It answers at once with a take id. Poll `get_take`
until the status is `done` or `failed`, then call `preview`. Use this for anything longer than
a few steps: a client gives up on a single tool call after a minute or two, and a take under
`--slowmo 8` can run for ten.

**Step by step.** `open`, `navigate`, `inspect` to find the selectors, `start`, the
interactions, `stop`. The browser stays open between calls, and capture keeps running while
the agent thinks, so the take is one continuous video rather than a set of clips.

## The tools

| tool | what it does |
|---|---|
| `open` | launch (or relaunch) the browser: `url`, `preset`, `width`, `height`, `scale`, `frame`, `frame_style`, `background`, `cursor`, `slowmo`, `audio` |
| `navigate` | the `navigate` op |
| `inspect` | the visible actionable elements: role, accessible name, a unique CSS selector, box |
| `start` | `start_recording`; returns the take id and the MP4 path (`out` to choose it) |
| `click`, `type`, `hover`, `press`, `scroll`, `wait`, `mark` | the op of the same name, same fields |
| `say`, `voice` | narration at this moment, and a recorded clip; see [audio.md](audio.md) |
| `frame` | the `frame` op; before `start` |
| `stop` | `stop_recording`; renders, returns path, duration, `zoom_events`, marks, take id |
| `close` | close the browser, rendering any open take first |
| `record_demo` | `script_jsonl` plus optional `preset`, `frame`, `frame_style`, `background`, `slowmo`, `audio`, the sound fields below, `out`; returns a take id |
| `get_take` | status (`queued`, `running`, `done`, `failed`), path, duration, `zoom_events`, marks; no id lists every take |
| `preview` | a contact sheet of keyframes and a poster, as images |

`open` and `record_demo` also take the soundtrack, as the flags of the same name: `tts`,
`voice`, `voiceover`, `voiceover_gain`, `music`, `music_volume`, `music_fade`, `music_loop`,
`duck`, `page_gain`, `narration_gain`, `loudness`, `captions`, `caption_style`. `get_take`
reports the take's `soundtrack`.

Every tool answers with the same envelope the NDJSON protocol uses, as the text of the result:
`{"ok":true,"result":{…}}`, or `{"ok":false,"error":"…"}` with the result marked as an error.
The fields of each op are documented in [the script protocol](script-protocol.md). A tool
called before `open` opens a browser with the flags the server was started with.

`open` takes the same values as the command-line flags and runs them through the same parser,
so a bad preset is refused with the list of presets. Settings belong to the browser: to film
in a different shape, call `open` again.

Takes run one at a time. A `record_demo` sent while another is filming is `queued` until the
first finishes. The interactive browser is separate, so close it before a long `record_demo`
if the machine is short of memory.

## inspect

`examples/demo.html` under `--preset readme`:

```json
{"ok":true,"result":{"event":"inspect","url":"file:///…/demo.html","viewport":[1100,620],
 "count":3,"elements":[
  {"role":"textbox","name":"Full name","selector":"#name","box":[360.0,258.5,380.0,39.0],"in_view":true},
  {"role":"textbox","name":"Email","selector":"#email","box":[360.0,333.5,380.0,39.0],"in_view":true},
  {"role":"button","name":"Sign up","selector":"#submit","box":[360.0,394.5,380.0,39.0],"in_view":true}]}}
```

The list comes from the browser's accessibility tree, so a `<div role="button">` is a button
and an input is named by its `<label>`. The selector is the shortest one that matches exactly
that element, preferring an id, a test id, `name`, `aria-label` or `placeholder` over a
structural path. The box is in CSS pixels, the coordinates every mark carries. `covered`
names an element sitting on top of the target's centre, which `click` would refuse. Elements
scrolled out of view are counted in `offscreen` and listed with `"all": true`; a script can
still reach them, because `click` scrolls first. `limit` caps the list (default 150).

`inspect` is an op as well, so `{"op":"inspect"}` works in `kaviri serve` too.

## preview

An agent cannot watch an MP4. `preview` takes a frame just after each interaction landed
(when the camera has settled on it), tops a sparse take up to six with evenly spaced
moments, thins a busy one to `frames` (default 9), and tiles them into one JPEG. The second
image is the poster: the moment the last interaction landed, which is usually the payoff.
Both are written beside the MP4 as `<take>.sheet.jpg` and `<take>.poster.jpg`.

Look at the sheet before handing a take on. A tile that shows the wide shot where you expected
a close-up, or the wrong part of the page, means the script needs another look.

## What it costs

Nothing. The server, the browser and ffmpeg all run locally, and kaviri makes no network
request of its own. The only pages it loads are the ones you send it to.
