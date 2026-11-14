//! The SoundCloud source (with hearthis.at) as a program of its own, beside
//! the YouTube one: see `main.rs`.

fn main() {
    use opendj_addon_ytdlp::{Kind, YtDlp};
    opendj_plugins::remote::serve(Box::new(YtDlp::new(Kind::SoundCloud)));
}
