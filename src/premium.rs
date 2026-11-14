//! YouTube Premium and SoundCloud Go+ sign-in, for 256 kbps AAC instead of
//! 128 kbps (YouTube's format 141, SoundCloud's high-quality AAC). yt-dlp
//! reads the DJ's sign-in from the browser they use those sites in
//! (`--cookies-from-browser`), so the add-on never sees a password. Only
//! downloads use it: searches and previews stay signed out, so the
//! accounts are used as little as possible.

use std::sync::Mutex;

use crate::download::Picked;
use crate::search::Site;

/// Browsers yt-dlp reads sign-ins from.
pub const BROWSERS: [&str; 9] = [
    "safari", "chrome", "firefox", "brave", "edge", "opera", "vivaldi", "chromium", "whale",
];

/// The browser named in Settings: `None` when empty, an error when yt-dlp
/// doesn't know it. A profile may follow a colon, as yt-dlp takes it:
/// `chrome:Profile 1`.
pub fn browser(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (name, profile) = match text.split_once(':') {
        Some((n, p)) => (n.trim().to_ascii_lowercase(), Some(p.trim())),
        None => (text.to_ascii_lowercase(), None),
    };
    if !BROWSERS.contains(&name.as_str()) {
        return Err(format!(
            "\"{text}\" isn't a browser yt-dlp can read a sign-in from. Type one of: {}.",
            BROWSERS.join(", ")
        ));
    }
    Ok(Some(match profile {
        Some(p) if !p.is_empty() => format!("{name}:{p}"),
        _ => name,
    }))
}

/// The bitrate of a YouTube audio format yt-dlp picked.
pub fn kbps(format_id: &str) -> Option<u32> {
    // A format id can carry a suffix, like "141-drc".
    match format_id.split('-').next()? {
        "141" => Some(256),
        "140" => Some(128),
        _ => None,
    }
}

static BROWSER: Mutex<Option<String>> = Mutex::new(None);

/// Sets the browser downloads read the sign-in from.
pub fn set(browser: Option<String>) {
    *BROWSER.lock().unwrap() = browser;
}

pub fn signed_in() -> bool {
    BROWSER.lock().unwrap().is_some()
}

/// yt-dlp's arguments for a download from `site`.
pub fn args(site: Site) -> Vec<String> {
    match (site, BROWSER.lock().unwrap().as_ref()) {
        (Site::YouTube | Site::SoundCloud, Some(b)) => {
            vec!["--cookies-from-browser".into(), b.clone()]
        }
        _ => Vec::new(),
    }
}

/// The Downloads list's note for a finished download from `site`.
pub fn note(site: Site, picked: &Picked) -> Option<String> {
    let signed_in = signed_in();
    match site {
        Site::YouTube => {
            let kbps = kbps(&picked.format)?;
            Some(match (kbps, signed_in) {
                (256, _) => "256 kbps (Premium)".into(),
                (k, true) => {
                    format!("{k} kbps: YouTube didn't offer Premium's 256 kbps for this one")
                }
                (k, false) => format!("{k} kbps"),
            })
        }
        // The artist's own file says nothing about a stream's bitrate.
        Site::SoundCloud if picked.format != "download" => {
            let kbps = picked.abr?.round() as u32;
            Some(match (kbps >= 200, signed_in) {
                (true, _) => format!("{kbps} kbps (Go+)"),
                (false, true) => {
                    format!("{kbps} kbps: SoundCloud didn't offer Go+ quality for this one")
                }
                (false, false) => format!("{kbps} kbps"),
            })
        }
        _ => None,
    }
}

/// Why yt-dlp couldn't read the sign-in, in words, when that's what
/// `error` says.
pub fn explain(error: &str) -> Option<String> {
    let lower = error.to_ascii_lowercase();
    if !lower.contains("cookie") {
        return None;
    }
    let browser = BROWSER.lock().unwrap().clone().unwrap_or_default();
    let mut why = format!("Couldn't read your sign-in from {browser}.");
    if lower.contains("operation not permitted") || lower.contains("permission denied") {
        why.push_str(
            " On a Mac, Safari's sign-ins need Full Disk Access for openDJ \
             (System Settings → Privacy & Security).",
        );
    } else if lower.contains("could not find") {
        why.push_str(" Is it installed, and signed in to YouTube or SoundCloud?");
    } else if lower.contains("decrypt") || lower.contains("keyring") || lower.contains("keychain") {
        why.push_str(" Allow openDJ when your computer asks for the browser's safe storage.");
    }
    Some(why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_browser_name() {
        assert_eq!(browser(""), Ok(None));
        assert_eq!(browser("  Safari "), Ok(Some("safari".into())));
        assert_eq!(
            browser("Chrome:Profile 1"),
            Ok(Some("chrome:Profile 1".into()))
        );
        assert_eq!(browser("firefox:"), Ok(Some("firefox".into())));
        assert!(browser("netscape").is_err());
    }

    #[test]
    fn tells_the_bitrate() {
        assert_eq!(kbps("141"), Some(256));
        assert_eq!(kbps("140-drc"), Some(128));
        assert_eq!(kbps("251"), None);
    }

    // One test for the global, so tests running at once don't race on it.
    #[test]
    fn passes_the_sign_in_to_downloads() {
        let p = |format: &str, abr: Option<f64>| Picked {
            format: format.into(),
            abr,
        };
        set(None);
        assert!(args(Site::YouTube).is_empty());
        assert_eq!(
            note(Site::YouTube, &p("140", None)).as_deref(),
            Some("128 kbps")
        );
        assert_eq!(
            note(Site::SoundCloud, &p("http_mp3_128", Some(128.0))).as_deref(),
            Some("128 kbps")
        );
        set(Some("safari".into()));
        assert_eq!(args(Site::YouTube), ["--cookies-from-browser", "safari"]);
        assert_eq!(args(Site::SoundCloud), ["--cookies-from-browser", "safari"]);
        assert!(args(Site::HearThis).is_empty());
        assert_eq!(
            note(Site::YouTube, &p("141", Some(256.0))).as_deref(),
            Some("256 kbps (Premium)")
        );
        assert!(note(Site::YouTube, &p("140", None))
            .unwrap()
            .contains("didn't offer"));
        assert_eq!(
            note(Site::SoundCloud, &p("hls_aac_256k", Some(256.0))).as_deref(),
            Some("256 kbps (Go+)")
        );
        assert!(note(Site::SoundCloud, &p("http_mp3_128", Some(128.0)))
            .unwrap()
            .contains("Go+ quality"));
        assert_eq!(note(Site::SoundCloud, &p("download", None)), None);
        assert!(explain("Operation not permitted: Cookies.binarycookies")
            .unwrap()
            .contains("Full Disk Access"));
        assert_eq!(
            explain("ERROR: could not find safari cookies database").as_deref(),
            Some(
                "Couldn't read your sign-in from safari. Is it installed, and signed in to \
                 YouTube or SoundCloud?"
            )
        );
        assert_eq!(explain("Video unavailable"), None);
        set(None);
    }
}
