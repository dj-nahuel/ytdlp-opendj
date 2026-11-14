//! Downloading what the DJ picked: one yt-dlp per track, three at a time,
//! taking audio openDJ plays as it is (AAC or MP3, or a SoundCloud track's
//! original file when the artist offers it), so no ffmpeg is needed.
//! Finished files are named "Artist - Title" and tagged by the add-on.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use opendj_plugins::source::{DownloadUpdate, Found, Progress};

use crate::search::Site;
use crate::tool::hide_window;

/// Downloads running at once.
pub const AT_ONCE: usize = 3;

/// Formats openDJ plays without converting, best first. YouTube's AAC
/// stream (format 140, 128 kbps; 141 at 256 kbps with Premium) before
/// anything else: openDJ can't play Opus.
const YOUTUBE_FORMATS: &str = "141/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]/140";
/// SoundCloud: the artist's own file when they allow downloads, else MP3,
/// else AAC.
const SOUNDCLOUD_FORMATS: &str =
    "download/bestaudio[ext=mp3]/bestaudio[acodec=mp3]/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]";

/// What a progress line from yt-dlp starts with.
const PROG: &str = "ODJPROG ";
/// What the finished file's line starts with.
const FILE: &str = "ODJFILE ";

/// Where a result's key points: its site and page.
pub fn source(key: &str) -> Option<(Site, String)> {
    let (tag, rest) = key.split_once(':')?;
    match tag {
        "yt" if !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
        {
            Some((
                Site::YouTube,
                format!("https://www.youtube.com/watch?v={rest}"),
            ))
        }
        "sc" if rest.starts_with("https://soundcloud.com/") => {
            Some((Site::SoundCloud, rest.to_string()))
        }
        _ => None,
    }
}

/// A name that's safe as a file name everywhere: no slashes or characters
/// Windows refuses, no leading dots, not too long.
pub fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let mut out: String = trimmed.chars().take(120).collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        "Untitled".into()
    } else {
        out
    }
}

/// "Artist - Title", or the title alone.
pub fn base_name(item: &Found) -> String {
    let title = if item.title.trim().is_empty() {
        "Untitled"
    } else {
        item.title.trim()
    };
    if item.artist.trim().is_empty() {
        safe(title)
    } else {
        safe(&format!("{} - {}", item.artist.trim(), title))
    }
}

/// `base` in `dir`, with " (2)", " (3)"… when a file of that name, with
/// any extension, is already there.
pub fn free_base(dir: &Path, base: &str) -> String {
    let taken = |b: &str| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                Path::new(&*name)
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy() == b)
            })
        })
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|b| !taken(b))
        .unwrap()
}

/// yt-dlp's arguments for downloading `url` from `site` to `dir/base.ext`.
pub fn args(site: Site, url: &str, dir: &Path, base: &str) -> Vec<String> {
    let template = dir.join(format!("{}.%(ext)s", base.replace('%', "%%")));
    vec![
        "--ignore-config".into(),
        "--no-warnings".into(),
        "--no-playlist".into(),
        "--no-mtime".into(),
        "--newline".into(),
        "-f".into(),
        match site {
            Site::YouTube => YOUTUBE_FORMATS,
            Site::SoundCloud => SOUNDCLOUD_FORMATS,
        }
        .into(),
        "--progress-template".into(),
        format!(
            "download:{PROG}%(progress.downloaded_bytes)s %(progress.total_bytes)s \
             %(progress.total_bytes_estimate)s %(progress.speed)s"
        ),
        "--print".into(),
        format!("after_move:{FILE}%(filepath)s"),
        // --print alone would skip the download.
        "--no-simulate".into(),
        "-o".into(),
        template.to_string_lossy().into_owned(),
        url.to_string(),
    ]
}

/// A progress line: how far (0..1) and how fast (bytes a second).
pub fn parse_progress(line: &str) -> Option<(f32, Option<f64>)> {
    let rest = line.trim().strip_prefix(PROG.trim_end())?.trim();
    let num = |s: Option<&str>| s.and_then(|s| s.parse::<f64>().ok());
    let mut parts = rest.split_whitespace();
    let got = num(parts.next())?;
    let total = num(parts.next());
    let estimate = num(parts.next());
    let speed = num(parts.next());
    let total = total.or(estimate).filter(|t| *t > 0.0)?;
    Some(((got / total).clamp(0.0, 1.0) as f32, speed))
}

