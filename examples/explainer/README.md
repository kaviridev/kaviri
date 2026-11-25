# X in 60 seconds

A narrated explainer, filmed by kaviri from a JSON file. One episode is one file in
`episodes/`; `stage.html` draws it and `make.py` films it.

```sh
python3 examples/explainer/make.py examples/explainer/episodes/mcp.json --out mcp.mp4 \
  --music bed.wav
```

You get a 1080x1920 MP4 with the voice, burned captions and the music ducked under the voice,
plus the kaviri script it ran (`mcp.jsonl`). Keep the whole thing under 60 seconds: about 140
words of narration.

## An episode

```jsonc
{
  "title": "MCP in 60 seconds",
  "voice": "cloudflare:arcas",          // any kaviri voice, engine:voice included
  "accent": "#2fd37e",                  // optional
  "scenes": [ { "type": "title", "title": "MCP", "say": "MCP, in sixty seconds." } ]
}
```

Every scene has `say`, the line narrated while it is on screen. The next scene comes when the
line ends.

| type | fields | on screen |
|---|---|---|
| `title` | `kicker`, `title`, `sub` | a big title |
| `icons` | `center`, `items` [{`icon`, `label`, `crossed`}], `cage`, `heading` | a thing with up to five things around it, crossed out or not |
| `mesh` | `heading`, `left` [], `right` [], `hub`, `count` | everything wired to everything, or through a hub when `hub` is set |
| `cards` | `kicker`, `items` [{`icon`, `title`, `text`, `say`}] | cards that appear one at a time, each with its own line |
| `code` | `kicker`, `code`, `chips` [] | code that types itself out |
| `terminal` | `kicker`, `command`, `output` | kaviri types the command for real, then the output shows |
| `video` | `src`, `label` | a clip or an image in a screen (paths from the repo root, like `/site/demo.mp4`) |
| `statement` | `text` (`*word*` is coloured, `\n` breaks) | a closing line |

The bottom 40% of the frame is left for the captions, which sit above where TikTok, Reels and
Shorts put their own buttons.

## Rules of the format

- One idea. If it needs "and also", it is two episodes.
- Show the real thing where you can: a `terminal` that really runs, a `video` of the product.
- No numbers or claims the source does not state.
- The caption is about the topic; credit kaviri in the first comment, not the caption.
