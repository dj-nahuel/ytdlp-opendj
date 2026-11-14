//! Searching YouTube and SoundCloud with yt-dlp: one yt-dlp per site,
//! started with `--flat-playlist -j`, which prints a line of JSON for each
//! result as soon as the site answers, without opening every video. Also
//! pasted links (a track or a playlist on YouTube, SoundCloud or
//! hearthis.at), and a playlist's songs looked up on YouTube one by one.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use opendj_plugins::source::Found;
use serde_json::Value;

use crate::tool::hide_window;

/// Results asked for from each site.
pub const PER_SITE: usize = 15;
/// Said of a result from YouTube's own artist channel (" - Topic"): the
/// released audio, not a video's soundtrack.
pub const OFFICIAL: &str = "official audio";
/// Results asked for each song of a playlist.
pub const PER_SONG: usize = 3;
/// Songs of a playlist looked up at once.
pub const SONGS_AT_ONCE: usize = 3;
/// Songs of a playlist looked up, at most.
pub const MAX_SONGS: usize = 100;
/// Tracks read from a pasted playlist link, at most.
pub const MAX_LINKED: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    YouTube,
    SoundCloud,
    /// Links only: it has no search yt-dlp can use.
    HearThis,
}

impl Site {
    /// The sites a typed search goes to.
    pub const SEARCHED: [Site; 2] = [Site::YouTube, Site::SoundCloud];

    pub fn name(self) -> &'static str {
        match self {
            Site::YouTube => "YouTube",
            Site::SoundCloud => "SoundCloud",
            Site::HearThis => "hearthis.at",
        }
    }

    /// yt-dlp's search prefix.
    fn prefix(self) -> &'static str {
        match self {
            Site::YouTube => "ytsearch",
            Site::SoundCloud => "scsearch",
            Site::HearThis => "",
        }
    }

    /// The start of a result's key, which downloads read back.
    pub fn tag(self) -> &'static str {
        match self {
            Site::YouTube => "yt",
            Site::SoundCloud => "sc",
            Site::HearThis => "ht",
        }
    }

    /// Where its track pages start.
    pub fn page_prefix(self) -> &'static str {
        match self {
            Site::YouTube => "https://www.youtube.com/",
            Site::SoundCloud => "https://soundcloud.com/",
            Site::HearThis => "https://hearthis.at/",
        }
    }
}

/// The site a pasted link is on, if it's one the add-on reads.
pub fn link_site(url: &str) -> Option<Site> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    match host.as_str() {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be" => {
            Some(Site::YouTube)
        }
        "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com" | "on.soundcloud.com" => {
            Some(Site::SoundCloud)
        }
        "hearthis.at" | "www.hearthis.at" => Some(Site::HearThis),
        _ => None,
    }
}

/// yt-dlp's arguments for reading a pasted link. YouTube playlists are read
/// flat (fast, and every entry has its title); a SoundCloud set's flat
/// entries have no titles, so those are read in full.
pub fn link_args(site: Site, url: &str) -> Vec<String> {
    let mut a = vec![
        "--dump-json".into(),
        "--no-warnings".into(),
        "--ignore-config".into(),
        "--no-progress".into(),
        "--ignore-errors".into(),
        "--playlist-end".into(),
        MAX_LINKED.to_string(),
    ];
    if site == Site::YouTube {
        a.insert(0, "--flat-playlist".into());
    }
    a.push(url.trim().to_string());
    a
}

/// yt-dlp's arguments for searching `site` for `text`.
pub fn args(site: Site, text: &str, n: usize) -> Vec<String> {
    vec![
        "--flat-playlist".into(),
        "--dump-json".into(),
        "--no-warnings".into(),
        "--ignore-config".into(),
        "--no-progress".into(),
        format!("{}{n}:{}", site.prefix(), text.trim()),
    ]
}

/// "Artist - Title" from a video's title, and the channel when the title
/// doesn't say. YouTube's own artist channels end in " - Topic".
pub fn split_title(title: &str, channel: &str) -> (String, String) {
    let channel = channel.trim().trim_end_matches(" - Topic").trim();
    for dash in [" - ", " – ", " — "] {
        if let Some((a, t)) = title.split_once(dash) {
            let (a, t) = (a.trim(), t.trim());
            if !a.is_empty() && !t.is_empty() {
                return (a.to_string(), t.to_string());
            }
        }
    }
    (channel.to_string(), title.trim().to_string())
}