/// "1.2 MB/s", "340 KB/s".
pub fn speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1_000_000.0)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1_000.0)
    }
}

/// Why a download failed, in words for the Downloads list.
pub fn explain(stderr: &str) -> String {
    let last = crate::search::last_error(stderr);
    if last.contains("Requested format is not available") {
        "No audio openDJ can play without converting (only Opus): not downloaded".into()
    } else {
        last
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Queued,
    Running(f32),
    Done(PathBuf),
    Failed(String),
}

impl Step {
    fn progress(&self) -> Progress {
        match self {
            Step::Queued => Progress::Queued,
            Step::Running(f) => Progress::Running(*f),
            Step::Done(p) => Progress::Done(vec![p.clone()]),
            Step::Failed(why) => Progress::Failed(why.clone()),
        }
    }
}

#[derive(Default)]
struct Live {
    step: Option<Step>,
    speed: Option<f64>,
    child: Option<Child>,
    cancelled: bool,
}

struct Job {
    key: String,
    item: Found,
    site: Site,
    url: String,
    dir: PathBuf,
    live: Arc<Mutex<Live>>,
    /// What was last told to openDJ.
    told: Option<Progress>,
    started: bool,
}

/// All downloads, queued, running and finished.
#[derive(Default)]
pub struct Downloads {
    jobs: Vec<Job>,
    queue: VecDeque<String>,
}

impl Downloads {
    /// Queues `items` for downloading into `dir`.
    pub fn add(&mut self, items: &[Found], dir: &Path) -> Result<(), String> {
        for item in items {
            let (site, url) =
                source(&item.key).ok_or_else(|| format!("can't download {}", item.key))?;
            self.jobs.retain(|j| j.key != item.key);
            self.queue.retain(|k| k != &item.key);
            self.jobs.push(Job {
                key: item.key.clone(),
                item: item.clone(),
                site,
                url,
                dir: dir.to_path_buf(),
                live: Arc::new(Mutex::new(Live {
                    step: Some(Step::Queued),
                    ..Live::default()
                })),
                told: None,
                started: false,
            });
            self.queue.push_back(item.key.clone());
        }
        Ok(())
    }

    /// Starts queued downloads while fewer than [`AT_ONCE`] run, with
    /// yt-dlp at `program`.
    pub fn pump(&mut self, program: Option<&Path>) {
        let Some(program) = program else {
            return;
        };
        let running = self
            .jobs
            .iter()
            .filter(|j| j.started && matches!(j.live.lock().unwrap().step, Some(Step::Running(_))))
            .count();
        for _ in running..AT_ONCE {
            let Some(key) = self.queue.pop_front() else {
                break;
            };
            if let Some(job) = self.jobs.iter_mut().find(|j| j.key == key) {
                job.started = true;
                start(job, program);
            }
        }
    }

    /// What changed since last asked.
    pub fn progress(&mut self) -> Vec<DownloadUpdate> {
        let mut out = Vec::new();
        for job in &mut self.jobs {
            let Some(step) = job.live.lock().unwrap().step.clone() else {
                continue;
            };
            let now = step.progress();
            if job.told.as_ref() != Some(&now) {
                job.told = Some(now.clone());
                out.push(DownloadUpdate {
                    key: job.key.clone(),
                    progress: now,
                });
            }
        }
        out
    }

    /// Speed of each download running.
    pub fn notes(&self) -> Vec<(String, String)> {
        self.jobs
            .iter()
            .filter_map(|j| {
                let live = j.live.lock().unwrap();
                match (&live.step, live.speed) {
                    (Some(Step::Running(_)), Some(s)) if s > 0.0 => Some((j.key.clone(), speed(s))),
                    _ => None,
                }
            })
            .collect()
    }

    /// Stops a queued or running download.
    pub fn cancel(&mut self, key: &str) -> Result<(), String> {
        let job = self
            .jobs
            .iter()
            .find(|j| j.key == key)
            .ok_or_else(|| "no such download".to_string())?;
        self.queue.retain(|k| k != key);
        let mut live = job.live.lock().unwrap();
        live.cancelled = true;
        if let Some(child) = live.child.as_mut() {
            let _ = child.kill();
        }
        live.step = Some(Step::Failed("Cancelled".into()));
        Ok(())
    }

    /// Tries a failed download again.
    pub fn retry(&mut self, key: &str) -> Result<(), String> {
        let job = self
            .jobs
            .iter()
            .find(|j| j.key == key)
            .ok_or_else(|| "no such download".to_string())?;
        let (item, dir) = (job.item.clone(), job.dir.clone());
        self.add(&[item], &dir)
    }
}

fn start(job: &mut Job, program: &Path) {
    let live = job.live.clone();
    if let Err(e) = std::fs::create_dir_all(&job.dir) {
        live.lock().unwrap().step = Some(Step::Failed(format!("{}: {e}", job.dir.display())));
        return;
    }
    let base = free_base(&job.dir, &base_name(&job.item));
    let mut cmd = Command::new(program);
    cmd.args(args(job.site, &job.url, &job.dir, &base))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            live.lock().unwrap().step = Some(Step::Failed(format!("couldn't start yt-dlp: {e}")));
            return;
        }
    };
    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    {
        let mut l = live.lock().unwrap();
        l.step = Some(Step::Running(0.0));
        l.child = Some(child);
    }
    let (item, site, url) = (job.item.clone(), job.site, job.url.clone());
    std::thread::spawn(move || {
        let err_reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = e.read_to_string(&mut text);
            }
            text
        });
        let mut file: Option<PathBuf> = None;
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(path) = line.strip_prefix(FILE) {
                    file = Some(PathBuf::from(path.trim()));
                } else if let Some((f, s)) = parse_progress(&line) {
                    let mut l = live.lock().unwrap();
                    l.step = Some(Step::Running(f));
                    l.speed = s;
                }
            }
        }
        let child = live.lock().unwrap().child.take();
        let ok = child
            .and_then(|mut c| c.wait().ok())
            .is_some_and(|s| s.success());
        let err = err_reader.join().unwrap_or_default();
        let step = match file {
            _ if live.lock().unwrap().cancelled => Step::Failed("Cancelled".into()),
            Some(path) if ok && path.is_file() => {
                crate::tags::write(&path, &item, site, &url);
                Step::Done(path)
            }
            _ => Step::Failed(explain(&err)),
        };
        let mut l = live.lock().unwrap();
        l.step = Some(step);
        l.speed = None;
    });
}

