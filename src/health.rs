//! Whether a site seems to have broken yt-dlp: YouTube changes its player
//! every few weeks, and until yt-dlp catches up, searches and downloads
//! fail with the same few errors. The add-on says so in plain words instead
//! of a raw error on every row, and looks for a newer yt-dlp straight away.

use std::time::SystemTime;

use crate::search::Site;

/// yt-dlp's words when a site has changed under it, rather than one video
/// being gone.
const BROKEN: [&str; 9] = [
    "Sign in to confirm you",
    "nsig extraction failed",
    "n challenge",
    "Unable to extract",
    "Failed to extract",
    "Precondition check failed",
    "HTTP Error 403",
    "player response",
    "JS challenge",
];

/// Whether yt-dlp's `error` says the site broke it.
pub fn looks_broken(error: &str) -> bool {
    BROKEN.iter().any(|b| error.contains(b))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Broken {
    pub site: Site,
    pub error: String,
    pub since: SystemTime,
}

/// The sites that seem broken now.
#[derive(Default)]
pub struct Health {
    broken: Vec<Broken>,
}

impl Health {
    /// Counts a failure on `site`. Returns whether it newly looks broken.
    pub fn failed(&mut self, site: Site, error: &str) -> bool {
        if !looks_broken(error) || self.broken.iter().any(|b| b.site == site) {
            return false;
        }
        self.broken.push(Broken {
            site,
            error: error.to_string(),
            since: SystemTime::now(),
        });
        true
    }

    /// `site` worked: it isn't broken.
    pub fn worked(&mut self, site: Site) {
        self.broken.retain(|b| b.site != site);
    }

    pub fn broken(&self) -> &[Broken] {
        &self.broken
    }

    /// One line for Settings and the panel, if anything's broken.
    pub fn line(&self, version: &str) -> Option<String> {
        let names: Vec<&str> = self.broken.iter().map(|b| b.site.name()).collect();
        if names.is_empty() {
            return None;
        }
        Some(format!(
            "{} isn't working with yt-dlp {version} right now. A newer yt-dlp usually fixes \
             it within days; the add-on looks for one every hour until then.",
            names.join(" and ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_breakage_from_a_missing_video() {
        assert!(looks_broken(
            "[youtube] abc: Sign in to confirm you’re not a bot. Use --cookies"
        ));
        assert!(looks_broken(
            "[youtube] abc: nsig extraction failed: You may experience throttling"
        ));
        assert!(looks_broken(
            "Unable to download API page: HTTP Error 403: Forbidden"
        ));
        assert!(!looks_broken("[youtube] abc: Video unavailable"));
        assert!(!looks_broken("[youtube] abc: Private video"));
    }

    #[test]
    fn tracks_broken_sites() {
        let mut h = Health::default();
        assert!(!h.failed(Site::YouTube, "Video unavailable"));
        assert_eq!(h.line("2025.11.12"), None);
        assert!(h.failed(Site::YouTube, "Sign in to confirm you're not a bot"));
        assert!(!h.failed(Site::YouTube, "Sign in to confirm you're not a bot"));
        assert!(h
            .line("2025.11.12")
            .unwrap()
            .starts_with("YouTube isn't working with yt-dlp 2025.11.12 right now"));
        h.worked(Site::SoundCloud);
        assert_eq!(h.broken().len(), 1);
        h.worked(Site::YouTube);
        assert_eq!(h.line("x"), None);
    }
}