/// "1.2M plays".
pub fn plays(n: u64) -> String {
    match n {
        0..=999 => format!("{n} plays"),
        1_000..=999_999 => format!("{}K plays", n / 1_000),
        _ => format!("{:.1}M plays", n as f64 / 1_000_000.0),
    }
}

/// One result line from yt-dlp, as a track.
pub fn parse(site: Site, line: &str) -> Option<Found> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let title = text("title");
    if title.is_empty() {
        return None;
    }
    // A video's id, or the track's page.
    let id = match site {
        Site::YouTube => Some(text("id")).filter(|id| !id.is_empty())?,
        _ => [text("webpage_url"), text("original_url"), text("url")]
            .into_iter()
            .find(|u| u.starts_with(site.page_prefix()))?,
    };
    let (channel, cover) = match site {
        Site::YouTube => (
            text("channel"),
            format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
        ),
        _ => (
            text("uploader"),
            Some(biggest_thumbnail(&v))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| text("thumbnail")),
        ),
    };
    // YouTube Music and SoundCloud name the artist themselves.
    let named = v["artists"][0]
        .as_str()
        .or(v["artist"].as_str())
        .unwrap_or_default()
        .split(", ")
        .next()
        .unwrap_or_default();
    let track = v["track"].as_str().unwrap_or_default();
    let (artist, title) = if !named.is_empty() && site == Site::YouTube && !track.is_empty() {
        (named.to_string(), track.to_string())
    } else if !named.is_empty() && site == Site::SoundCloud {
        (named.to_string(), title)
    } else {
        split_title(&title, &channel)
    };
    let mut from = site.name().to_string();
    let topic = channel.ends_with(" - Topic");
    let channel = channel.trim_end_matches(" - Topic");
    if !channel.is_empty() {
        from.push_str(&format!(" · {channel}"));
    }
    if topic {
        from.push_str(&format!(" · {OFFICIAL}"));
    }
    if let Some(n) = v["view_count"].as_u64() {
        from.push_str(&format!(" · {}", plays(n)));
    }
    Some(Found {
        key: format!("{}:{}", site.tag(), id),
        artist,
        title,
        duration: v["duration"].as_f64().unwrap_or(0.0),
        // What downloads take: YouTube's AAC stream, the others' MP3.
        format: match site {
            Site::YouTube => "M4A".into(),
            _ => "MP3".into(),
        },
        quality: "~128 kbps".into(),
        from,
        cover_url: cover,
        ..Found::default()
    })
}

fn biggest_thumbnail(v: &Value) -> String {
    v["thumbnails"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t["width"].as_u64().unwrap_or(0), t["url"].as_str()?)))
        .max_by_key(|(w, _)| *w)
        .map(|(_, u)| u.to_string())
        .unwrap_or_default()
}

/// The last thing yt-dlp said on stderr, for when a search fails.
pub fn last_error(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            let l = l.trim_start_matches("ERROR:").trim();
            // Not the "please report this issue" that follows.
            l.split("; please report").next().unwrap_or(l).to_string()
        })
        .unwrap_or_else(|| "yt-dlp stopped".into())
}

/// One site's search, running.
#[derive(Default)]
pub struct Running {
    pub items: Vec<Found>,
    pub done: bool,
    pub error: Option<String>,
    /// A pasted playlist's name, once read.
    pub title: String,
    /// For a playlist's songs: how many there are, and how many have been
    /// looked up.
    pub songs: usize,
    pub looked_up: usize,
    stopped: bool,
    child: Option<Child>,
}

pub type Shared = Arc<Mutex<Running>>;

/// Starts yt-dlp at `program` searching `site` for `text`; results land in
/// the returned [`Shared`] as they come.
pub fn spawn(program: &Path, site: Site, text: &str) -> Shared {
    run(program, args(site, text, PER_SITE), site, false)
}

/// Starts yt-dlp reading a pasted link on `site`.
pub fn spawn_link(program: &Path, site: Site, url: &str) -> Shared {
    run(program, link_args(site, url), site, true)
}

