#!/bin/sh
# Builds Dev All from this checkout and installs it for the current user (macOS):
#   ~/Applications/devall.app  (shows in Launchpad and Spotlight).
# Run it again after any change to rebuild and replace the installed copy:
#   cd apps/desktop && npm run app:install
# If the installed app is open it is asked to quit and reopened afterwards.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
apps="$HOME/Applications"

echo "1/3  Building the app bundle..."
cd "$root/apps/desktop"
npx tauri build --bundles app

bundle=$(ls -d "$root"/target/release/bundle/macos/*.app | head -n 1)
name=$(basename "$bundle")

echo "2/3  Installing $name..."
was_running=0
if pgrep -f "$apps/$name/Contents/MacOS/" >/dev/null 2>&1; then
  was_running=1
  osascript -e "quit app \"$apps/$name\"" || true
  sleep 2
fi
mkdir -p "$apps"
rm -rf "$apps/$name"
cp -R "$bundle" "$apps/"

echo "3/3  Installed $apps/$name"
[ "$was_running" = 1 ] && open "$apps/$name"
exit 0
