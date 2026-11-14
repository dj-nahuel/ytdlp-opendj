//! Browsing the DJ's own lists, through openDJ's "Search for: Playlist":
//! on YouTube, Liked Music, liked videos, Watch Later and their playlists
//! (read with the browser sign-in); on SoundCloud, the likes and sets of
//! the profile named in Settings. Each list shows as a folder that opens to
//! its tracks, and downloads whole.

use std::path::Path;
use std::process::Stdio;

use opendj_plugins::source::Found;
use serde_json::Value;

use crate::kind::Kind;
use crate::search::{self, Shared, Site};

/// What a list's key starts with.
pub const LIST: &str = "list:";

/// The DJ's lists that are always there, by name and address.
pub fn fixed(kind: Kind, profile: Option<&str>) -> Vec<(String, String)> {
    match kind {
        Kind::YouTube => [
            ("Liked Music", "https://music.youtube.com/playlist?list=LM"),
            ("Liked videos", "https://www.youtube.com/playlist?list=LL"),
            ("Watch Later", "https://www.youtube.com/playlist?list=WL"),
        ]
        .iter()
        .map(|(n, u)| (n.to_string(), u.to_string()))
        .collect(),
        Kind::SoundCloud => profile
            .map(|p| vec![("Likes".to_string(), format!("{p}/likes"))])
            .unwrap_or_default(),
    }
}

/// Where the DJ's own playlists are listed.
pub fn playlists_page(kind: Kind, profile: Option<&str>) -> Option<String> {
    match kind {
        Kind::YouTube => Some("https://www.youtube.com/feed/playlists".into()),
        Kind::SoundCloud => profile.map(|p| format!("{p}/sets")),
    }
}

/// A SoundCloud profile typed in Settings, as its page's address:
/// "soundcloud.com/name", "https://soundcloud.com/name/likes" or just
/// "name".
pub fn profile(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches('/');
    let t = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let t = t
        .strip_prefix("www.")
        .or_else(|| t.strip_prefix("m."))
        .unwrap_or(t);
    let name = t.strip_prefix("soundcloud.com/").unwrap_or(t);
    let name = name.split('/').next()?.trim();
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| format!("https://soundcloud.com/{name}"))
}

/// A list as a folder row.
pub fn folder(kind: Kind, name: &str, url: &str) -> Found {
    Found {
        key: format!("{LIST}{url}"),
        folder: true,
        title: name.to_string(),
        from: kind.site_name().to_string(),
        ..Found::default()
    }
}

/// A list's address from its key.
pub fn url(key: &str) -> Option<&str> {
    key.strip_prefix(LIST)
}

/// One playlist from the DJ's playlists page, as a folder: its title, or
/// for a SoundCloud set (whose flat entry has none) its address's last
/// part made readable.
pub fn parse_list(kind: Kind, line: &str) -> Option<Found> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let url = v["url"].as_str().or(v["webpage_url"].as_str())?.trim();
    if url.is_empty() {
        return None;
    }
    let title = v["title"].as_str().unwrap_or_default().trim().to_string();
    let title = if title.is_empty() {
        let slug = url.trim_end_matches('/').rsplit('/').next()?;
        slug.replace(['-', '_'], " ")
    } else {
        title
    };
    let mut f = folder(kind, &title, url);
    f.track_count = v["playlist_count"].as_u64().unwrap_or_default() as usize;
    Some(f)
}

/// yt-dlp's arguments for listing the playlists on `page`.
pub fn list_args(page: &str) -> Vec<String> {
    vec![
        "--flat-playlist".into(),
        "--dump-json".into(),
        "--no-warnings".into(),
        "--ignore-config".into(),
        "--no-progress".into(),
        "--ignore-errors".into(),
        page.to_string(),
    ]
}

/// The DJ's lists whose names contain `filter`: the fixed ones at once,
/// their playlists as yt-dlp at `program` reads them.
pub fn spawn(program: &Path, kind: Kind, profile: Option<&str>, filter: &str) -> Shared {
    let filter = filter.trim().to_lowercase();
    let wanted = move |f: &Found| filter.is_empty() || f.title.to_lowercase().contains(&filter);
    let shared: Shared = Shared::default();
    shared.lock().unwrap().items = fixed(kind, profile)
        .iter()
        .map(|(n, u)| folder(kind, n, u))
        .filter(&wanted)
        .collect();
    let Some(page) = playlists_page(kind, profile) else {
        shared.lock().unwrap().done = true;
        return shared;
    };
    let site = kind.searched()[0];
    let mut cmd = crate::tool::command(program);
    cmd.args(crate::premium::args(site))
        .args(list_args(&page))
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    let out = shared.clone();
    std::thread::spawn(move || {
        let result = cmd.output();
        let mut r = out.lock().unwrap();
        r.done = true;
        match result {
            Ok(o) => {
                let lists = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| parse_list(kind, l))
                    .filter(&wanted)
                    .collect::<Vec<_>>();
                if lists.is_empty() && !o.status.success() {
                    r.error = Some(search::last_error(&String::from_utf8_lossy(&o.stderr)));
                }
                r.items.extend(lists);
            }
            Err(e) => r.error = Some(format!("couldn't start yt-dlp: {e}")),
        }
    });
    shared
}

