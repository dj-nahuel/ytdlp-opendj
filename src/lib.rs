//! An optional add-on that gives openDJ a YouTube and a SoundCloud source
//! in its rail (see [`kind`]), through yt-dlp (<https://github.com/yt-dlp/yt-dlp>): a last
//! resort for tracks that can't be bought, such as edits, bootlegs and
//! out-of-print records.
//!
//! The add-on fetches the official yt-dlp build for this computer and keeps
//! it current (see [`tool`]). yt-dlp runs as a separate program; openDJ
//! never links it.
//!
//! It isn't part of openDJ: see this repository's README. What is
//! downloaded, and whether the DJ has the right to it, is up to the DJ.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use opendj_plugins::source::{
    DownloadUpdate, Found, Progress, SearchSource, Snapshot, SourceInfo, SourceQuery, StatusPage,
    StatusSection,
};
use opendj_plugins::{Manifest, Plugin, PluginConfig, PluginError, Setting, SettingKind};

pub mod deno;
pub mod download;
pub mod health;
pub mod kind;
pub mod premium;
pub mod preview;
pub mod search;
pub mod tags;
pub mod tool;

#[cfg(all(test, unix))]
mod testutil;

use deno::Deno;
use download::Downloads;
use health::Health;
pub use kind::Kind;
use search::{Shared, Site};
use tool::{State, Tool};

/// One search: a yt-dlp per site.
struct Search {
    sites: Vec<(Site, Shared)>,
    /// A playlist's songs, each looked up on YouTube.
    songs: bool,
}

pub struct YtDlp {
    kind: Kind,
    tool: Tool,
    /// The add-on's own folder in openDJ's config.
    dir: PathBuf,
    update: bool,
    searches: HashMap<String, Search>,
    next: u64,
    downloads: Downloads,
    preview: Option<preview::Preview>,
    /// The site of the result being heard.
    preview_site: Option<Site>,
    deno: Deno,
    /// Whether to fetch Deno for full YouTube support.
    use_deno: bool,
    health: Health,
    /// The browser YouTube downloads read a Premium sign-in from, or why
    /// the name typed in Settings isn't one.
    premium: Result<Option<String>, String>,
}

