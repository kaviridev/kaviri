# Sound

A take can come out with four kinds of sound, mixed into one AAC track at a set loudness:

| source | how you ask for it | where it lands |
|---|---|---|
| narration | a `say` op in the script | at the moment the op ran |
| a recorded clip | a `voice` op in the script | at the moment the op ran |
| a voiceover | `--voiceover <file>` | from the first frame |
| a music bed | `--music <file>` | the whole take, looped, faded, ducked under speech |
| the page | `--audio` | where the page played it |

Captions come from what the narration says: an `.srt` and a `.vtt` beside the MP4, and with
`--captions burn`, drawn into the picture.

Nothing here needs a sound card, PulseAudio or PipeWire. It all works headless, on a CI runner.

```sh
kaviri doctor --fix          # once: installs the local speech engine and a voice
kaviri record --script examples/narrated.jsonl --out narrated.mp4 \
  --preset tiktok --music bed.mp3 --captions burn
```

## Narration: `say`

```jsonc
{"op":"say","text":"Signing up takes two fields."}
{"op":"say","text":"Now the export.","voice":"en_US-norman-medium","wait":true}
```

| field | type | default | notes |
|---|---|---|---|
| `text` | string | required | what to say |
| `voice` | string | `--voice`, else the engine's default | an engine voice name, or a path to a Piper `.onnx` |
| `wait` | bool | `false` | hold here until the line has been said |
| `pad_ms` | number | `150` | extra pause after the line, with `wait` |
| `gain_db` | number | `0` | this line's level, -60 to 24 |

**The line starts where the op is, and the script keeps going.** `say` returns as soon as the
line is synthesised and placed, so whatever comes next happens while it is being said. That is
what you want for "watch the total update" followed by the click that updates it.

**The next `wait` stretches until the line has been said.** A `{"op":"wait","ms":300}` after a
two-second line waits two seconds, and its result says `"stretched_from": 300`. That is the rule
that lines the actions up with the words: write `say`, then `wait`, then the action the line
introduces, and the action lands after the words whatever the voice's pace. A wait that should
not stretch says `"stretch": false`. `{"op":"wait","speech":true}` waits exactly to the end of
the line in flight.

`stop_recording` lets a line in flight finish too, plus a breath, so the last sentence is never
cut off.

The result of a `say` says how long the line is and when it ends on the take's clock:

```jsonc
{"ok":true,"result":{"event":"mark","kind":"say","label":"Signing up takes two fields.",
  "t":0.24,"secs":2.108,"until":2.352,"voice":"en_US-ljspeech-medium","timed_words":true,
  "recording":true}}
```

A `say` before `start_recording` synthesises the line (so it is cached for the take) and puts
nothing on the timeline.

**Synthesis is not filmed as a freeze.** Under `record` every line is synthesised before the
browser starts, so a missing voice or a bad key fails in a second, and the take finds every line
already made. Under `serve` and `kaviri mcp`, synthesis runs on its own thread while the frame
pump keeps going.

**Lines are cached** by engine, voice and text in `~/.cache/kaviri/tts` (or `$KAVIRI_CACHE`,
or `$XDG_CACHE_HOME/kaviri`). The second take of a script costs nothing, and with a cloud engine
it is not billed twice.

## A recorded clip: `voice`

```jsonc
{"op":"voice","file":"intro.m4a","text":"Welcome to the dashboard."}
```

| field | type | default | notes |
|---|---|---|---|
| `file` | string | required | wav, mp3, m4a, anything ffmpeg reads |
| `text` | string | none | what the clip says, for captions; spread evenly over it |
| `wait` | bool | `false` | hold here until the clip has played |
| `gain_db` | number | `0` | -60 to 24 |

It is placed exactly like a `say` line, and a following `wait` stretches to it in the same way.
`record` checks every clip exists before the browser starts.

## Speech engines: `--tts`

| engine | cost | where it runs | word timings | licence of what you get |
|---|---|---|---|---|
| `piper` (default) | free | locally, as a separate process | per sentence, words spread within | voices below: public domain data, MIT model |
| `none` | free | nowhere | estimated | silence |
| `openai` | your key | OpenAI | spread evenly | OpenAI's terms |
| `elevenlabs` | your key | ElevenLabs | per character, from the API | ElevenLabs' terms |
| `cloudflare` | your key | Workers AI (MeloTTS) | spread evenly | MeloTTS is MIT |

