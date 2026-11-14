//! YouTube Premium sign-in, for YouTube's 256 kbps AAC (format 141)
//! instead of 128 kbps. yt-dlp reads the DJ's YouTube sign-in from the
//! browser they use YouTube in (`--cookies-from-browser`), so the add-on
//! never sees a password. Only YouTube downloads use it: searches and
//! previews stay signed out, so the account is used as little as possible.

use std::sync::Mutex;

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

/// Sets the browser YouTube downloads read the sign-in from.
pub fn set(browser: Option<String>) {
    *BROWSER.lock().unwrap() = browser;
}

pub fn signed_in() -> bool {
    BROWSER.lock().unwrap().is_some()
}

/// yt-dlp's arguments for a YouTube download.
pub fn args() -> Vec<String> {
    match BROWSER.lock().unwrap().as_ref() {
        Some(b) => vec!["--cookies-from-browser".into(), b.clone()],
        None => Vec::new(),
    }
}

/// The Downloads list's note for a finished YouTube download in `format_id`.
pub fn note(format_id: &str) -> Option<String> {
    let kbps = kbps(format_id)?;
    Some(match (kbps, signed_in()) {
        (256, _) => "256 kbps (Premium)".into(),
        (k, true) => format!("{k} kbps: YouTube didn't offer Premium's 256 kbps for this one"),
        (k, false) => format!("{k} kbps"),
    })
}

/// Why yt-dlp couldn't read the sign-in, in words, when that's what
/// `error` says.
pub fn explain(error: &str) -> Option<String> {
    let lower = error.to_ascii_lowercase();
    if !lower.contains("cookie") {
        return None;
    }
    let browser = BROWSER.lock().unwrap().clone().unwrap_or_default();
    let mut why = format!("Couldn't read your YouTube sign-in from {browser}.");
    if lower.contains("operation not permitted") || lower.contains("permission denied") {
        why.push_str(
            " On a Mac, Safari's sign-ins need Full Disk Access for openDJ \
             (System Settings → Privacy & Security).",
        );
    } else if lower.contains("could not find") {
        why.push_str(" Is it installed, and signed in to YouTube?");
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
    fn passes_the_sign_in_to_youtube_downloads() {
        set(None);
        assert!(args().is_empty());
        assert_eq!(note("140").as_deref(), Some("128 kbps"));
        set(Some("safari".into()));
        assert_eq!(args(), ["--cookies-from-browser", "safari"]);
        assert_eq!(note("141").as_deref(), Some("256 kbps (Premium)"));
        assert!(note("140").unwrap().contains("didn't offer"));
        assert!(explain("Operation not permitted: Cookies.binarycookies")
            .unwrap()
            .contains("Full Disk Access"));
        assert_eq!(
            explain("ERROR: could not find safari cookies database").as_deref(),
            Some("Couldn't read your YouTube sign-in from safari. Is it installed, and signed in to YouTube?")
        );
        assert_eq!(explain("Video unavailable"), None);
        set(None);
    }
}
