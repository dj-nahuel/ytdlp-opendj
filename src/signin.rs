//! "Use my browser's sign-in": the add-on finds the DJ's default browser,
//! checks that it's signed in to the site (yt-dlp reads its cookies, as
//! downloads will), and says so. When it isn't, the site's sign-in page
//! opens in that browser and the add-on checks again by itself; when
//! Safari's cookies are walled off, the Full Disk Access page opens.
//!
//! Google stopped yt-dlp signing in to YouTube by itself in 2024, so the
//! browser's own sign-in is the only way in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::kind::Kind;

/// How often a sign-in not found yet is looked for again.
const RECHECK: Duration = Duration::from_secs(15);
/// How long to keep looking after opening the sign-in page.
const KEEP_LOOKING: Duration = Duration::from_secs(10 * 60);

/// yt-dlp's name for a browser, from its macOS bundle id, Windows ProgId
/// or Linux desktop file.
pub fn yt_dlp_name(id: &str) -> Option<&'static str> {
    let id = id.trim().to_ascii_lowercase();
    let has = |s: &str| id.contains(s);
    Some(if has("safari") {
        "safari"
    } else if has("brave") {
        "brave"
    } else if has("edge") || has("msedge") {
        "edge"
    } else if has("vivaldi") {
        "vivaldi"
    } else if has("opera") {
        "opera"
    } else if has("firefox") || has("mozilla") {
        "firefox"
    } else if has("chromium") {
        "chromium"
    } else if has("chrome") {
        "chrome"
    } else {
        return None;
    })
}

/// The https handler in the output of
/// `defaults read com.apple.LaunchServices/com.apple.launchservices.secure LSHandlers`.
pub fn mac_handler(plist: &str) -> Option<String> {
    plist.split('}').find_map(|block| {
        if !block.contains("LSHandlerURLScheme = https;") {
            return None;
        }
        let at = block.find("LSHandlerRoleAll = ")? + "LSHandlerRoleAll = ".len();
        let rest = &block[at..];
        let id = rest.split(';').next()?.trim().trim_matches('"');
        Some(id.to_string())
    })
}

/// The ProgId in `reg query ...\https\UserChoice /v ProgId`.
pub fn windows_prog_id(out: &str) -> Option<String> {
    out.lines()
        .find(|l| l.contains("ProgId"))
        .and_then(|l| l.split_whitespace().last())
        .map(str::to_string)
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    crate::tool::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The default browser, by yt-dlp's name for it. Safari when macOS was
/// never told another.
pub fn default_browser() -> Option<&'static str> {
    if let Ok(own) = std::env::var("OPENDJ_YTDLP_BROWSER") {
        return BROWSERS_LEAKED.iter().find(|b| **b == own.trim()).copied();
    }
    if cfg!(target_os = "macos") {
        let plist = run(
            "defaults",
            &[
                "read",
                "com.apple.LaunchServices/com.apple.launchservices.secure",
                "LSHandlers",
            ],
        )
        .unwrap_or_default();
        match mac_handler(&plist) {
            Some(id) => yt_dlp_name(&id),
            None => Some("safari"),
        }
    } else if cfg!(windows) {
        let out = run(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
                "/v",
                "ProgId",
            ],
        )?;
        yt_dlp_name(&windows_prog_id(&out)?)
    } else {
        yt_dlp_name(&run("xdg-settings", &["get", "default-web-browser"])?)
    }
}

/// Names `OPENDJ_YTDLP_BROWSER` may give.
const BROWSERS_LEAKED: [&str; 9] = crate::premium::BROWSERS;

/// Opens `url` in the default browser.
pub fn open_url(url: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    crate::tool::hide_window(&mut cmd);
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).spawn();
}

/// Where the DJ signs in to `kind`'s site.
pub fn sign_in_page(kind: Kind) -> &'static str {
    match kind {
        Kind::YouTube => {
            "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F"
        }
        Kind::SoundCloud => "https://soundcloud.com/signin",
    }
}