/// The tracks of list `url` on `site`, read now with yt-dlp at `program`.
pub fn open(program: &Path, site: Site, url: &str) -> Result<Vec<Found>, String> {
    let out = crate::tool::command(program)
        .args(crate::premium::args(site))
        .args(search::link_args(site, url))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't start yt-dlp: {e}"))?;
    let tracks: Vec<Found> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| search::parse(site, l))
        .collect();
    if tracks.is_empty() && !out.status.success() {
        return Err(search::last_error(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_soundcloud_profile() {
        let want = Some("https://soundcloud.com/dj-x".to_string());
        assert_eq!(profile("dj-x"), want);
        assert_eq!(profile("soundcloud.com/dj-x"), want);
        assert_eq!(profile("https://soundcloud.com/dj-x/likes/"), want);
        assert_eq!(profile("https://m.soundcloud.com/dj-x"), want);
        assert_eq!(profile(""), None);
        assert_eq!(profile("not a name"), None);
    }

    #[test]
    fn lists_the_fixed_lists() {
        let yt = fixed(Kind::YouTube, None);
        assert_eq!(yt[0].0, "Liked Music");
        assert_eq!(fixed(Kind::SoundCloud, None), Vec::new());
        let sc = fixed(Kind::SoundCloud, Some("https://soundcloud.com/x"));
        assert_eq!(sc[0].1, "https://soundcloud.com/x/likes");
        assert_eq!(
            playlists_page(Kind::SoundCloud, Some("https://soundcloud.com/x")).as_deref(),
            Some("https://soundcloud.com/x/sets")
        );
        assert_eq!(playlists_page(Kind::SoundCloud, None), None);
    }

    #[test]
    fn reads_playlists() {
        let yt = parse_list(
            Kind::YouTube,
            r#"{"url":"https://www.youtube.com/playlist?list=PL1","title":"Warm-up","playlist_count":12}"#,
        )
        .unwrap();
        assert!(yt.folder);
        assert_eq!(yt.title, "Warm-up");
        assert_eq!(yt.track_count, 12);
        assert_eq!(
            url(&yt.key),
            Some("https://www.youtube.com/playlist?list=PL1")
        );
        let sc = parse_list(
            Kind::SoundCloud,
            r#"{"url":"https://soundcloud.com/x/sets/late-night-edits"}"#,
        )
        .unwrap();
        assert_eq!(sc.title, "late night edits");
        assert!(parse_list(Kind::YouTube, r#"{"title":"no url"}"#).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn spawns_and_opens() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = crate::testutil::fake(
            &dir,
            r#"case "$*" in
  *feed/playlists*) echo '{"url":"https://www.youtube.com/playlist?list=PL1","title":"Warm-up"}'
                    echo '{"url":"https://www.youtube.com/playlist?list=PL2","title":"Peak"}' ;;
  *) echo '{"id":"abc","title":"A - B","channel":"C","duration":200}' ;;
esac"#,
        );
        let s = spawn(&program, Kind::YouTube, None, "");
        for _ in 0..200 {
            if s.lock().unwrap().done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let titles: Vec<String> = s
            .lock()
            .unwrap()
            .items
            .iter()
            .map(|f| f.title.clone())
            .collect();
        assert_eq!(
            titles,
            [
                "Liked Music",
                "Liked videos",
                "Watch Later",
                "Warm-up",
                "Peak"
            ]
        );
        let peak = spawn(&program, Kind::YouTube, None, "pea");
        for _ in 0..200 {
            if peak.lock().unwrap().done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(peak.lock().unwrap().items.len(), 1);
        let tracks = open(
            &program,
            Site::YouTube,
            "https://www.youtube.com/playlist?list=PL1",
        )
        .unwrap();
        assert_eq!(tracks[0].key, "yt:abc");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
