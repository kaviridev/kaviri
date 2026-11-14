# kaviri socials

Everything needed to open and fill the brand accounts. The PNGs in this folder are
rendered from the social kit canvas and are upload-ready at each platform's size.

Never promote kaviri from a personal account. Each platform gets a kaviri brand
account, and the posting schedule is the 6-month content calendar (3 posts a week,
hosted beta in December, launch in late January, teams in February).

## Handle

**`kaviridev`** everywhere, because it matches kaviri.dev. Plain `kaviri` is taken on
GitHub (a 2014 user account), so it is not worth chasing on the others.

| Platform | Handle | Profile | Banner |
| --- | --- | --- | --- |
| GitHub org | `kaviridev` | `icon-512.png` | n/a |
| X | `@kaviridev` | `profile-800.png` | `x-header-1500x500.png` |
| Bluesky | `@kaviri.dev` (domain handle, DNS TXT) | `profile-800.png` | `x-header-1500x500.png` |
| LinkedIn company page | `kaviridev` | `profile-800.png` | `linkedin-banner-1584x396.png` |
| YouTube | `@kaviridev` | `profile-800.png` | `youtube-banner-2560x1440.png` |
| TikTok | `@kaviridev` | `profile-800.png` | n/a |
| dev.to | `kaviridev` | `profile-800.png` | n/a |

Sign every account up with `hello@kaviri.dev` (it forwards to the owner), so none of
them is tied to a personal address.

## Bios

**X / Bluesky** (160 max):

> Your demo video is a build artifact. kaviri records product demos from a script in
> your repo, in CI. Free and open source. kaviri.dev

**LinkedIn tagline** (120 max):

> Product demos that update when the product does. Recorded from a script in your repo, re-rendered in CI.

**LinkedIn about / YouTube description:**

> kaviri records product demo videos from a script that lives next to your code. Run it
> in CI and the video is made again whenever the product changes, so the demo in your
> README, docs and landing page never goes stale. The camera follows what the script
> does, with zooms and pans you never have to describe.
>
> The recorder is free and open source under Apache 2.0. The hosted service adds
> rendering with no CI setup, stable video URLs, take history and pull request previews.
>
> kaviri (ka-VEE-ree) is ChiShona for "twice, a second time".
> kaviri.dev

**YouTube channel line:** New demo-craft Shorts weekly.

**TikTok** (80 max):

> Demo videos recorded from a script, in CI. Free, open source.

**GitHub org description:** The open-source demo recorder for web apps that runs in your CI.

Link in every bio: `https://kaviri.dev`

## Upload notes

- The profile picture is cropped to a circle everywhere, so the mark sits centred.
- On X and LinkedIn the profile picture covers the lower left of the banner, so the
  banner's text sits to the right.
- YouTube shows only the centre 1546x423 of the banner on every device. All text is
  inside it.
- Add `shorts-end-card-1080x1920.png` as the last 2 to 3 seconds of every Short.
- `og-1200x630.png` is the link preview for kaviri.dev, already served at `/og.png`.

## Positioning

"The open-source demo recorder for web apps that runs in your CI." Do not compete with
polished screen recorders on polish, and stay friendly towards them. Lead with the
failing build, not the zoom (see `../README.md`, The one-liner).

## Order of setup

1. GitHub org `kaviridev`, then transfer `kaviri` and `kaviri-cloud` into it. GitHub
   redirects the old URLs, but update `uses: thisisisheanesu/kaviri@v1` in the docs and
   the site in the same change.
2. X, Bluesky, LinkedIn page, YouTube, TikTok, dev.to, each with the files above.
3. Connect X, LinkedIn and YouTube to Buffer's free plan (3 channels) and load the
   calendar from 16 Nov 2026.
4. Pin the 16 Nov intro thread on X and Bluesky.
