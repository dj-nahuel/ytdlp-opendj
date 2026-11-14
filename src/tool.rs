//! The yt-dlp program itself: the add-on fetches the official build for
//! this computer from yt-dlp's GitHub releases, checks it against the
//! release's SHA-256 list, keeps it in the add-on's own folder, and looks
//! for a newer release once a day, since sites change and yt-dlp follows.
//!
//! `OPENDJ_YTDLP_BIN` points the add-on at a yt-dlp of the DJ's own
//! instead; that one is never replaced.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The newest release's files. No GitHub API: its hourly limit is low, and
/// a release's own SHA-256 list says whether the yt-dlp here is current.
const LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const SUMS: &str = "SHA2-256SUMS";
/// How often to look for a newer yt-dlp.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often a site's breakage may set off a check for a newer yt-dlp.
const FORCE_EVERY: Duration = Duration::from_secs(60 * 60);
/// Where the last check is remembered, in the add-on's folder.
const CHECKED_FILE: &str = "yt-dlp-checked";
/// yt-dlp builds are about 35 MB; anything far bigger isn't one.
const MAX_SIZE: u64 = 200 * 1024 * 1024;

/// The release file for this computer, or `None` where yt-dlp has none.
pub fn asset(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        // One universal build for Intel and Apple silicon.
        ("macos", _) => "yt-dlp_macos",
        ("windows", "x86_64") => "yt-dlp.exe",
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        ("windows", "x86") => "yt-dlp_x86.exe",
        ("linux", "x86_64") => "yt-dlp_linux",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        _ => return None,
    })
}

pub fn this_asset() -> Option<&'static str> {
    asset(std::env::consts::OS, std::env::consts::ARCH)
}

/// The SHA-256 of `name` in a release's `SHA2-256SUMS` file
/// (`<hash>  <name>` lines), lower case.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether it's time to look for a newer yt-dlp, given when it was last
/// done.
pub fn due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        None => true,
        // A clock that went backwards: check again.
        Some(t) => now.duration_since(t).map_or(true, |d| d >= CHECK_EVERY),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Nothing done yet.
    Idle,
    /// Fetching yt-dlp for the first time.
    Fetching,
    Ready(Ready),
    /// It couldn't be fetched and there's none to use.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    pub path: PathBuf,
    /// As `yt-dlp --version` says, e.g. "2025.11.12".
    pub version: String,
    /// The DJ's own yt-dlp, from `OPENDJ_YTDLP_BIN`: never updated.
    pub own: bool,
    /// When a newer release was last looked for.
    pub checked: Option<SystemTime>,
    /// Why the last update failed; the yt-dlp there is still used.
    pub update_error: Option<String>,
}

/// yt-dlp for the add-on, fetched and updated on a thread of its own so
/// openDJ never waits for it.
#[derive(Clone)]
pub struct Tool {
    state: Arc<Mutex<State>>,
    busy: Arc<Mutex<bool>>,
    /// When a check was last forced by [`Tool::check_now`].
    forced: Arc<Mutex<Option<std::time::Instant>>>,
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
            busy: Arc::new(Mutex::new(false)),
            forced: Arc::new(Mutex::new(None)),
        }
    }
}

impl Tool {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// The program to run, once there is one.
    pub fn program(&self) -> Option<PathBuf> {
        match &*self.state.lock().unwrap() {
            State::Ready(r) => Some(r.path.clone()),
            _ => None,
        }
    }

    /// Makes sure there's a yt-dlp in `dir`, and a current one when
    /// `update` is on and a day has passed. Returns at once; the work runs
    /// on a thread, one at a time.
    pub fn ensure(&self, dir: &Path, update: bool) {
        self.start(dir, update, false);
    }

