//! The yt-dlp add-on as a program of its own: openDJ starts it from its
//! add-ons folder and talks to it over stdin and stdout (see
//! `opendj_plugins::remote`). Install it with `scripts/install.sh`.

fn main() {
    opendj_plugins::remote::serve(Box::new(opendj_addon_ytdlp::YtDlp::new()));
}