`--tts` sets the engine, `KAVIRI_TTS` sets the default. Nothing is ever sent to a cloud engine
unless it is named; the key comes from the environment (`OPENAI_API_KEY`,
`ELEVENLABS_API_KEY`, `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`) and goes to curl on
stdin, so it never appears in the process list. `KAVIRI_OPENAI_TTS_MODEL` and
`KAVIRI_ELEVENLABS_MODEL` override the models (`gpt-4o-mini-tts`, `eleven_multilingual_v2`).
For OpenAI the voice is a voice name (`alloy`, `nova`, `onyx`...), for ElevenLabs a voice id,
for Cloudflare a language code (`en`).

**`none`** writes silence as long as the line would take to say, at 2.6 words a second, with
the words spread over it. A script's timing, its captions and its mix can then be tested on a
machine with no engine and no network. The integration test uses it.

### Piper, and the voices kaviri will fetch

`kaviri doctor --fix` downloads the Piper release for the machine and the default voice into
`~/.cache/kaviri`, and checks each file against a SHA-256 pinned in `src/tts.rs` before it is
used. `kaviri doctor --fix --voice <name>` adds another. `kaviri voices` lists them:

| voice | |
|---|---|
| `en_US-ljspeech-medium` | US English, female. The default |
| `en_US-kristin-medium` | US English, female |
| `en_US-norman-medium` | US English, male |
| `en_US-john-medium` | US English, male |
| `en_GB-cori-medium` | British English, female |

Every one is trained on public-domain recordings (LJ Speech, LibriVox), none descends from
`lessac`, and each comes from a pinned revision of `rhasspy/piper-voices`, which is MIT. Any other Piper voice works by path
(`"voice":"/path/to/voice.onnx"`, with its `.onnx.json` beside it), and its licence is then
yours to check: many Piper voices are fine-tuned from `lessac`, whose Blizzard 2013 data is not
licensed for commercial use, and some (`ryan`, `hfc_*`) are CC BY-NC-SA. kaviri does not list
those.

`$KAVIRI_PIPER` points at a Piper binary of your own, which is then used instead of the one in
the cache; otherwise `piper` on `PATH` is used if `doctor --fix` was never run.

**Why Piper is fetched, not bundled.** The Piper release contains espeak-ng, the phonemiser
that turns text into sounds, and espeak-ng is GPL-3.0. kaviri is Apache-2.0 and must not ship
it, so Piper is treated exactly like ffmpeg: a separate program the user installs (here, by
asking `doctor --fix` to), run as a child process, never linked and never in kaviri's archive.
[THIRD-PARTY.md](../THIRD-PARTY.md) has the detail.

### Engines evaluated and not wired up

- **Kokoro-82M** (Apache-2.0 weights and voices) sounds better than Piper's medium voices. It
  needs a phonemiser too: its own `misaki` is Python, and the ONNX ports (`kokoro-onnx`,
  `sherpa-onnx`) fall back to, or statically link, espeak-ng, so it carries the same GPL
  constraint as Piper and a 300 MB download. It fits behind the same trait as a separate
  `sherpa-onnx-offline-tts` process, and is the next engine to add.
- **In-process ONNX** through the `ort` crate was ruled out for the same reason: with no
  permissively licensed English phonemiser in Rust, the model runs in process but the text
  front end still has to be espeak-ng.

## A voiceover: `--voiceover`

`--voiceover narration.m4a` lays a recorded track under the whole take, from its first frame,
cut to the take's length. `--voiceover-gain <dB>` sets its level. The music ducks under it as it
does under narration. A recorded clip that belongs at a moment rather than at the start is a
`voice` op.

## A music bed: `--music`

`--music bed.mp3` loops the file to the take's length (`--music-loop off` plays it once), fades
it in and out over `--music-fade` (1500 ms by default, capped at a third of the take), sets it
to `--music-volume` (-18 dB by default), and ducks it under narration and voiceover.