fn run(program: &Path, args: Vec<String>, site: Site, link: bool) -> Shared {
    let shared: Shared = Arc::default();
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut r = shared.lock().unwrap();
            r.done = true;
            r.error = Some(format!("couldn't start yt-dlp: {e}"));
            return shared.clone();
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    shared.lock().unwrap().child = Some(child);
    let err_text = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = stderr {
        let err_text = err_text.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut t = err_text.lock().unwrap();
                t.push_str(&line);
                t.push('\n');
            }
        });
    }
    let out = shared.clone();
    std::thread::spawn(move || {
        let mut found = 0;
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(item) = parse(site, &line) {
                    found += 1;
                    let mut r = out.lock().unwrap();
                    if link && r.title.is_empty() {
                        r.title = playlist_title(&line);
                    }
                    r.items.push(item);
                }
            }
        }
        let child = out.lock().unwrap().child.take();
        let ok = child
            .and_then(|mut c| c.wait().ok())
            .is_some_and(|s| s.success());
        let mut r = out.lock().unwrap();
        r.done = true;
        if !ok && found == 0 && !r.stopped {
            // Give stderr's reader a moment to finish.
            std::thread::sleep(std::time::Duration::from_millis(50));
            r.error = Some(last_error(&err_text.lock().unwrap()));
        }
    });
    shared
}

