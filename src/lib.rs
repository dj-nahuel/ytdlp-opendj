//! An optional add-on that gives openDJ a YouTube and SoundCloud source in
//! its rail, through yt-dlp (<https://github.com/yt-dlp/yt-dlp>): a last
//! resort for tracks that can't be bought, such as edits, bootlegs and
//! out-of-print records.
//!
//! The add-on fetches the official yt-dlp build for this computer and keeps
//! it current (see [`tool`]). yt-dlp runs as a separate program; openDJ
//! never links it.
//!
//! It isn't part of openDJ: see this repository's README. What is
//! downloaded, and whether the DJ has the right to it, is up to the DJ.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use opendj_plugins::source::{
    DownloadUpdate, Found, SearchSource, Snapshot, SourceInfo, SourceQuery, StatusPage,
    StatusSection,
};
use opendj_plugins::{Manifest, Plugin, PluginConfig, PluginError, Setting, SettingKind};

pub mod tool;

use tool::{State, Tool};

const MANIFEST: Manifest = Manifest {
    id: "ytdlp",
    name: "YouTube",
    hosts: &[],
    downloads: true,
};

const INFO: SourceInfo = SourceInfo {
    name: "YouTube",
    icon_svg: include_bytes!("../icon.svg"),
    hint: "Artist - Title, or a YouTube or SoundCloud link",
};

/// Said while searching isn't built yet.
const SOON: &str = "Searching comes in the next version of the add-on.";

pub struct YtDlp {
    tool: Tool,
    /// The add-on's own folder in openDJ's config.
    dir: PathBuf,
    update: bool,
}

impl Default for YtDlp {
    fn default() -> Self {
        Self::new()
    }
}

impl YtDlp {
    pub fn new() -> Self {
        Self {
            tool: Tool::default(),
            dir: std::env::temp_dir().join("opendj-ytdlp"),
            update: true,
        }
    }

    /// A line on how yt-dlp stands, for Settings and the panel.
    fn line(&self) -> String {
        line(&self.tool.state())
    }
}

/// A line on how yt-dlp stands.
pub fn line(state: &State) -> String {
    match state {
        State::Idle => "Getting ready…".into(),
        State::Fetching => "Downloading yt-dlp (about 35 MB)…".into(),
        State::Failed(why) => format!("yt-dlp isn't available: {why}"),
        State::Ready(r) if r.own => format!("Using your own yt-dlp {}", r.version),
        State::Ready(r) => match &r.update_error {
            Some(why) => format!("yt-dlp {} · update failed: {why}", r.version),
            None => format!("yt-dlp {} · ready", r.version),
        },
    }
}

/// The add-on's page in its panel: which yt-dlp it runs.
pub fn page(state: &State, now: SystemTime) -> StatusPage {
    let mut rows = Vec::new();
    if let State::Ready(r) = state {
        rows.push(("Version".to_string(), r.version.clone()));
        rows.push((
            "Updates".to_string(),
            if r.own {
                "Your own yt-dlp (OPENDJ_YTDLP_BIN): not updated".to_string()
            } else {
                match r.checked {
                    Some(t) => format!("Daily · last checked {}", ago(t, now)),
                    None => "Daily · not checked yet".to_string(),
                }
            },
        ));
        rows.push(("Program".to_string(), r.path.display().to_string()));
    }
    StatusPage {
        title: "yt-dlp".into(),
        line: match state {
            State::Ready(r) => r.version.clone(),
            State::Fetching => "Downloading…".into(),
            State::Failed(_) => "Not available".into(),
            State::Idle => String::new(),
        },
        note: line(state),
        sections: vec![StatusSection {
            heading: String::new(),
            rows,
        }],
    }
}