**Ducking** is a sidechain compressor keyed on the speech: any line pulls the bed down by most
of 20 dB within 25 ms, and it comes back over about half a second after the line ends.
`--duck off` leaves the bed level. With no narration or voiceover there is nothing to duck under,
and the bed plays at its level throughout.

Use music you have the rights to. kaviri ships none.

## The mix

```text
page ─────────── volume ───────────────────────────────┐
narration ─┐                                           ├─ amix ─ loudnorm ─ AAC
voiceover ─┴─ volume ─ amix ─ asplit ──────────────────┤
                                 └─ key ─┐             │
music ─ loop ─ trim ─ fades ─ volume ─ sidechaincompress
```

Every source has its own level: `--page-gain`, `--narration-gain` (all `say` and `voice` clips;
`gain_db` on an op adjusts one), `--voiceover-gain` and `--music-volume`, all in dB.

**Loudness is normalised to -16 LUFS** by default (EBU R128 integrated loudness, true peak
-1.5 dBTP), which is what the social platforms normalise to, so a take is neither quieter nor
squashed when it is posted. `--loudness -23` targets broadcast; `--loudness off` leaves the mix
as it is. It is two passes of ffmpeg's `loudnorm`: the first measures, the second applies one
linear gain, so the ducking survives. A single dynamic pass would pump the bed back up between
sentences. The video stream is copied, not re-encoded, unless captions are burned in.

The result of `stop_recording` says what went in:

```jsonc
"soundtrack":{"sources":["narration","music"],"lines":3,"loudness":-16.0,"measured_lufs":-18.4,
  "captions":{"cues":3,"burned":true,"srt":"narrated.srt","vtt":"narrated.vtt"}}
```

## Captions

`--captions` is one of:

- `auto` (the default): an `.srt` and a `.vtt` beside the MP4 when the take says something,
  nothing otherwise.
- `file`: the sidecars, always.
- `burn`: the sidecars, and the captions drawn into the video. This re-encodes the video once.
- `off`.

Cues follow the words. Piper's timings are exact at each sentence and spread by word length
inside it; ElevenLabs times every character; the others are spread by word length over the line.
A cue is at most two lines, of 22 characters on a vertical video, 42 on a landscape one and 30
on a square one, and breaks at the end of a sentence or after a comma once it is half full. Each
cue stays up 0.6 s after its last word, never into the next.

`--caption-style` is `pill` (white on a dark translucent pill, like kaviri's own chrome; the
default), `bold` (large heavy white type with a dark edge, the social look) or `plain`.

**Safe areas.** On a 9:16 video the bottom fifth belongs to the platform (caption, handle,
sound) and the right edge to its buttons. Captions sit 24% of the height up and 14% in from the
sides, which clears TikTok, Reels and Shorts. A landscape take puts them 7% up, a square one 10%.

Burned captions are drawn by the browser, one transparent PNG per cue, and overlaid for their
seconds. ffmpeg's text filters need a build with libass or freetype, which a static build may
not have, and the browser sets type far better.

## The page's own sound: `--audio`

`--audio` taps what the page plays (every connection to an `AudioContext`'s destination, and
every `<audio>` and `<video>`) and mixes it in as the `page` source. How it is captured is in
the comments at the top of `src/audio.rs`.

**Under `--slowmo`** the page's clock runs k times slower than the wall clock, and its audio does
not: a context plays at the sound card's rate whatever the page thinks the time is. So each burst
of sound starts at the page-time moment it began and plays from there at its own speed and pitch.
A click, a chime or a notification lands on the frame that caused it and sounds the way it
should. What this cannot fix is a long continuous sound kept in step with the visuals, such as a
game's music or a `<video>`: the sound runs on in real time while the picture runs k times slower,
so it drifts. Film those without `--slowmo`.

## Over MCP

`record_demo` and `open` take the same settings as fields: `tts`, `voice`, `voiceover`,
`voiceover_gain`, `music`, `music_volume`, `music_fade`, `music_loop`, `duck`, `page_gain`,
`narration_gain`, `loudness`, `captions`, `caption_style`. On/off fields take `true` or `false`.
The interactive session has `say` and `voice` tools, the ops of the same name.