/// macOS's Full Disk Access page.
const FULL_DISK_ACCESS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// Whether a Netscape cookie file holds a sign-in to `kind`'s site.
pub fn signed_in(cookies: &str, kind: Kind) -> bool {
    let (domain, names): (&str, &[&str]) = match kind {
        Kind::YouTube => (
            "youtube.com",
            &["LOGIN_INFO", "SAPISID", "__Secure-3PAPISID"],
        ),
        Kind::SoundCloud => ("soundcloud.com", &["oauth_token"]),
    };
    cookies.lines().any(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        f.len() >= 7 && f[0].ends_with(domain) && names.contains(&f[5]) && !f[6].is_empty()
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Off,
    Checking(String),
    SignedIn(String),
    /// The browser has no sign-in to the site (yet).
    NotSignedIn(String),
    /// The browser's sign-ins couldn't be read.
    Failed(String),
}

impl State {
    /// The browser yt-dlp should read sign-ins from, if any.
    pub fn browser(&self) -> Option<&str> {
        match self {
            State::SignedIn(b) => Some(b),
            _ => None,
        }
    }

    /// A line for Settings and the panel.
    pub fn line(&self, kind: Kind) -> Option<String> {
        let site = kind.site_name();
        let browser = |b: &str| title(b);
        Some(match self {
            State::Off => return None,
            State::Checking(b) => format!("Looking for your {site} sign-in in {}…", browser(b)),
            State::SignedIn(b) => format!("Signed in to {site} through {}", browser(b)),
            State::NotSignedIn(b) => format!(
                "Not signed in to {site} in {}: sign in on the page that opened, and this \
                 updates by itself",
                browser(b)
            ),
            State::Failed(why) => why.clone(),
        })
    }
}

/// "Safari", "Chrome".
pub fn title(browser: &str) -> String {
    let mut c = browser.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The browser sign-in, checked on a thread of its own.
#[derive(Clone)]
pub struct SignIn {
    state: Arc<Mutex<State>>,
    /// Bumped on each start, so an old check stops.
    run: Arc<Mutex<u64>>,
}

impl Default for SignIn {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Off)),
            run: Arc::new(Mutex::new(0)),
        }
    }
}

impl SignIn {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }

    pub fn off(&self) {
        *self.run.lock().unwrap() += 1;
        *self.state.lock().unwrap() = State::Off;
    }

    /// Looks for `kind`'s sign-in in `browser` with yt-dlp at `program`,
    /// keeping temporary files in `dir`. Opens the sign-in page (or Full
    /// Disk Access) when that's what's missing, and looks again for a
    /// while. Returns at once.
    pub fn start(&self, program: PathBuf, browser: String, kind: Kind, dir: PathBuf) {
        let me = {
            let mut r = self.run.lock().unwrap();
            *r += 1;
            *r
        };
        *self.state.lock().unwrap() = State::Checking(browser.clone());
        let this = self.clone();
        std::thread::spawn(move || {
            let mut opened = None::<Instant>;
            loop {
                let state = check(&program, &browser, kind, &dir);
                if *this.run.lock().unwrap() != me {
                    return;
                }
                *this.state.lock().unwrap() = state.clone();
                match state {
                    State::NotSignedIn(_) => {
                        let since = *opened.get_or_insert_with(|| {
                            open_url(sign_in_page(kind));
                            Instant::now()
                        });
                        if since.elapsed() > KEEP_LOOKING {
                            return;
                        }
                    }
                    State::Failed(ref why) if why.contains("Full Disk Access") => {
                        let since = *opened.get_or_insert_with(|| {
                            if cfg!(target_os = "macos") {
                                open_url(FULL_DISK_ACCESS);
                            }
                            Instant::now()
                        });
                        if since.elapsed() > KEEP_LOOKING {
                            return;
                        }
                    }
                    _ => return,
                }
                std::thread::sleep(RECHECK);
            }
        });
    }
}