/// The playlist a linked result came from, if it came from one.
pub fn playlist_title(line: &str) -> String {
    let v: Value = serde_json::from_str(line.trim()).unwrap_or_default();
    v["playlist_title"]
        .as_str()
        .or(v["playlist"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// "3. Bonobo – Kerala": how a playlist's song heads its results.
pub fn group(n: usize, song: &str) -> String {
    let song = match song.split_once(" - ") {
        Some((a, t)) => format!("{} – {}", a.trim(), t.trim()),
        None => song.trim().to_string(),
    };
    format!("{n}. {song}")
}

/// A song's results, best first: YouTube's own artist channels (" -
/// Topic", the released audio) before the rest, in YouTube's order.
pub fn rank(mut items: Vec<Found>) -> Vec<Found> {
    items.sort_by_key(|f| !f.from.contains(OFFICIAL));
    items
}

/// Looks up each of `songs` ("Artist - Title") on YouTube, a few at a time,
/// skipping the groups in `skip`.
pub fn spawn_songs(program: &Path, songs: &[String], skip: &[String]) -> Shared {
    let shared: Shared = Arc::default();
    let queue: Vec<(String, String)> = songs
        .iter()
        .take(MAX_SONGS)
        .enumerate()
        .map(|(i, song)| (group(i + 1, song), song.clone()))
        .filter(|(g, _)| !skip.contains(g))
        .collect();
    {
        let mut r = shared.lock().unwrap();
        r.songs = queue.len();
        r.done = queue.is_empty();
    }
    let queue = Arc::new(Mutex::new(
        queue.into_iter().collect::<std::collections::VecDeque<_>>(),
    ));
    for _ in 0..SONGS_AT_ONCE {
        let (queue, out, program) = (queue.clone(), shared.clone(), program.to_path_buf());
        std::thread::spawn(move || loop {
            if out.lock().unwrap().stopped {
                return;
            }
            let Some((group, song)) = queue.lock().unwrap().pop_front() else {
                return;
            };
            let found = look_up(&program, &song);
            let mut r = out.lock().unwrap();
            r.items.extend(rank(found).into_iter().map(|mut f| {
                f.group = group.clone();
                f
            }));
            r.looked_up += 1;
            r.done = r.looked_up >= r.songs;
        });
    }
    shared
}

/// One song's YouTube results.
fn look_up(program: &Path, song: &str) -> Vec<Found> {
    let mut cmd = Command::new(program);
    cmd.args(args(Site::YouTube, song, PER_SONG))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| parse(Site::YouTube, l))
        .collect()
}

/// Stops a search that's still running.
pub fn kill(shared: &Shared) {
    let mut r = shared.lock().unwrap();
    r.stopped = true;
    if let Some(child) = r.child.as_mut() {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_search() {
        let a = args(Site::YouTube, " Bonobo - Kerala ", 15);
        assert_eq!(a.last().unwrap(), "ytsearch15:Bonobo - Kerala");
        assert!(a.contains(&"--flat-playlist".to_string()));
        assert_eq!(
            args(Site::SoundCloud, "x", 10).last().unwrap(),
            "scsearch10:x"
        );
    }

    #[test]
    fn splits_titles() {
        assert_eq!(
            split_title("Bonobo - Kerala (Official Video)", "Ninja Tune"),
            ("Bonobo".into(), "Kerala (Official Video)".into())
        );
        assert_eq!(
            split_title("Kerala", "Bonobo - Topic"),
            ("Bonobo".into(), "Kerala".into())
        );
        assert_eq!(
            split_title("Fred again.. – Delilah", "x"),
            ("Fred again..".into(), "Delilah".into())
        );
        assert_eq!(split_title(" - x", "Chan"), ("Chan".into(), "- x".into()));
    }

    #[test]
    fn reads_a_youtube_result() {
        let line = r#"{"_type":"url","ie_key":"Youtube","id":"abc123","url":"https://www.youtube.com/watch?v=abc123","title":"Bonobo - Kerala","duration":245.0,"channel":"Ninja Tune","view_count":1234567}"#;
        let f = parse(Site::YouTube, line).unwrap();
        assert_eq!(f.key, "yt:abc123");
        assert_eq!((f.artist.as_str(), f.title.as_str()), ("Bonobo", "Kerala"));
        assert_eq!(f.duration, 245.0);
        assert_eq!(f.from, "YouTube · Ninja Tune · 1.2M plays");
        assert_eq!(f.cover_url, "https://i.ytimg.com/vi/abc123/hqdefault.jpg");
        assert!(!f.folder);
    }

    #[test]
    fn reads_a_soundcloud_result() {
        let line = r#"{"_type":"url","url":"https://api-v2.soundcloud.com/tracks/1","webpage_url":"https://soundcloud.com/dj/edit","title":"Kerala (DJ edit)","uploader":"dj","duration":301.5,"view_count":5400,"artists":["Bonobo"],"thumbnails":[{"url":"https://i1.sndcdn.com/a-small.jpg","width":100},{"url":"https://i1.sndcdn.com/a-t500x500.jpg","width":500}]}"#;
        let f = parse(Site::SoundCloud, line).unwrap();
        assert_eq!(f.key, "sc:https://soundcloud.com/dj/edit");
        assert_eq!(f.artist, "Bonobo");
        assert_eq!(f.title, "Kerala (DJ edit)");
        assert_eq!(f.from, "SoundCloud · dj · 5K plays");
        assert_eq!(f.cover_url, "https://i1.sndcdn.com/a-t500x500.jpg");
    }

    #[test]
    fn skips_lines_that_arent_results() {
        assert!(parse(Site::YouTube, "not json").is_none());
        assert!(parse(Site::YouTube, r#"{"id":"x"}"#).is_none());
        assert!(parse(
            Site::SoundCloud,
            r#"{"title":"x","url":"https://evil.example/"}"#
        )
        .is_none());
    }

    #[test]
    fn counts_plays() {
        assert_eq!(plays(950), "950 plays");
        assert_eq!(plays(5_400), "5K plays");
        assert_eq!(plays(2_500_000), "2.5M plays");
    }

    #[test]
    fn keeps_yt_dlps_last_error() {
        assert_eq!(
            last_error("WARNING: x\nERROR: [youtube] Sign in to confirm you're not a bot\n\n"),
            "[youtube] Sign in to confirm you're not a bot"
        );
        assert_eq!(
            last_error("ERROR: Unable to download API page: 403; please report this issue on x"),
            "Unable to download API page: 403"
        );
        assert_eq!(last_error(""), "yt-dlp stopped");
    }

    /// A stand-in yt-dlp that prints two results and then fails.
    #[cfg(unix)]
    #[test]
    fn runs_a_search_and_collects_results() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("yt-dlp");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho '{\"id\":\"a\",\"title\":\"A - One\",\"channel\":\"c\"}'\n\
             echo 'junk'\necho '{\"id\":\"b\",\"title\":\"Two\",\"channel\":\"B - Topic\"}'\n\
             echo 'ERROR: late' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = spawn(&fake, Site::YouTube, "q");
        for _ in 0..200 {
            if s.lock().unwrap().done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let r = s.lock().unwrap();
        assert!(r.done);
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.items[1].artist, "B");
        // Results came, so the late error isn't shown.
        assert!(r.error.is_none());
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn knows_link_sites() {
        assert_eq!(
            link_site("https://www.youtube.com/watch?v=x"),
            Some(Site::YouTube)
        );
        assert_eq!(
            link_site("https://music.youtube.com/playlist?list=PL1"),
            Some(Site::YouTube)
        );
        assert_eq!(link_site("https://youtu.be/x"), Some(Site::YouTube));
        assert_eq!(
            link_site("https://soundcloud.com/a/sets/b"),
            Some(Site::SoundCloud)
        );
        assert_eq!(
            link_site("https://on.soundcloud.com/abc"),
            Some(Site::SoundCloud)
        );
        assert_eq!(
            link_site("https://hearthis.at/dj/mix/"),
            Some(Site::HearThis)
        );
        assert_eq!(link_site("https://youtube.com.evil.example/x"), None);
        assert_eq!(link_site("https://open.spotify.com/x"), None);
        assert_eq!(link_site("not a link"), None);
    }

    #[test]
    fn reads_links_flat_only_on_youtube() {
        let yt = link_args(Site::YouTube, " https://youtu.be/x ");
        assert_eq!(yt[0], "--flat-playlist");
        assert_eq!(yt.last().unwrap(), "https://youtu.be/x");
        let sc = link_args(Site::SoundCloud, "https://soundcloud.com/a/sets/b");
        assert!(!sc.contains(&"--flat-playlist".to_string()));
        assert!(sc.contains(&"--ignore-errors".to_string()));
    }

    #[test]
    fn reads_youtube_musics_own_artist() {
        let line = r#"{"id":"m1","title":"Kerala","channel":"Bonobo - Topic","artist":"Bonobo, Ninja Tune","track":"Kerala","duration":245,"playlist_title":"Migration"}"#;
        let f = parse(Site::YouTube, line).unwrap();
        assert_eq!((f.artist.as_str(), f.title.as_str()), ("Bonobo", "Kerala"));
        assert_eq!(f.from, "YouTube · Bonobo · official audio");
        assert_eq!(playlist_title(line), "Migration");
        assert_eq!(playlist_title(r#"{"id":"x"}"#), "");
    }

    #[test]
    fn reads_a_hearthis_track() {
        let line = r#"{"id":"123","title":"Moofi - Dr. Kreep","webpage_url":"https://hearthis.at/moofi/dr-kreep/","thumbnail":"https://img.hearthis.at/a.jpg","duration":70,"view_count":40}"#;
        let f = parse(Site::HearThis, line).unwrap();
        assert_eq!(f.key, "ht:https://hearthis.at/moofi/dr-kreep/");
        assert_eq!(
            (f.artist.as_str(), f.title.as_str()),
            ("Moofi", "Dr. Kreep")
        );
        assert_eq!(f.cover_url, "https://img.hearthis.at/a.jpg");
        assert_eq!(f.from, "hearthis.at · 40 plays");
        assert_eq!(f.format, "MP3");
    }

    #[test]
    fn heads_songs_and_puts_official_audio_first() {
        assert_eq!(group(3, "Bonobo - Kerala"), "3. Bonobo – Kerala");
        assert_eq!(group(1, "Kerala"), "1. Kerala");
        let f = |key: &str, from: &str| Found {
            key: key.into(),
            from: from.into(),
            ..Found::default()
        };
        let ranked = rank(vec![
            f("a", "YouTube · Fan"),
            f("b", "YouTube · Bonobo · official audio"),
            f("c", "YouTube · Ninja Tune"),
        ]);
        let keys: Vec<&str> = ranked.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["b", "a", "c"]);
    }

    /// A stand-in yt-dlp answering every song search with one result.
    #[cfg(unix)]
    #[test]
    fn looks_up_each_song() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-songs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("yt-dlp");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a; do q=\"$a\"; done\n\
             case \"$q\" in *Missing*) exit 0;; esac\n\
             echo \"{\\\"id\\\":\\\"v\\\",\\\"title\\\":\\\"${q#ytsearch3:}\\\",\\\"channel\\\":\\\"c\\\"}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let songs = vec![
            "A - One".to_string(),
            "B - Missing".into(),
            "C - Three".into(),
        ];
        let s = spawn_songs(&fake, &songs, &["3. C – Three".to_string()]);
        for _ in 0..300 {
            if s.lock().unwrap().done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let r = s.lock().unwrap();
        assert!(r.done);
        assert_eq!((r.songs, r.looked_up), (2, 2));
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].group, "1. A – One");
        assert_eq!(
            (r.items[0].artist.as_str(), r.items[0].title.as_str()),
            ("A", "One")
        );
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
