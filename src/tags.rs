//! Tags for a finished download: artist, title, cover, and a comment that
//! says where it came from, so the collection knows a YouTube rip from a
//! bought file. Written by the add-on itself, since yt-dlp needs ffmpeg
//! for that.

use std::path::Path;
use std::time::Duration;

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use opendj_plugins::source::Found;

use crate::search::Site;

/// Covers bigger than this aren't embedded.
const MAX_COVER: u64 = 5 * 1024 * 1024;

/// The comment written into every download, e.g. "Downloaded from YouTube
/// with yt-dlp: https://… (lossy)". A SoundCloud artist's own WAV, FLAC
/// or AIFF isn't marked lossy.
pub fn comment(site: Site, url: &str, path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let lossless = matches!(ext.as_str(), "wav" | "flac" | "aif" | "aiff");
    format!(
        "Downloaded from {} with yt-dlp: {url}{}",
        site.name(),
        if lossless { "" } else { " (lossy)" }
    )
}

/// The picture's type from its first bytes, if it's one we embed.
pub fn mime(bytes: &[u8]) -> Option<MimeType> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(MimeType::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(MimeType::Png)
    } else {
        None
    }
}

/// Writes tags into `path`. A file that can't be tagged is kept as it is.
pub fn write(path: &Path, item: &Found, site: Site, url: &str) {
    let cover = fetch_cover(&item.cover_url);
    if let Err(e) = try_write(path, item, site, url, cover) {
        eprintln!("ytdlp-opendj: couldn't tag {}: {e}", path.display());
    }
}

fn try_write(
    path: &Path,
    item: &Found,
    site: Site,
    url: &str,
    cover: Option<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = lofty::read_from_path(path)?;
    let kind = file.primary_tag_type();
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or("no tag")?;
    // SoundCloud originals may carry the artist's own tags: keep those.
    if tag.artist().is_none() && !item.artist.is_empty() {
        tag.set_artist(item.artist.clone());
    }
    if tag.title().is_none() && !item.title.is_empty() {
        tag.set_title(item.title.clone());
    }
    tag.insert_text(ItemKey::Comment, comment(site, url, path));
    if let Some(bytes) = cover {
        if tag.pictures().is_empty() {
            if let Some(m) = mime(&bytes) {
                tag.push_picture(
                    Picture::unchecked(bytes)
                        .pic_type(PictureType::CoverFront)
                        .mime_type(m)
                        .build(),
                );
            }
        }
    }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

fn fetch_cover(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("https://") {
        return None;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(opendj_plugins::USER_AGENT)
        .build()
        .into();
    let mut response = agent.get(url).call().ok()?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_COVER)
        .read_to_vec()
        .ok()?;
    mime(&bytes).map(|_| bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_it_came_from() {
        assert_eq!(
            comment(
                Site::SoundCloud,
                "https://soundcloud.com/a/b",
                Path::new("x.mp3")
            ),
            "Downloaded from SoundCloud with yt-dlp: https://soundcloud.com/a/b (lossy)"
        );
        assert_eq!(
            comment(Site::SoundCloud, "u", Path::new("x.WAV")),
            "Downloaded from SoundCloud with yt-dlp: u"
        );
    }

    #[test]
    fn knows_pictures() {
        assert!(matches!(
            mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(MimeType::Jpeg)
        ));
        assert!(matches!(
            mime(b"\x89PNG\r\n\x1a\nrest"),
            Some(MimeType::Png)
        ));
        assert!(mime(b"<html>").is_none());
    }

    #[test]
    fn leaves_files_it_cant_tag() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.mp3");
        std::fs::write(&path, b"not really mp3").unwrap();
        write(&path, &Found::default(), Site::YouTube, "u");
        assert_eq!(std::fs::read(&path).unwrap(), b"not really mp3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second of 8-bit mono silence as a WAV file.
    fn wav() -> Vec<u8> {
        let data = vec![128u8; 8000];
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend(&(36 + data.len() as u32).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(&16u32.to_le_bytes());
        v.extend(&1u16.to_le_bytes()); // PCM
        v.extend(&1u16.to_le_bytes()); // mono
        v.extend(&8000u32.to_le_bytes());
        v.extend(&8000u32.to_le_bytes());
        v.extend(&1u16.to_le_bytes());
        v.extend(&8u16.to_le_bytes());
        v.extend(b"data");
        v.extend(&(data.len() as u32).to_le_bytes());
        v.extend(data);
        v
    }

    #[test]
    fn tags_a_file() {
        let dir = std::env::temp_dir().join(format!("ytdlp-opendj-tagged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Bonobo - Kerala.wav");
        std::fs::write(&path, wav()).unwrap();
        let item = Found {
            artist: "Bonobo".into(),
            title: "Kerala".into(),
            ..Found::default()
        };
        try_write(
            &path,
            &item,
            Site::SoundCloud,
            "https://soundcloud.com/a/b",
            None,
        )
        .unwrap();
        let file = lofty::read_from_path(&path).unwrap();
        let tag = file.primary_tag().unwrap();
        assert_eq!(tag.artist().as_deref(), Some("Bonobo"));
        assert_eq!(tag.title().as_deref(), Some("Kerala"));
        assert_eq!(
            tag.comment().as_deref(),
            Some("Downloaded from SoundCloud with yt-dlp: https://soundcloud.com/a/b")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