    /// Looks for a newer yt-dlp now, whatever the day's check said: for
    /// when a site seems to have broken this one. At most once an hour.
    pub fn check_now(&self, dir: &Path) {
        {
            let mut last = self.forced.lock().unwrap();
            if last.is_some_and(|t| t.elapsed() < FORCE_EVERY) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        self.start(dir, true, true);
    }

    fn start(&self, dir: &Path, update: bool, force: bool) {
        {
            let mut busy = self.busy.lock().unwrap();
            if *busy {
                return;
            }
            *busy = true;
        }
        let this = self.clone();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            this.run(&dir, update, force);
            *this.busy.lock().unwrap() = false;
        });
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }

    fn run(&self, dir: &Path, update: bool, force: bool) {
        if let Some(own) = std::env::var_os("OPENDJ_YTDLP_BIN") {
            let path = PathBuf::from(own);
            self.set(match version(&path) {
                Ok(version) => State::Ready(Ready {
                    path,
                    version,
                    own: true,
                    checked: None,
                    update_error: None,
                }),
                Err(why) => State::Failed(format!("OPENDJ_YTDLP_BIN: {why}")),
            });
            return;
        }
        let Some(name) = this_asset() else {
            self.set(State::Failed(format!(
                "yt-dlp has no build for {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
            return;
        };
        let path = dir.join("bin").join(name);
        let have = path.is_file().then(|| version(&path).ok()).flatten();
        let checked = read_checked(dir);
        if let Some(v) = &have {
            self.set(State::Ready(Ready {
                path: path.clone(),
                version: v.clone(),
                own: false,
                checked,
                update_error: None,
            }));
            if !update || !(force || due(checked, SystemTime::now())) {
                return;
            }
        } else {
            self.set(State::Fetching);
        }
        let result = latest_sums().and_then(|sums| {
            let want =
                sum_for(&sums, name).ok_or_else(|| format!("{name} isn't in yt-dlp's {SUMS}"))?;
            let current = have.is_some()
                && std::fs::read(&path).is_ok_and(|bytes| sha256_hex(&bytes) == want);
            if !current {
                fetch(name, &want, &path)?;
            }
            version(&path)
        });
        let now = SystemTime::now();
        match result {
            Ok(version) => {
                write_checked(dir, now);
                self.set(State::Ready(Ready {
                    path,
                    version,
                    own: false,
                    checked: Some(now),
                    update_error: None,
                }));
            }
            Err(why) => match have {
                // Keep using the one there; try again next start.
                Some(version) => self.set(State::Ready(Ready {
                    path,
                    version,
                    own: false,
                    checked,
                    update_error: Some(why),
                })),
                None => self.set(State::Failed(why)),
            },
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(opendj_plugins::USER_AGENT)
        .build()
        .into()
}

/// The newest release's SHA-256 list.
fn latest_sums() -> Result<String, String> {
    let bytes = get_bytes(&format!("{LATEST}/{SUMS}"), 1024 * 1024)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = agent()
        .get(url)
        .call()
        .map_err(|e| format!("couldn't download {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download of {url} broke off: {e}"))?;
    Ok(bytes)
}

/// Downloads the newest release's file `name` to `path`, if its SHA-256
/// is `want`. Written beside it first, then moved, so a yt-dlp that's
/// running is never half replaced.
fn fetch(name: &str, want: &str, path: &Path) -> Result<(), String> {
    let bytes = get_bytes(&format!("{LATEST}/{name}"), MAX_SIZE)?;
    if sha256_hex(&bytes) != want {
        // Or a release came out between the two downloads: tried again
        // next time.
        return Err(format!(
            "the downloaded {name} doesn't match its SHA-256, so it isn't used"
        ));
    }
    install(&bytes, path)
}

/// Writes a program to `path`: beside it first, then moved, so one that's
/// running is never half replaced.
pub(crate) fn install(bytes: &[u8], path: &Path) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.new"));
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What `yt-dlp --version` prints.
pub fn version(path: &Path) -> Result<String, String> {
    let mut cmd = Command::new(path);
    cmd.arg("--version");
    hide_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("couldn't run {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("{} didn't start: {err}", path.display()));
    }
    Ok(text)
}

/// Arguments given to every yt-dlp run, such as where Deno is.
static EXTRA_ARGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sets the arguments every later yt-dlp run gets first.
pub fn set_extra_args(args: Vec<String>) {
    *EXTRA_ARGS.lock().unwrap() = args;
}

/// A yt-dlp run of `program`: no console window, and the add-on's own
/// arguments (see [`set_extra_args`]) before the caller's.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(EXTRA_ARGS.lock().unwrap().iter());
    hide_window(&mut cmd);
    cmd
}

/// No console window flashing up on Windows when yt-dlp runs.
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn read_checked(dir: &Path) -> Option<SystemTime> {
    read_time(&dir.join(CHECKED_FILE))
}

pub(crate) fn read_time(file: &Path) -> Option<SystemTime> {
    let secs: u64 = std::fs::read_to_string(file).ok()?.trim().parse().ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

fn write_checked(dir: &Path, when: SystemTime) {
    write_time(&dir.join(CHECKED_FILE), when);
}

pub(crate) fn write_time(file: &Path, when: SystemTime) {
    if let Ok(d) = when.duration_since(SystemTime::UNIX_EPOCH) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, d.as_secs().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_build_for_each_computer() {
        assert_eq!(asset("macos", "aarch64"), Some("yt-dlp_macos"));
        assert_eq!(asset("macos", "x86_64"), Some("yt-dlp_macos"));
        assert_eq!(asset("windows", "x86_64"), Some("yt-dlp.exe"));
        assert_eq!(asset("windows", "aarch64"), Some("yt-dlp_arm64.exe"));
        assert_eq!(asset("linux", "x86_64"), Some("yt-dlp_linux"));
        assert_eq!(asset("linux", "aarch64"), Some("yt-dlp_linux_aarch64"));
        assert_eq!(asset("freebsd", "x86_64"), None);
    }

    #[test]
    fn reads_the_sums_file() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  yt-dlp\n{b}  yt-dlp_macos\n{a} *yt-dlp.exe\nnot a line\n");
        assert_eq!(sum_for(&sums, "yt-dlp_macos"), Some("b".repeat(64)));
        assert_eq!(sum_for(&sums, "yt-dlp.exe"), Some(a.clone()));
        assert_eq!(sum_for(&sums, "yt-dlp_linux"), None);
        assert_eq!(sum_for("short  yt-dlp_linux", "yt-dlp_linux"), None);
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn checks_once_a_day() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(due(None, now));
        assert!(!due(Some(now - Duration::from_secs(3600)), now));
        assert!(due(Some(now - CHECK_EVERY), now));
        // Clock moved back.
        assert!(due(Some(now + Duration::from_secs(60)), now));
    }

    #[test]
    fn remembers_the_last_check() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-test-{}", std::process::id()));
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        write_checked(&dir, when);
        assert_eq!(read_checked(&dir), Some(when));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fetches the real yt-dlp from GitHub: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn fetches_yt_dlp_from_github() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-live-{}", std::process::id()));
        let tool = Tool::default();
        tool.run(&dir, true, false);
        let State::Ready(r) = tool.state() else {
            panic!("{:?}", tool.state());
        };
        assert!(!r.version.is_empty());
        assert!(r.checked.is_some());
        // A second run within the day doesn't fetch again.
        tool.run(&dir, true, false);
        assert!(matches!(tool.state(), State::Ready(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
