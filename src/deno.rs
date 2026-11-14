//! Deno, the JavaScript runtime yt-dlp needs for full YouTube support: it
//! solves YouTube's player challenges, without which some videos fail or
//! come only in formats openDJ can't play. Fetched from Deno's GitHub
//! releases like yt-dlp itself (see [`crate::tool`]), checked against the
//! release's SHA-256, and handed to yt-dlp with `--js-runtimes`.
//!
//! `OPENDJ_DENO_BIN` points the add-on at a Deno of the DJ's own instead.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::tool::{due, get_bytes, install, read_time, sha256_hex, write_time};

const LATEST: &str = "https://github.com/denoland/deno/releases/latest/download";
/// Deno's zip is about 45 MB.
const MAX_ZIP: u64 = 200 * 1024 * 1024;
/// The SHA-256 of the zip installed, to tell whether a newer one is out.
const SUM_FILE: &str = "deno.sha256";
const CHECKED_FILE: &str = "deno-checked";

/// Deno's release file for this computer, or `None` where it has none.
pub fn asset(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("macos", "aarch64") => "deno-aarch64-apple-darwin.zip",
        ("macos", "x86_64") => "deno-x86_64-apple-darwin.zip",
        ("windows", "x86_64") => "deno-x86_64-pc-windows-msvc.zip",
        ("linux", "x86_64") => "deno-x86_64-unknown-linux-gnu.zip",
        ("linux", "aarch64") => "deno-aarch64-unknown-linux-gnu.zip",
        _ => return None,
    })
}

/// The program's name inside the zip.
pub fn program_name() -> &'static str {
    if cfg!(windows) {
        "deno.exe"
    } else {
        "deno"
    }
}

/// The SHA-256 in a `.sha256sum` file, whatever its layout: the first run
/// of 64 hex digits.
pub fn sum_in(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_hexdigit())
        .find(|t| t.len() == 64)
        .map(str::to_ascii_lowercase)
}

/// The program out of Deno's zip.
pub fn unzip(zip: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))
        .map_err(|e| format!("Deno's zip didn't open: {e}"))?;
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("{name} isn't in Deno's zip"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Deno's zip didn't unpack: {e}"))?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not wanted (switched off), or nothing done yet.
    Off,
    Fetching,
    Ready(PathBuf),
    /// None for this computer, or it couldn't be fetched: yt-dlp runs
    /// without it.
    Failed(String),
}

/// Deno for the add-on, fetched on a thread of its own.
#[derive(Clone)]
pub struct Deno {
    state: Arc<Mutex<State>>,
    busy: Arc<Mutex<bool>>,
}

impl Default for Deno {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Off)),
            busy: Arc::new(Mutex::new(false)),
        }
    }
}

impl Deno {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// yt-dlp's arguments for using Deno, once it's there.
    pub fn args(&self) -> Vec<String> {
        match self.state() {
            State::Ready(path) => vec!["--js-runtimes".into(), format!("deno:{}", path.display())],
            _ => Vec::new(),
        }
    }

    /// Makes sure there's a Deno in `dir` when `wanted`, and a current one
    /// when `update` is on and a day has passed. Returns at once.
    pub fn ensure(&self, dir: &Path, wanted: bool, update: bool) {
        if !wanted {
            *self.state.lock().unwrap() = State::Off;
            return;
        }
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
            let state = run(&dir, update, &|s| *this.state.lock().unwrap() = s);
            *this.state.lock().unwrap() = state;
            *this.busy.lock().unwrap() = false;
        });
    }
}

fn run(dir: &Path, update: bool, show: &dyn Fn(State)) -> State {
    if let Some(own) = std::env::var_os("OPENDJ_DENO_BIN") {
        return State::Ready(PathBuf::from(own));
    }
    let Some(zip_name) = asset(std::env::consts::OS, std::env::consts::ARCH) else {
        return State::Failed(format!(
            "Deno has no build for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    };
    let path = dir.join("bin").join(program_name());
    let have = path.is_file();
    let checked = read_time(&dir.join(CHECKED_FILE));
    if have {
        show(State::Ready(path.clone()));
        if !update || !due(checked, SystemTime::now()) {
            return State::Ready(path);
        }
    } else {
        show(State::Fetching);
    }
    let result = (|| {
        let sums = get_bytes(&format!("{LATEST}/{zip_name}.sha256sum"), 4096)?;
        let want = sum_in(&String::from_utf8_lossy(&sums))
            .ok_or_else(|| format!("no SHA-256 in {zip_name}.sha256sum"))?;
        let installed = std::fs::read_to_string(dir.join(SUM_FILE)).unwrap_or_default();
        if have && installed.trim() == want {
            return Ok(());
        }
        let zip = get_bytes(&format!("{LATEST}/{zip_name}"), MAX_ZIP)?;
        if sha256_hex(&zip) != want {
            return Err(format!(
                "the downloaded {zip_name} doesn't match its SHA-256, so it isn't used"
            ));
        }
        install(&unzip(&zip, program_name())?, &path)?;
        let _ = std::fs::write(dir.join(SUM_FILE), &want);
        Ok(())
    })();
    match result {
        Ok(()) => {
            write_time(&dir.join(CHECKED_FILE), SystemTime::now());
            State::Ready(path)
        }
        // An older Deno still works.
        Err(_) if have => State::Ready(path),
        Err(why) => State::Failed(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn picks_the_build_for_each_computer() {
        assert_eq!(
            asset("macos", "aarch64"),
            Some("deno-aarch64-apple-darwin.zip")
        );
        assert_eq!(
            asset("windows", "x86_64"),
            Some("deno-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(
            asset("linux", "aarch64"),
            Some("deno-aarch64-unknown-linux-gnu.zip")
        );
        assert_eq!(asset("windows", "aarch64"), None);
    }

    #[test]
    fn reads_sums_in_any_layout() {
        let h = "AB".repeat(32);
        assert_eq!(sum_in(&format!("{h}  deno-x.zip\n")), Some("ab".repeat(32)));
        assert_eq!(
            sum_in(&format!("Algorithm Hash Path\nSHA256 {h} C:\\deno.zip")),
            Some("ab".repeat(32))
        );
        assert_eq!(sum_in("nothing here"), None);
    }

    #[test]
    fn unzips_the_program() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("deno", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"#!fake deno").unwrap();
            w.finish().unwrap();
        }
        let zip = buf.into_inner();
        assert_eq!(unzip(&zip, "deno").unwrap(), b"#!fake deno");
        assert!(unzip(&zip, "deno.exe").is_err());
        assert!(unzip(b"not a zip", "deno").is_err());
    }

    #[test]
    fn hands_deno_to_yt_dlp_once_ready() {
        let d = Deno::default();
        assert!(d.args().is_empty());
        *d.state.lock().unwrap() = State::Ready(PathBuf::from("/a/bin/deno"));
        assert_eq!(d.args(), ["--js-runtimes", "deno:/a/bin/deno"]);
        d.ensure(Path::new("/nowhere"), false, true);
        assert_eq!(d.state(), State::Off);
    }
}
