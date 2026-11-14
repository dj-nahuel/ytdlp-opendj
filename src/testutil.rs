//! Stand-in yt-dlp scripts for tests.

use std::path::{Path, PathBuf};

/// Writes a shell script standing in for yt-dlp at `dir/yt-dlp`, and waits
/// until it can run: while another test thread is starting a program, the
/// new file can briefly refuse to start ("Text file busy").
pub fn fake(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let fake = dir.join("yt-dlp");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\n[ -n \"$ODJ_FAKE_PROBE\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(&fake)
            .env("ODJ_FAKE_PROBE", "1")
            .status()
        {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => break,
        }
    }
    fake
}
