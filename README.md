# ytdlp-opendj

An optional add-on for [openDJ](https://github.com/openwave-studio/opendj)
that adds two sources to the library rail, through
[yt-dlp](https://github.com/yt-dlp/yt-dlp): **YouTube** (with YouTube
Music), the widest catalog of released songs, and **SoundCloud DL**
(SoundCloud with hearthis.at), for DJ edits, bootlegs, remixes and free
downloads. Each is its own program with its own settings, so you can order
them or switch one off. (openDJ's own SoundCloud entry is for browsing, so
this one has a name of its own.) It's meant as a last resort for
tracks you can't buy: edits, bootlegs, white labels and out-of-print
records.

It is kept out of openDJ itself and is not distributed with it. **You are
responsible for having the rights to what you download**, and for following
the terms of the sites you download from.

## What it does

The add-on is being built in steps:

| Step | What | State |
| --- | --- | --- |
| 1 | Fetches yt-dlp for your computer, checks it, keeps it up to date | done |
| 2 | Search YouTube and SoundCloud | done |
| 3 | Download audio into `Downloaded/YouTube` | done |
| 4 | Pasted links and "Add to Downloads", hearthis.at | done |
| 5 | Hear a result before downloading | done |
| 6 | Full YouTube support (Deno) and a clear message when YouTube breaks yt-dlp | done |
| 7 | YouTube Premium and SoundCloud Go+ sign-in for 256 kbps audio | done |
| 8 | YouTube and SoundCloud as separate sources | done |
| 9 | Browse your YouTube playlists and SoundCloud likes and sets | done |

Paste a YouTube or YouTube Music link into YouTube's search box, or a
SoundCloud or hearthis.at link into SoundCloud DL's, to list that track, or every track of a playlist or set (up to
200). A playlist openDJ sends with its songs (as from "Add to Downloads")
has each song looked up on that source's site, three results each, released audio
from the artist's own channel first, so "Tick best of each" picks those.

The play button on a result plays it while it streams in, from a file in
the add-on's folder that's removed when you stop. Sizes shown before
downloading are estimates from the length.

Downloads are the audio as the site sends it, with no converting, so no
ffmpeg is needed: AAC from YouTube (openDJ can't play YouTube's Opus),
MP3 or AAC from SoundCloud, or the artist's own file when they allow
downloads. They're saved as `Artist - Title`, three at a time, with the
artist, title, cover and a comment saying where they came from.

Audio from YouTube and SoundCloud is lossy (128 kbps, or 256 kbps with
YouTube Premium or SoundCloud Go+). Buy a proper copy when one comes out.

## yt-dlp

The first time openDJ starts the add-on, it downloads the official yt-dlp
build for your computer from
[yt-dlp's releases](https://github.com/yt-dlp/yt-dlp/releases), checks it
against the release's SHA-256 list, and keeps it in the add-on's folder in
openDJ's settings folder. Once a day it looks for a newer release; turn that
off in **Settings → YouTube**. The add-on's panel shows which version it
runs.

To use a yt-dlp of your own instead, start openDJ with `OPENDJ_YTDLP_BIN`
set to its path. That one is never replaced.

yt-dlp is public domain (Unlicense); its ready-made builds contain GPLv3+
code. It runs as a separate program, and openDJ never links it.

## Deno

YouTube now hides its audio behind JavaScript puzzles that yt-dlp solves
with a JavaScript runtime. Without one, some videos fail or come only in
formats openDJ can't play. So the add-on also fetches
[Deno](https://github.com/denoland/deno/releases) (about 45 MB), checks it
against the release's SHA-256, and hands it to yt-dlp. It looks for a newer
Deno once a day along with yt-dlp. Turn it off with **Full YouTube support**
in **Settings → YouTube**, or set `OPENDJ_DENO_BIN` to use your own.

## When YouTube breaks yt-dlp

YouTube changes its player every few weeks, and searches and downloads
then fail until yt-dlp catches up, usually within days. When the add-on
sees the errors that mean this (not a single missing video), its panel and
Settings say so in one line, and it looks for a newer yt-dlp every hour
until the site works again.

## Your own lists

Set **Search for** to **Playlist** in a source's search bar and search with
the box empty to list your own lists, or type a word to list only those
named with it. Each list opens to its tracks, and downloads whole into a
folder named after it.

- **YouTube** lists Liked Music, liked videos, Watch Later and your
  playlists. It needs **Use my browser's sign-in** switched on (below),
  since these lists are private.
- **SoundCloud DL** lists the likes and sets of the profile in **Your
  SoundCloud profile** (your name, or `soundcloud.com/yourname`). Public
  likes and sets need no sign-in; with the sign-in switched on, private sets
  show too.

## Signing in (YouTube Premium and SoundCloud Go+)

Switch on **Use my browser's sign-in** in the source's settings. The add-on
finds your default browser and checks straight away that it's signed in to
the site; the top of the settings and the source's panel say how it went:

- **Signed in to YouTube through Safari**: done. Your own lists show with
  Search for: Playlist, and with Premium or Go+ downloads come at 256 kbps
  instead of 128.
- **Not signed in**: the site's sign-in page opens in your browser. Sign in
  there; the add-on notices within a few seconds.
- **Safari needs Full Disk Access**: the macOS page for it opens. Switch on
  openDJ there and restart openDJ. Chrome, Brave, Edge and the like ask to
  unlock their safe storage in the Keychain instead: choose Always Allow.

Google stopped yt-dlp signing in to YouTube by itself in 2024, so the
browser's own sign-in is the only way: yt-dlp reads its cookies
(`--cookies-from-browser`). Your password never reaches openDJ. Only
downloads and your lists use it; searches and previews stay signed out. To
use a browser other than the default, start openDJ with
`OPENDJ_YTDLP_BROWSER` set to its name (`safari`, `chrome`, `firefox`,
`brave`, `edge`, `opera`, `vivaldi`).

- The Downloads list says each download's bitrate. The sites don't always
  offer 256 kbps, even signed in; then the download comes at 128 kbps and
  the list says so. A SoundCloud artist's own download file still comes
  first.
- Downloading signed in is against both sites' terms and could get the
  account flagged.

## Installing it

You need [Rust](https://rustup.rs). Then:

```sh
git clone https://github.com/dj-nahuel/ytdlp-opendj
cd ytdlp-opendj
scripts/install.sh
```

This builds `opendj-addon-ytdlp` (YouTube) and `opendj-addon-soundcloud`
(SoundCloud DL) and puts them in openDJ's add-ons folder. Each fetches its
own yt-dlp; only YouTube fetches Deno.
Restart openDJ to load it. openDJ starts every program in that folder and
talks to it over stdin and stdout, so openDJ itself is never rebuilt.

## Licence

GPL-3.0-or-later.
