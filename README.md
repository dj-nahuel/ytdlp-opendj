# ytdlp-opendj

An optional add-on for [openDJ](https://github.com/openwave-studio/opendj)
that adds a **YouTube** source to the library rail, through
[yt-dlp](https://github.com/yt-dlp/yt-dlp). It's meant as a last resort for
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
| 7 | YouTube Premium sign-in for 256 kbps audio | next |

Paste a YouTube, YouTube Music, SoundCloud or hearthis.at link into the
search box to list that track, or every track of a playlist or set (up to
200). A playlist openDJ sends with its songs (as from "Add to Downloads")
has each song looked up on YouTube, three results each, released audio
from the artist's own channel first, so "Tick best of each" picks those.

The play button on a result plays it while it streams in, from a file in
the add-on's folder that's removed when you stop. Sizes shown before
downloading are estimates from the length.

Downloads are the audio as the site sends it, with no converting, so no
ffmpeg is needed: AAC from YouTube (openDJ can't play YouTube's Opus),
MP3 or AAC from SoundCloud, or the artist's own file when they allow
downloads. They're saved as `Artist - Title`, three at a time, with the
artist, title, cover and a comment saying where they came from.

Audio from YouTube and SoundCloud is lossy (128 to 160 kbps without
Premium). Buy a proper copy when one comes out.

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

## Installing it

You need [Rust](https://rustup.rs). Then:

```sh
git clone https://github.com/dj-nahuel/ytdlp-opendj
cd ytdlp-opendj
scripts/install.sh
```

This builds `opendj-addon-ytdlp` and puts it in openDJ's add-ons folder.
Restart openDJ to load it. openDJ starts every program in that folder and
talks to it over stdin and stdout, so openDJ itself is never rebuilt.

## Licence

GPL-3.0-or-later.