/// One look for the sign-in: yt-dlp reads the browser's cookies for a
/// one-result search, and saves them to a file the add-on reads.
pub fn check(program: &Path, browser: &str, kind: Kind, dir: &Path) -> State {
    let _ = std::fs::create_dir_all(dir);
    let file = dir.join("signin-cookies.txt");
    let _ = std::fs::remove_file(&file);
    let probe = match kind {
        Kind::YouTube => "ytsearch1:opendj",
        Kind::SoundCloud => "scsearch1:opendj",
    };
    let mut cmd = Command::new(program);
    cmd.args([
        "--ignore-config",
        "--no-warnings",
        "--flat-playlist",
        "--simulate",
        "--cookies-from-browser",
        browser,
        "--cookies",
    ])
    .arg(&file)
    .arg(probe)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    crate::tool::hide_window(&mut cmd);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return State::Failed(format!("couldn't start yt-dlp: {e}")),
    };
    let cookies = std::fs::read_to_string(&file).unwrap_or_default();
    // The file holds every cookie of the browser: don't leave it lying.
    let _ = std::fs::remove_file(&file);
    if signed_in(&cookies, kind) {
        return State::SignedIn(browser.to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = crate::search::last_error(&err);
    let lower = last.to_ascii_lowercase();
    if lower.contains("operation not permitted") || lower.contains("permission denied") {
        return State::Failed(format!(
            "{} needs Full Disk Access for openDJ to read your sign-in: switch openDJ on in \
             the page that opened, then restart openDJ",
            title(browser)
        ));
    }
    if lower.contains("cookie") || lower.contains("keyring") || lower.contains("decrypt") {
        return State::Failed(format!(
            "Couldn't read {}'s sign-ins: {last}",
            title(browser)
        ));
    }
    State::NotSignedIn(browser.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_default_browser() {
        assert_eq!(yt_dlp_name("com.apple.safari"), Some("safari"));
        assert_eq!(yt_dlp_name("com.google.chrome"), Some("chrome"));
        assert_eq!(yt_dlp_name("com.brave.browser"), Some("brave"));
        assert_eq!(yt_dlp_name("MSEdgeHTM"), Some("edge"));
        assert_eq!(yt_dlp_name("ChromeHTML"), Some("chrome"));
        assert_eq!(yt_dlp_name("FirefoxURL-308046B0AF4A39CB"), Some("firefox"));
        assert_eq!(yt_dlp_name("firefox.desktop"), Some("firefox"));
        assert_eq!(yt_dlp_name("org.chromium.Chromium"), Some("chromium"));
        assert_eq!(yt_dlp_name("com.kagi.kagimacOS"), None);
    }

    #[test]
    fn reads_the_mac_https_handler() {
        let plist = r#"(
        {
        LSHandlerContentType = "public.html";
        LSHandlerRoleAll = "com.apple.safari";
    },
        {
        LSHandlerPreferredVersions =         {
            LSHandlerRoleAll = "-";
        };
        LSHandlerRoleAll = "com.google.chrome";
        LSHandlerURLScheme = https;
    }
)"#;
        // The nested block's own "}" splits it; the scheme line still
        // follows the bundle id in the same piece.
        let found = mac_handler(plist);
        assert!(found.is_some());
        assert_eq!(yt_dlp_name(&found.unwrap()), Some("chrome"));
        assert_eq!(mac_handler("()"), None);
        let simple =
            "{\n LSHandlerRoleAll = \"com.brave.browser\";\n LSHandlerURLScheme = https;\n}";
        assert_eq!(mac_handler(simple).as_deref(), Some("com.brave.browser"));
    }

    #[test]
    fn reads_the_windows_prog_id() {
        let out = "\r\nHKEY_CURRENT_USER\\...\\UserChoice\r\n    ProgId    REG_SZ    BraveHTML\r\n";
        assert_eq!(windows_prog_id(out).as_deref(), Some("BraveHTML"));
    }

    #[test]
    fn finds_a_sign_in_in_cookies() {
        let yt = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\tabc\n";
        assert!(signed_in(yt, Kind::YouTube));
        assert!(!signed_in(yt, Kind::SoundCloud));
        let sc = ".soundcloud.com\tTRUE\t/\tTRUE\t0\toauth_token\t2-1\n";
        assert!(signed_in(sc, Kind::SoundCloud));
        let empty = ".youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\t\n";
        assert!(!signed_in(empty, Kind::YouTube));
    }

    #[test]
    fn says_how_it_stands() {
        assert_eq!(State::Off.line(Kind::YouTube), None);
        assert_eq!(
            State::SignedIn("safari".into())
                .line(Kind::YouTube)
                .unwrap(),
            "Signed in to YouTube through Safari"
        );
        assert_eq!(State::SignedIn("safari".into()).browser(), Some("safari"));
        assert_eq!(State::NotSignedIn("chrome".into()).browser(), None);
    }

    #[cfg(unix)]
    #[test]
    fn checks_with_yt_dlp() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-signin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Writes a signed-in cookie file to the path after --cookies.
        let yes = crate::testutil::fake(
            &dir,
            r#"while [ "$1" != "--cookies" ]; do shift; done
printf '.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tx\n' > "$2""#,
        );
        assert_eq!(
            check(&yes, "safari", Kind::YouTube, &dir),
            State::SignedIn("safari".into())
        );
        assert!(!dir.join("signin-cookies.txt").exists());
        let walled = dir.join("walled");
        std::fs::create_dir_all(&walled).unwrap();
        let no = crate::testutil::fake(
            &walled,
            "echo 'ERROR: [Errno 1] Operation not permitted: Cookies.binarycookies' >&2; exit 1",
        );
        let State::Failed(why) = check(&no, "safari", Kind::YouTube, &dir) else {
            panic!("should fail")
        };
        assert!(why.contains("Full Disk Access"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
