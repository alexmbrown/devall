#!/bin/sh
# Builds Dev All from this checkout and installs it for the current user (macOS):
#   ~/Applications/devall.app  (shows in Launchpad and Spotlight), registered with LaunchServices so
#   devall: links (browser, chat, other editors) open it — the bundle declares the scheme
#   (CFBundleURLTypes, from plugins.deep-link in tauri.conf.json).
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

# devall: links go to a bundle LaunchServices knows: register the installed copy and forget the build
# output, so links never open a stale bundle in target/.
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
if [ -x "$lsregister" ]; then
  "$lsregister" -u "$bundle" >/dev/null 2>&1 || true
  "$lsregister" -f "$apps/$name" >/dev/null 2>&1 || true
fi

echo "3/3  Installed $apps/$name (devall: links open it)"
[ "$was_running" = 1 ] && open "$apps/$name"
exit 0