/// Waits up to `limit` for every download to finish; for tests.
#[cfg(test)]
fn settle(d: &mut Downloads, program: &Path, limit: std::time::Duration) {
    let end = std::time::Instant::now() + limit;
    while std::time::Instant::now() < end {
        d.pump(Some(program));
        let busy = d.jobs.iter().any(|j| {
            matches!(
                j.live.lock().unwrap().step,
                Some(Step::Queued) | Some(Step::Running(_))
            )
        });
        if !busy {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn item(key: &str, artist: &str, title: &str) -> Found {
        Found {
            key: key.into(),
            artist: artist.into(),
            title: title.into(),
            ..Found::default()
        }
    }

    #[test]
    fn reads_keys_back() {
        assert_eq!(
            source("yt:abc_-1"),
            Some((
                Site::YouTube,
                "https://www.youtube.com/watch?v=abc_-1".into()
            ))
        );
        assert_eq!(
            source("sc:https://soundcloud.com/dj/edit"),
            Some((Site::SoundCloud, "https://soundcloud.com/dj/edit".into()))
        );
        assert_eq!(source("yt:a&b"), None);
        assert_eq!(source("sc:https://evil.example/x"), None);
        assert_eq!(source("nope"), None);
    }

    #[test]
    fn makes_safe_names() {
        assert_eq!(safe("AC/DC: Back?"), "AC_DC_ Back_");
        assert_eq!(safe("..hidden"), "hidden");
        assert_eq!(safe("trailing. "), "trailing");
        assert_eq!(safe("   "), "Untitled");
        assert_eq!(safe(&"x".repeat(300)).len(), 120);
        assert_eq!(base_name(&item("k", "Bonobo", "Kerala")), "Bonobo - Kerala");
        assert_eq!(base_name(&item("k", "", "Kerala")), "Kerala");
    }

    #[test]
    fn never_overwrites() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_base(&dir, "A - B"), "A - B");
        std::fs::write(dir.join("A - B.m4a"), b"").unwrap();
        assert_eq!(free_base(&dir, "A - B"), "A - B (2)");
        std::fs::write(dir.join("A - B (2).mp3"), b"").unwrap();
        assert_eq!(free_base(&dir, "A - B"), "A - B (3)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_the_download() {
        let a = args(
            Site::YouTube,
            "https://www.youtube.com/watch?v=x",
            Path::new("/m"),
            "100% - Hit",
        );
        let i = a.iter().position(|s| s == "-f").unwrap();
        assert!(a[i + 1].starts_with("141/bestaudio[ext=m4a]"));
        assert!(!a[i + 1].contains("opus"));
        let o = a.iter().position(|s| s == "-o").unwrap();
        assert_eq!(
            a[o + 1],
            Path::new("/m")
                .join("100%% - Hit.%(ext)s")
                .to_string_lossy()
        );
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/watch?v=x");
        let sc = args(Site::SoundCloud, "u", Path::new("/m"), "b");
        assert!(sc.iter().any(|s| s.starts_with("download/")));
    }

    #[test]
    fn reads_progress_lines() {
        assert_eq!(
            parse_progress("ODJPROG 50 200 NA 1000.5"),
            Some((0.25, Some(1000.5)))
        );
        assert_eq!(parse_progress("ODJPROG 50 NA 100 NA"), Some((0.5, None)));
        assert_eq!(parse_progress("ODJPROG 50 NA NA NA"), None);
        assert_eq!(parse_progress("[download] 50%"), None);
        assert_eq!(speed(1_250_000.0), "1.2 MB/s");
        assert_eq!(speed(340_000.0), "340 KB/s");
    }

    #[test]
    fn explains_failures() {
        assert!(explain(
            "ERROR: [youtube] x: Requested format is not available. Use --list-formats"
        )
        .contains("Opus"));
        assert_eq!(explain("ERROR: Video unavailable"), "Video unavailable");
    }

    /// A stand-in yt-dlp writing a file where `-o` says, with progress.
    #[cfg(unix)]
    fn fake(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let fake = dir.join("yt-dlp");
        std::fs::write(&fake, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        fake
    }

    #[cfg(unix)]
    #[test]
    fn downloads_and_reports() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-dl-{}", std::process::id()));
        let out = dir.join("out");
        std::fs::create_dir_all(&dir).unwrap();
        // The template is the argument after -o; the file gets .mp3.
        let program = fake(
            &dir,
            r#"while [ "$1" != "-o" ]; do shift; done; t="$2"; f="${t%.%(ext)s}.mp3"
echo "ODJPROG 10 100 NA 2000000"; echo "ODJPROG 100 100 NA NA"
printf 'not really mp3' > "$f"; echo "ODJFILE $f""#,
        );
        let mut d = Downloads::default();
        d.add(
            &[
                item("yt:abc", "Bonobo", "Kerala"),
                item("yt:def", "", "Two"),
            ],
            &out,
        )
        .unwrap();
        assert_eq!(d.progress().len(), 2);
        settle(&mut d, &program, Duration::from_secs(5));
        let done: Vec<_> = d.progress();
        assert_eq!(done.len(), 2);
        assert_eq!(
            done[0].progress,
            Progress::Done(vec![out.join("Bonobo - Kerala.mp3")])
        );
        assert!(out.join("Two.mp3").is_file());
        // Nothing new since.
        assert!(d.progress().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn fails_with_yt_dlps_reason_and_retries() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = fake(
            &dir,
            "echo 'ERROR: [youtube] abc: Video unavailable' >&2; exit 1",
        );
        let mut d = Downloads::default();
        d.add(&[item("yt:abc", "A", "B")], &dir.join("out"))
            .unwrap();
        settle(&mut d, &program, Duration::from_secs(5));
        let p = d.progress();
        assert_eq!(
            p.last().unwrap().progress,
            Progress::Failed("[youtube] abc: Video unavailable".into())
        );
        d.retry("yt:abc").unwrap();
        assert_eq!(d.progress()[0].progress, Progress::Queued);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cancels() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = fake(&dir, "echo 'ODJPROG 1 100 NA NA'; exec sleep 30");
        let mut d = Downloads::default();
        d.add(&[item("yt:abc", "A", "B")], &dir.join("out"))
            .unwrap();
        d.pump(Some(&program));
        std::thread::sleep(Duration::from_millis(100));
        d.cancel("yt:abc").unwrap();
        settle(&mut d, &program, Duration::from_secs(5));
        assert_eq!(
            d.progress().last().unwrap().progress,
            Progress::Failed("Cancelled".into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
