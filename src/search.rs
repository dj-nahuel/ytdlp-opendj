//! Searching YouTube and SoundCloud with yt-dlp: one yt-dlp per site,
//! started with `--flat-playlist -j`, which prints a line of JSON for each
//! result as soon as the site answers, without opening every video.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use opendj_plugins::source::Found;
use serde_json::Value;

use crate::tool::hide_window;

/// Results asked for from each site.
pub const PER_SITE: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    YouTube,
    SoundCloud,
}

impl Site {
    pub const ALL: [Site; 2] = [Site::YouTube, Site::SoundCloud];

    pub fn name(self) -> &'static str {
        match self {
            Site::YouTube => "YouTube",
            Site::SoundCloud => "SoundCloud",
        }
    }

    /// yt-dlp's search prefix.
    fn prefix(self) -> &'static str {
        match self {
            Site::YouTube => "ytsearch",
            Site::SoundCloud => "scsearch",
        }
    }

    /// The start of a result's key, which A3 reads back to download it.
    pub fn tag(self) -> &'static str {
        match self {
            Site::YouTube => "yt",
            Site::SoundCloud => "sc",
        }
    }
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
    // A video's id, or a SoundCloud track's page.
    let id = match site {
        Site::YouTube => Some(text("id")).filter(|id| !id.is_empty())?,
        Site::SoundCloud => [text("webpage_url"), text("url")]
            .into_iter()
            .find(|u| u.starts_with("https://soundcloud.com/"))?,
    };
    let (channel, cover) = match site {
        Site::YouTube => (
            text("channel"),
            format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
        ),
        Site::SoundCloud => (text("uploader"), biggest_thumbnail(&v)),
    };
    let named = v["artists"][0].as_str().unwrap_or_default();
    let (artist, title) = if site == Site::SoundCloud && !named.is_empty() {
        (named.to_string(), title)
    } else {
        split_title(&title, &channel)
    };
    let mut from = format!("{} · {}", site.name(), channel.trim_end_matches(" - Topic"));
    if let Some(n) = v["view_count"].as_u64() {
        from.push_str(&format!(" · {}", plays(n)));
    }
    Some(Found {
        key: format!("{}:{}", site.tag(), id),
        artist,
        title,
        duration: v["duration"].as_f64().unwrap_or(0.0),
        // What A3 downloads: YouTube's AAC stream, SoundCloud's MP3 or AAC.
        format: match site {
            Site::YouTube => "M4A".into(),
            Site::SoundCloud => "MP3".into(),
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
    child: Option<Child>,
}

pub type Shared = Arc<Mutex<Running>>;

/// Starts yt-dlp at `program` searching `site`; results land in the
/// returned [`Shared`] as they come.
pub fn spawn(program: &Path, site: Site, text: &str) -> Shared {
    let shared: Shared = Arc::default();
    let mut cmd = Command::new(program);
    cmd.args(args(site, text, PER_SITE))
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
                    out.lock().unwrap().items.push(item);
                }
            }
        }
        let child = out.lock().unwrap().child.take();
        let ok = child
            .and_then(|mut c| c.wait().ok())
            .is_some_and(|s| s.success());
        let mut r = out.lock().unwrap();
        r.done = true;
        if !ok && found == 0 {
            // Give stderr's reader a moment to finish.
            std::thread::sleep(std::time::Duration::from_millis(50));
            r.error = Some(last_error(&err_text.lock().unwrap()));
        }
    });
    shared
}

/// Stops a site's search that's still running.
pub fn kill(shared: &Shared) {
    if let Some(child) = shared.lock().unwrap().child.as_mut() {
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
}