impl YtDlp {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            tool: Tool::default(),
            dir: std::env::temp_dir().join("opendj-ytdlp"),
            update: true,
            searches: HashMap::new(),
            next: 0,
            downloads: Downloads::default(),
            preview: None,
            preview_site: None,
            deno: Deno::default(),
            use_deno: true,
            health: Health::default(),
            premium: Ok(None),
        }
    }

    /// A line on how yt-dlp stands, for Settings and the panel: a broken
    /// site first.
    fn line(&self) -> String {
        let state = self.tool.state();
        let version = match &state {
            State::Ready(r) => r.version.as_str(),
            _ => "",
        };
        let line = self.health.line(version).unwrap_or_else(|| line(&state));
        match &self.premium {
            Err(why) => format!("{line} · {why}"),
            Ok(_) => line,
        }
    }

    /// Gives every later yt-dlp run Deno, once it's there.
    fn pass_deno(&self) {
        tool::set_extra_args(self.deno.args());
    }

    /// A failure on `site`: when it looks like the site broke yt-dlp, says
    /// so and looks for a newer yt-dlp now.
    fn failed(&mut self, site: Site, error: &str) {
        if self.health.failed(site, error) && self.update {
            self.tool.check_now(&self.dir);
        }
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

/// A line on Deno.
pub fn deno_line(state: &deno::State) -> String {
    match state {
        deno::State::Off => "Off: some YouTube videos may fail".into(),
        deno::State::Fetching => "Downloading (about 45 MB)…".into(),
        deno::State::Ready(_) => "Ready".into(),
        deno::State::Failed(why) => format!("Not available: {why}"),
    }
}

/// The add-on's page in its panel: which yt-dlp it runs, with Deno, and
/// what seems broken.
pub fn page(
    state: &State,
    deno: &deno::State,
    broken: Option<String>,
    now: SystemTime,
) -> StatusPage {
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
    rows.push(("Deno (full YouTube)".to_string(), deno_line(deno)));
    StatusPage {
        title: "yt-dlp".into(),
        line: match state {
            State::Ready(_) if broken.is_some() => "Site broken, update pending".into(),
            State::Ready(r) => r.version.clone(),
            State::Fetching => "Downloading…".into(),
            State::Failed(_) => "Not available".into(),
            State::Idle => String::new(),
        },
        note: broken.unwrap_or_else(|| line(state)),
        sections: vec![StatusSection {
            heading: String::new(),
            rows,
        }],
    }
}

/// How the Premium sign-in stands.
pub fn premium_line(premium: &Result<Option<String>, String>) -> String {
    match premium {
        Ok(Some(b)) => format!("Signed in through {b}: downloads at 256 kbps where offered"),
        Ok(None) => "Not signed in: downloads at 128 kbps".into(),
        Err(why) => why.clone(),
    }
}

/// What a search's sites have found so far: YouTube's first, then
/// SoundCloud's.
fn snapshot(sites: &[(Site, Shared)]) -> Snapshot {
    let mut items = Vec::new();
    let mut counts = Vec::new();
    let mut errors = Vec::new();
    let mut done = true;
    for (site, shared) in sites {
        let r = shared.lock().unwrap();
        items.extend(r.items.iter().cloned());
        done &= r.done;
        if let Some(e) = &r.error {
            errors.push(format!("{} failed: {e}", site.name()));
        } else if r.done {
            counts.push(format!("{} on {}", r.items.len(), site.name()));
        }
    }
    let note = counts
        .into_iter()
        .chain(errors)
        .collect::<Vec<_>>()
        .join(" · ");
    // A pasted playlist is listed by its name.
    let title = sites
        .first()
        .map(|(_, s)| s.lock().unwrap().title.clone())
        .unwrap_or_default();
    Snapshot {
        items,
        done,
        note,
        title,
    }
}

/// A playlist's songs looked up so far: "18 of 22 found".
fn songs_snapshot(shared: &Shared) -> Snapshot {
    let r = shared.lock().unwrap();
    let mut groups: Vec<&str> = r.items.iter().map(|f| f.group.as_str()).collect();
    groups.dedup();
    let note = if r.done {
        format!("{} of {} found", groups.len(), r.songs)
    } else {
        format!("{} of {} looked up", r.looked_up, r.songs)
    };
    Snapshot {
        items: r.items.clone(),
        done: r.done,
        note,
        title: String::new(),
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
        self.kind.manifest()
    }
    fn source(&mut self) -> Option<&mut dyn SearchSource> {
        Some(self)
    }
    fn settings(&self) -> Vec<Setting> {
        let kind = self.kind;
        let name = kind.site_name();
        let folder = kind.info().name;
        let plan = kind.plan();
        let mut settings = vec![
            Setting::new("", SettingKind::Note, "", &self.line()),
            Setting::new(
                "update",
                SettingKind::Switch(true),
                "Keep yt-dlp up to date",
                "Looks for a newer yt-dlp once a day. Sites change often, and an old \
                 yt-dlp stops finding things.",
            ),
        ];
        if kind.wants_deno() {
            settings.push(Setting::new(
                "deno",
                SettingKind::Switch(true),
                "Full YouTube support",
                "Downloads Deno (about 45 MB), which yt-dlp needs to answer YouTube's player \
                 checks. Without it, some videos fail or come only in formats openDJ can't play.",
            ));
        }
        settings.push(Setting::new(
            "premium_browser",
            SettingKind::Text,
            &format!("{plan} browser"),
            &format!(
                "With {plan}, downloads come at 256 kbps instead of 128 where {name} offers \
                 it. Type the browser you're signed in to {name} with: safari, chrome, \
                 firefox, brave, edge, opera or vivaldi (add :Profile 2 for another Chrome \
                 profile). yt-dlp reads the sign-in from it for each download; your password \
                 never reaches the add-on. Searches and previews stay signed out. On a Mac, \
                 Safari needs Full Disk Access for openDJ, and Chrome-type browsers ask to \
                 unlock their safe storage: choose Always Allow. Downloading signed in is \
                 against {name}'s terms and could get the account flagged. Leave empty to \
                 stay signed out."
            ),
        ));
        settings.push(Setting::new(
            // openDJ's `source::DOWNLOAD_FOLDER`.
            "download_folder",
            SettingKind::Folder,
            "Download folder",
            &format!(
                "Tracks are saved as Artist - Title, tagged with where they came from. By \
                 default, Downloaded/{folder} in your music folder."
            ),
        ));
        settings.push(Setting::new(
            "about",
            SettingKind::Note,
            "About these downloads",
            &format!(
                "Audio from {name} is lossy (128 kbps, 256 with {plan}). Use it for tracks \
                 you can't buy, and buy a proper copy when one comes out."
            ),
        ));
        settings
    }
    fn configure(&mut self, config: &PluginConfig) {
        if let Some(dir) = &config.dir {
            self.dir = dir.clone();
        }
        self.update = config.is_on("update", true);
        self.use_deno = self.kind.wants_deno() && config.is_on("deno", true);
        self.premium = premium::browser(config.get("premium_browser"));
        premium::set(self.premium.clone().unwrap_or(None));
        self.tool.ensure(&self.dir, self.update);
        self.deno.ensure(&self.dir, self.use_deno, self.update);
        self.pass_deno();
    }
}

impl SearchSource for YtDlp {
    fn info(&self) -> SourceInfo {
        self.kind.info()
    }

    fn sites(&self) -> &'static [&'static str] {
        // Links from these, anywhere in openDJ, can be sent here.
        self.kind.hosts()
    }

    fn describe(&self, query: &SourceQuery) -> Vec<String> {
        let text = query.text.trim();
        if !query.songs.is_empty() {
            let n = query.songs.len().min(search::MAX_SONGS);
            let site = self.kind.searched()[0].name();
            return vec![format!("Each of the {n} songs of {text}, on {site}")];
        }
        if text.is_empty() {
            return Vec::new();
        }
        if opendj_plugins::is_web_url(text) {
            return match search::link_site(text).filter(|&s| self.kind.owns(s)) {
                Some(site) => vec![format!(
                    "{} link: the track, or every track of the playlist",
                    site.name()
                )],
                None => vec![self.kind.other_link().to_string()],
            };
        }
        self.kind
            .searched()
            .iter()
            .map(|site| format!("{}: {text}", site.name()))
            .collect()
    }

    fn start(&mut self, query: &SourceQuery) -> Result<String, PluginError> {
        // Also a chance to pick up a day-old yt-dlp's update.
        self.tool.ensure(&self.dir, self.update);
        self.pass_deno();
        let text = query.text.trim();
        let link = opendj_plugins::is_web_url(text) && query.songs.is_empty();
        let site = search::link_site(text).filter(|&s| self.kind.owns(s));
        if link && site.is_none() {
            return Err(PluginError::Network(self.kind.other_link().into()));
        }
        let Some(program) = self.tool.program() else {
            return Err(PluginError::Network(self.line()));
        };
        let search = if !query.songs.is_empty() {
            Search {
                sites: vec![(
                    self.kind.searched()[0],
                    search::spawn_songs(
                        &program,
                        self.kind.searched()[0],
                        &query.songs,
                        &query.skip,
                    ),
                )],
                songs: true,
            }
        } else if let (true, Some(site)) = (link, site) {
            Search {
                sites: vec![(site, search::spawn_link(&program, site, text))],
                songs: false,
            }
        } else {
            Search {
                sites: self
                    .kind
                    .searched()
                    .iter()
                    .map(|&site| (site, search::spawn(&program, site, text)))
                    .collect(),
                songs: false,
            }
        };
        self.next += 1;
        let id = self.next.to_string();
        self.searches.insert(id.clone(), search);
        Ok(id)
    }

    fn results(&mut self, id: &str) -> Result<Snapshot, PluginError> {
        let search = self
            .searches
            .get(id)
            .ok_or_else(|| PluginError::Network("This search ended; search again.".into()))?;
        let snap = if search.songs {
            songs_snapshot(&search.sites[0].1)
        } else {
            snapshot(&search.sites)
        };
        // How each site did, for spotting one that broke yt-dlp.
        let outcomes: Vec<(Site, bool, Option<String>)> = search
            .sites
            .iter()
            .map(|(site, s)| {
                let r = s.lock().unwrap();
                (*site, !r.items.is_empty(), r.error.clone())
            })
            .collect();
        for (site, found, error) in outcomes {
            if found {
                self.health.worked(site);
            } else if let Some(e) = error {
                self.failed(site, &e);
            }
        }
        Ok(snap)
    }

    fn stop(&mut self, id: &str) {
        if let Some(search) = self.searches.remove(id) {
            for (_, s) in &search.sites {
                search::kill(s);
            }
        }
    }

    fn download(&mut self, _id: &str, items: &[Found], dir: &Path) -> Result<(), PluginError> {
        self.downloads
            .add(items, dir)
            .map_err(PluginError::Network)?;
        self.pass_deno();
        self.downloads.pump(self.tool.program().as_deref());
        Ok(())
    }

    fn progress(&mut self) -> Vec<DownloadUpdate> {
        // Downloads asked for while yt-dlp was still arriving start now.
        self.pass_deno();
        self.downloads.pump(self.tool.program().as_deref());
        let updates = self.downloads.progress();
        for u in &updates {
            let Some((site, _)) = download::source(&u.key) else {
                continue;
            };
            match &u.progress {
                Progress::Done(_) => self.health.worked(site),
                Progress::Failed(why) => self.failed(site, why),
                _ => {}
            }
        }
        updates
    }

    fn notes(&mut self) -> Vec<(String, String)> {
        self.downloads.notes()
    }

    fn cancel(&mut self, key: &str) -> Result<(), PluginError> {
        self.downloads.cancel(key).map_err(PluginError::Network)
    }

    fn retry(&mut self, key: &str) -> Result<(), PluginError> {
        self.downloads.retry(key).map_err(PluginError::Network)?;
        self.downloads.pump(self.tool.program().as_deref());
        Ok(())
    }

    fn previews(&self) -> bool {
        true
    }

    fn preview(&mut self, _id: &str, item: &Found) -> Result<(), PluginError> {
        self.stop_preview();
        let program = self
            .tool
            .program()
            .ok_or_else(|| PluginError::Network(self.line()))?;
        self.pass_deno();
        let p = preview::Preview::start(&program, &item.key, &self.dir.join("preview"))
            .map_err(PluginError::Network)?;
        self.preview = Some(p);
        self.preview_site = download::source(&item.key).map(|(site, _)| site);
        Ok(())
    }

    fn preview_file(&mut self) -> Result<Option<PathBuf>, PluginError> {
        let result = match self.preview.as_mut() {
            Some(p) => p.file(),
            None => Ok(None),
        };
        if let Some(site) = self.preview_site {
            match &result {
                Ok(Some(_)) => self.health.worked(site),
                Err(why) => self.failed(site, why),
                Ok(None) => {}
            }
        }
        result.map_err(PluginError::Network)
    }

    fn stop_preview(&mut self) {
        if let Some(p) = self.preview.take() {
            p.stop();
        }
    }

    fn status(&mut self) -> Option<StatusPage> {
        let state = self.tool.state();
        let broken = match &state {
            State::Ready(r) => self.health.line(&r.version),
            _ => None,
        };
        let mut page = page(&state, &self.deno.state(), broken, SystemTime::now());
        if !self.kind.wants_deno() {
            page.sections[0]
                .rows
                .retain(|(label, _)| !label.starts_with("Deno"));
        }
        page.sections[0]
            .rows
            .push((self.kind.plan().to_string(), premium_line(&self.premium)));
        Some(page)
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
        let p = page(&ready(false, None), &deno::State::Off, None, now);
        assert_eq!(p.line, "2025.11.12");
        let rows = &p.sections[0].rows;
        assert_eq!(rows[0], ("Version".into(), "2025.11.12".into()));
        assert_eq!(rows[1].1, "Daily · last checked 2 h ago");
        let own = page(&ready(true, None), &deno::State::Off, None, now);
        assert!(own.sections[0].rows[1].1.contains("not updated"));
    }

    #[test]
    fn says_how_premium_stands() {
        assert!(premium_line(&Ok(None)).contains("128 kbps"));
        assert!(premium_line(&Ok(Some("safari".into()))).contains("safari"));
        let mut y = YtDlp::new(Kind::YouTube);
        y.premium = premium::browser("netscape");
        assert!(y.line().contains("isn't a browser"));
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
        let y = YtDlp::new(Kind::YouTube);
        let q = SourceQuery {
            text: "Bonobo - Kerala".into(),
            ..Default::default()
        };
        assert_eq!(y.describe(&q), vec!["YouTube: Bonobo - Kerala"]);
        assert!(y.describe(&SourceQuery::default()).is_empty());
        let link = SourceQuery {
            text: "https://www.youtube.com/watch?v=x".into(),
            ..Default::default()
        };
        assert_eq!(
            y.describe(&link),
            vec!["YouTube link: the track, or every track of the playlist"]
        );
        let other = SourceQuery {
            text: "https://example.com/x".into(),
            ..Default::default()
        };
        assert_eq!(y.describe(&other), vec![Kind::YouTube.other_link()]);
        let soundcloud_link = SourceQuery {
            text: "https://soundcloud.com/a/b".into(),
            ..Default::default()
        };
        assert_eq!(
            y.describe(&soundcloud_link),
            vec![Kind::YouTube.other_link()]
        );
        let songs = SourceQuery {
            text: "Friday".into(),
            songs: vec!["A - B".into(), "C - D".into()],
            ..Default::default()
        };
        assert_eq!(
            y.describe(&songs),
            vec!["Each of the 2 songs of Friday, on YouTube"]
        );
        let sc = YtDlp::new(Kind::SoundCloud);
        assert_eq!(sc.describe(&q), vec!["SoundCloud: Bonobo - Kerala"]);
        let ht = SourceQuery {
            text: "https://hearthis.at/dj/mix/".into(),
            ..Default::default()
        };
        assert_eq!(
            sc.describe(&ht),
            vec!["hearthis.at link: the track, or every track of the playlist"]
        );
        assert_eq!(
            sc.describe(&songs),
            vec!["Each of the 2 songs of Friday, on SoundCloud"]
        );
    }

    #[test]
    fn soundcloud_has_no_deno_setting() {
        let keys = |k| {
            YtDlp::new(k)
                .settings()
                .iter()
                .map(|s| s.key)
                .collect::<Vec<_>>()
        };
        assert!(keys(Kind::YouTube).contains(&"deno"));
        assert!(!keys(Kind::SoundCloud).contains(&"deno"));
    }

    #[test]
    fn puts_both_sites_together() {
        use opendj_plugins::source::Found;
        let site = |items: usize, done: bool, error: Option<&str>| -> Shared {
            let s: Shared = Default::default();
            {
                let mut r = s.lock().unwrap();
                r.items = (0..items)
                    .map(|i| Found {
                        key: i.to_string(),
                        ..Default::default()
                    })
                    .collect();
                r.done = done;
                r.error = error.map(str::to_string);
            }
            s
        };
        let snap = snapshot(&[
            (Site::YouTube, site(3, true, None)),
            (Site::SoundCloud, site(1, false, None)),
        ]);
        assert_eq!(snap.items.len(), 4);
        assert!(!snap.done);
        assert_eq!(snap.note, "3 on YouTube");
        let snap = snapshot(&[
            (Site::YouTube, site(0, true, Some("bot check"))),
            (Site::SoundCloud, site(2, true, None)),
        ]);
        assert!(snap.done);
        assert_eq!(snap.note, "2 on SoundCloud · YouTube failed: bot check");
    }

    #[test]
    fn counts_songs_found() {
        use opendj_plugins::source::Found;
        let s: Shared = Default::default();
        {
            let mut r = s.lock().unwrap();
            r.songs = 3;
            r.looked_up = 2;
            let f = |g: &str| Found {
                group: g.into(),
                ..Default::default()
            };
            r.items = vec![f("1. A – B"), f("1. A – B"), f("2. C – D")];
        }
        assert_eq!(songs_snapshot(&s).note, "2 of 3 looked up");
        {
            let mut r = s.lock().unwrap();
            r.looked_up = 3;
            r.done = true;
        }
        let snap = songs_snapshot(&s);
        assert!(snap.done);
        assert_eq!(snap.note, "2 of 3 found");
        assert_eq!(snap.items.len(), 3);
    }
}
