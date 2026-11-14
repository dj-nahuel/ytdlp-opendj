#!/bin/sh
# Builds the yt-dlp add-on and puts it in openDJ's add-ons folder, where
# openDJ finds it the next time it starts. Your openDJ isn't rebuilt.
#
#   scripts/install.sh
#   OPENDJ_PLUGINS=/path/to/opendj/crates/plugins scripts/install.sh
#       # build against a local openDJ checkout instead of GitHub's main
#
# yt-dlp itself is fetched by the add-on the first time openDJ starts it.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
command -v cargo >/dev/null || { echo "Rust isn't installed: get it from https://rustup.rs" >&2; exit 1; }

if [ -n "${OPENDJ_CONFIG:-}" ]; then
  config=$OPENDJ_CONFIG
elif [ "$(uname -s)" = Darwin ]; then
  config="$HOME/Library/Application Support/openDJ"
else
  config="${XDG_CONFIG_HOME:-$HOME/.config}/opendj"
fi

cd "$here"
# iCloud Drive won't let cargo link its build scripts ("Operation not
# permitted"), so a checkout there builds in the user's cache folder instead.
case "$here" in
  *"/Mobile Documents/"*) : "${CARGO_TARGET_DIR:=$HOME/Library/Caches/ytdlp-opendj}" ;;
esac
target=${CARGO_TARGET_DIR:-target}
export CARGO_TARGET_DIR="$target"
if [ -n "${OPENDJ_PLUGINS:-}" ]; then
  cargo build --release --bin opendj-addon-ytdlp \
    --config "patch.\"https://github.com/openwave-studio/opendj\".opendj-plugins.path=\"$OPENDJ_PLUGINS\""
else
  # The add-on follows openDJ's main branch, which Cargo.lock would
  # otherwise keep at the commit of the first build.
  if [ -f Cargo.lock ]; then
    cargo update -p opendj-plugins || true
  fi
  cargo build --release --bin opendj-addon-ytdlp
fi

mkdir -p "$config/addons"
# Copy then rename, so an openDJ that's running never sees half a file.
cp "$target/release/opendj-addon-ytdlp" "$config/addons/.opendj-addon-ytdlp.new"
mv "$config/addons/.opendj-addon-ytdlp.new" "$config/addons/opendj-addon-ytdlp"
echo "Installed in $config/addons. Restart openDJ to load it."

