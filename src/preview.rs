//! Hearing a result before downloading it: yt-dlp writes the audio
//! straight into one file in the add-on's folder (no `.part`), front to
//! back, and openDJ plays that file as it grows.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use crate::download::source;
use crate::search::Site;

/// The preview file's name, before yt-dlp adds the extension.
const NAME: &str = "preview";

/// Formats for hearing: streams that play from their first bytes. Never a
/// SoundCloud or hearthis.at original, which may be a large WAV.
fn formats(site: Site) -> &'static str {
    match site {
        Site::YouTube => "140/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]",
        Site::SoundCloud => {
            "bestaudio[ext=mp3]/bestaudio[acodec=mp3]/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]"
        }
        Site::HearThis => "mp3/bestaudio[acodec=mp3]",
    }
}

/// yt-dlp's arguments for hearing `url` from `site`, written into `dir`.
pub fn args(site: Site, url: &str, dir: &Path) -> Vec<String> {
    vec![
        "--ignore-config".into(),
        "--no-warnings".into(),
        "--quiet".into(),
        "--no-playlist".into(),
        // Straight into the file openDJ reads, never resumed from an older
        // preview.
        "--no-part".into(),
        "--no-continue".into(),
        "-f".into(),
        formats(site).into(),
        "-o".into(),
        dir.join(format!("{NAME}.%(ext)s"))
            .to_string_lossy()
            .into_owned(),
        url.to_string(),
    ]
}

/// The preview file in `dir`, once its first bytes are there.
pub fn find(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let path = e.path();
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        let ext = path.extension()?.to_string_lossy().into_owned();
        // yt-dlp's own scratch files (".ytdl", ".part-Frag1") aren't it.
        let audio = matches!(ext.as_str(), "m4a" | "mp3" | "mp4" | "aac");
        (stem == NAME && audio && e.metadata().is_ok_and(|m| m.len() > 0)).then_some(path)
    })
}

/// Removes every preview file from `dir`.
pub fn clear(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(NAME) {
                // Still open in openDJ on Windows: it goes next time.
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// One preview being fetched.
pub struct Preview {
    child: Child,
    dir: PathBuf,
}

impl Preview {
    /// Starts fetching result `key` with yt-dlp at `program` into `dir`.
    pub fn start(program: &Path, key: &str, dir: &Path) -> Result<Preview, String> {
        let (site, url) = source(key).ok_or_else(|| format!("can't play {key}"))?;
        clear(dir);
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut cmd = crate::tool::command(program);
        cmd.args(args(site, &url, dir))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| format!("couldn't start yt-dlp: {e}"))?;
        Ok(Preview {
            child,
            dir: dir.to_path_buf(),
        })
    }

    /// The file once bytes arrive; an error once yt-dlp gave up with none.
    pub fn file(&mut self) -> Result<Option<PathBuf>, String> {
        if let Some(path) = find(&self.dir) {
            return Ok(Some(path));
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(e) = self.child.stderr.as_mut() {
                    let _ = e.read_to_string(&mut err);
                }
                // A last look: it may have finished between the two.
                if let Some(path) = find(&self.dir) {
                    return Ok(Some(path));
                }
                Err(if status.success() {
                    "Nothing came to play".into()
                } else {
                    crate::download::explain(&err)
                })
            }
            _ => Ok(None),
        }
    }

    /// Stops fetching and forgets what came.
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        clear(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn streams_straight_into_one_file() {
        let a = args(
            Site::SoundCloud,
            "https://soundcloud.com/a/b",
            Path::new("/p"),
        );
        assert!(a.contains(&"--no-part".to_string()));
        let f = a.iter().position(|s| s == "-f").unwrap();
        assert!(!a[f + 1].contains("download"));
        let o = a.iter().position(|s| s == "-o").unwrap();
        assert_eq!(
            a[o + 1],
            Path::new("/p").join("preview.%(ext)s").to_string_lossy()
        );
        assert!(formats(Site::YouTube).starts_with("140"));
    }

    #[test]
    fn finds_only_audio_that_has_started() {
        let dir = temp("find");
        assert_eq!(find(&dir), None);
        std::fs::write(dir.join("preview.m4a"), b"").unwrap();
        assert_eq!(find(&dir), None);
        std::fs::write(dir.join("preview.m4a.ytdl"), b"x").unwrap();
        std::fs::write(dir.join("preview.m4a"), b"abc").unwrap();
        assert_eq!(find(&dir), Some(dir.join("preview.m4a")));
        clear(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    use crate::testutil::fake;

    fn wait(p: &mut Preview) -> Result<Option<PathBuf>, String> {
        for _ in 0..300 {
            match p.file() {
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                other => return other,
            }
        }
        Ok(None)
    }

    #[cfg(unix)]
    #[test]
    fn hands_over_the_growing_file_and_stops() {
        let dir = temp("preview");
        let out = dir.join("p");
        let program = fake(
            &dir,
            r#"while [ "$1" != "-o" ]; do shift; done; f="${2%.%(ext)s}.m4a"
printf 'abc' > "$f"; exec sleep 30"#,
        );
        let mut p = Preview::start(&program, "yt:abc", &out).unwrap();
        assert_eq!(wait(&mut p), Ok(Some(out.join("preview.m4a"))));
        p.stop();
        assert_eq!(find(&out), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn says_why_nothing_came() {
        let dir = temp("nopreview");
        let program = fake(
            &dir,
            "echo 'ERROR: [youtube] abc: Video unavailable' >&2; exit 1",
        );
        let mut p = Preview::start(&program, "yt:abc", &dir.join("p")).unwrap();
        assert_eq!(wait(&mut p), Err("[youtube] abc: Video unavailable".into()));
        assert!(Preview::start(&program, "nope", &dir.join("p")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