/// "3 h ago", "just now".
pub fn ago(then: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(then).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

impl Plugin for YtDlp {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn source(&mut self) -> Option<&mut dyn SearchSource> {
        Some(self)
    }
    fn settings(&self) -> Vec<Setting> {
        vec![
            Setting::new("", SettingKind::Note, "", &self.line()),
            Setting::new(
                "update",
                SettingKind::Switch(true),
                "Keep yt-dlp up to date",
                "Looks for a newer yt-dlp once a day. Sites change often, and an old \
                 yt-dlp stops finding things.",
            ),
            Setting::new(
                // openDJ's `source::DOWNLOAD_FOLDER`.
                "download_folder",
                SettingKind::Folder,
                "Download folder",
                "By default, Downloaded/YouTube in your music folder.",
            ),
            Setting::new(
                "about",
                SettingKind::Note,
                "About these downloads",
                "Audio from YouTube and SoundCloud is lossy (128 to 160 kbps). Use it for \
                 tracks you can't buy, and buy a proper copy when one comes out.",
            ),
        ]
    }
    fn configure(&mut self, config: &PluginConfig) {
        if let Some(dir) = &config.dir {
            self.dir = dir.clone();
        }
        self.update = config.is_on("update", true);
        self.tool.ensure(&self.dir, self.update);
    }
}

impl SearchSource for YtDlp {
    fn info(&self) -> SourceInfo {
        INFO
    }

    fn describe(&self, query: &SourceQuery) -> Vec<String> {
        let text = query.text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        vec![format!("YouTube: {text}"), SOON.to_string()]
    }

    fn start(&mut self, _query: &SourceQuery) -> Result<String, PluginError> {
        // Also a chance to pick up a day-old yt-dlp's update.
        self.tool.ensure(&self.dir, self.update);
        Err(PluginError::Network(SOON.into()))
    }

    fn results(&mut self, _id: &str) -> Result<Snapshot, PluginError> {
        Err(PluginError::NotSupported)
    }

    fn download(&mut self, _id: &str, _items: &[Found], _dir: &Path) -> Result<(), PluginError> {
        Err(PluginError::NotSupported)
    }

    fn progress(&mut self) -> Vec<DownloadUpdate> {
        Vec::new()
    }

    fn status(&mut self) -> Option<StatusPage> {
        Some(page(&self.tool.state(), SystemTime::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tool::Ready;

    fn ready(own: bool, update_error: Option<&str>) -> State {
        State::Ready(Ready {
            path: PathBuf::from("/x/bin/yt-dlp_macos"),
            version: "2025.11.12".into(),
            own,
            checked: Some(SystemTime::UNIX_EPOCH),
            update_error: update_error.map(str::to_string),
        })
    }

    #[test]
    fn says_how_yt_dlp_stands() {
        assert_eq!(line(&ready(false, None)), "yt-dlp 2025.11.12 · ready");
        assert_eq!(line(&ready(true, None)), "Using your own yt-dlp 2025.11.12");
        assert_eq!(
            line(&ready(false, Some("offline"))),
            "yt-dlp 2025.11.12 · update failed: offline"
        );
        assert!(line(&State::Failed("no build".into())).contains("no build"));
    }

    #[test]
    fn page_lists_version_and_updates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(7200);
        let p = page(&ready(false, None), now);
        assert_eq!(p.line, "2025.11.12");
        let rows = &p.sections[0].rows;
        assert_eq!(rows[0], ("Version".into(), "2025.11.12".into()));
        assert_eq!(rows[1].1, "Daily · last checked 2 h ago");
        let own = page(&ready(true, None), now);
        assert!(own.sections[0].rows[1].1.contains("not updated"));
    }

    #[test]
    fn says_how_long_ago() {
        let t = SystemTime::UNIX_EPOCH;
        assert_eq!(ago(t, t + Duration::from_secs(5)), "just now");
        assert_eq!(ago(t, t + Duration::from_secs(600)), "10 min ago");
        assert_eq!(ago(t, t + Duration::from_secs(3 * 86_400)), "3 days ago");
    }

    #[test]
    fn describes_a_search() {
        let y = YtDlp::new();
        let q = SourceQuery {
            text: "Bonobo - Kerala".into(),
            ..Default::default()
        };
        assert_eq!(y.describe(&q)[0], "YouTube: Bonobo - Kerala");
        assert!(y.describe(&SourceQuery::default()).is_empty());
    }
}
