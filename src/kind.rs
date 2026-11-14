//! The add-on is two sources in openDJ's rail, built from this one crate:
//! YouTube (with YouTube Music), and SoundCloud (with hearthis.at, the same
//! kind of DJ uploads). Each is its own program with its own settings,
//! yt-dlp and Downloads, so the DJ can order them, or switch one off,
//! without the other.

use opendj_plugins::source::SourceInfo;
use opendj_plugins::Manifest;

use crate::search::Site;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    YouTube,
    SoundCloud,
}

const YOUTUBE: Manifest = Manifest {
    // Kept from when the add-on was one source, so its settings stay.
    id: "ytdlp",
    name: "YouTube",
    hosts: &[],
    downloads: true,
};

const SOUNDCLOUD: Manifest = Manifest {
    id: "ytdlp-soundcloud",
    name: "SoundCloud DL",
    hosts: &[],
    downloads: true,
};

impl Kind {
    pub fn manifest(self) -> &'static Manifest {
        match self {
            Kind::YouTube => &YOUTUBE,
            Kind::SoundCloud => &SOUNDCLOUD,
        }
    }

    pub fn info(self) -> SourceInfo {
        match self {
            Kind::YouTube => SourceInfo {
                name: "YouTube",
                icon_svg: include_bytes!("../icon.svg"),
                hint: "Artist - Title, or a YouTube or YouTube Music link",
            },
            Kind::SoundCloud => SourceInfo {
                // openDJ's rail already has a SoundCloud (browsing, coming
                // later), and hides sources by name: a name of its own keeps
                // hiding one from hiding the other.
                name: "SoundCloud DL",
                icon_svg: include_bytes!("../icon-soundcloud.svg"),
                hint: "Artist - Title, or a SoundCloud or hearthis.at link",
            },
        }
    }

    /// The sites a typed search goes to.
    pub fn searched(self) -> &'static [Site] {
        match self {
            Kind::YouTube => &[Site::YouTube],
            Kind::SoundCloud => &[Site::SoundCloud],
        }
    }

    /// Whether links and results from `site` are this source's.
    pub fn owns(self, site: Site) -> bool {
        match self {
            Kind::YouTube => site == Site::YouTube,
            Kind::SoundCloud => matches!(site, Site::SoundCloud | Site::HearThis),
        }
    }

    /// Hosts whose links, anywhere in openDJ, can be sent here.
    pub fn hosts(self) -> &'static [&'static str] {
        match self {
            Kind::YouTube => &[
                "www.youtube.com",
                "youtube.com",
                "music.youtube.com",
                "youtu.be",
            ],
            Kind::SoundCloud => &["soundcloud.com", "hearthis.at"],
        }
    }

    /// What's said for a link that isn't this source's.
    pub fn other_link(self) -> &'static str {
        match self {
            Kind::YouTube => {
                "Only YouTube and YouTube Music links are read here. SoundCloud and \
                 hearthis.at links go in the SoundCloud source."
            }
            Kind::SoundCloud => {
                "Only SoundCloud and hearthis.at links are read here. YouTube links go in \
                 the YouTube source."
            }
        }
    }

    /// Whether yt-dlp needs Deno for this source (YouTube's player checks).
    pub fn wants_deno(self) -> bool {
        self == Kind::YouTube
    }

    /// The site's own name, for sentences.
    pub fn site_name(self) -> &'static str {
        self.searched()[0].name()
    }

    /// The paid plan whose sign-in gives 256 kbps.
    pub fn plan(self) -> &'static str {
        match self {
            Kind::YouTube => "YouTube Premium",
            Kind::SoundCloud => "SoundCloud Go+",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_sites() {
        assert!(Kind::YouTube.owns(Site::YouTube));
        assert!(!Kind::YouTube.owns(Site::HearThis));
        assert!(Kind::SoundCloud.owns(Site::HearThis));
        assert!(!Kind::SoundCloud.owns(Site::YouTube));
        assert_ne!(Kind::YouTube.manifest().id, Kind::SoundCloud.manifest().id);
        assert_eq!(Kind::SoundCloud.searched(), [Site::SoundCloud]);
    }
}
