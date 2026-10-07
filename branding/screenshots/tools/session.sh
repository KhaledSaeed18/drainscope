#!/bin/bash
# Runs a headless GNOME Shell with the packaged drainscope extension, the capture extension and
# the demo Monitor1 service on a scratch session bus, until the capture extension writes
# $W/done (or $W/stop appears). Run inside a toolbox (see capture.sh). Everything lives under
# $W: XDG directories, logs and the screenshots.
set -u
W=$1
SCHEME=${SCHEME:-default}
TOOLS=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$TOOLS/../../.." && pwd)
mkdir -p "$W"/{data/gnome-shell/extensions,config,cache,state,shots}
rm -f "$W/stop" "$W/done"
[ -d "$TOOLS/capture@drainscope" ] && cp -r "$TOOLS/capture@drainscope" "$W/data/gnome-shell/extensions/"
[ -f "$TOOLS/monitors.xml" ] && cp "$TOOLS/monitors.xml" "$W/config/monitors.xml"
export XDG_DATA_HOME=$W/data XDG_CONFIG_HOME=$W/config XDG_CACHE_HOME=$W/cache XDG_STATE_HOME=$W/state
# Host apps (Firefox, Showtime…) so their names and icons resolve as on a normal desktop.
export XDG_DATA_DIRS=/usr/local/share:/usr/share:/run/host/usr/share:/run/host/var/lib/flatpak/exports/share
# GLib hides entries whose Exec isn't on PATH, and these apps live on the host: copy the ones
# the demo data mentions, for their names and icons only.
mkdir -p "$W/data/applications"
for id in org.mozilla.firefox org.gnome.Showtime org.gnome.TextEditor org.gnome.Nautilus \
  libreoffice-writer org.gnome.Ptyxis org.gnome.Software; do
  for dir in /run/host/usr/share/applications /run/host/var/lib/flatpak/exports/share/applications; do
    [ -f "$dir/$id.desktop" ] || continue
    sed -e 's/^Exec=.*/Exec=true/' -e '/^TryExec=/d' -e '/^DBusActivatable=/d' "$dir/$id.desktop" \
      > "$W/data/applications/$id.desktop"
    break
  done
done
export CAPTURE_DIR=$W/shots CAPTURE_PLAN=${PLAN:?PLAN: the shot plan (JSON)}
exec dbus-run-session -- bash -c '
  gjs -m "'"$TOOLS"'/demo-monitor.js" "'"$REPO"'/data/dbus/interfaces/io.github.khaledsaeed18.Drainscope.Monitor1.xml" > "'"$W"'/demo.log" 2>&1 &
  until grep -q serving "'"$W"'/demo.log"; do sleep 0.2; done
  gsettings set org.gnome.shell disable-user-extensions false
  gsettings set org.gnome.shell enabled-extensions "[\"drainscope@khaledsaeed18.github.io\", \"capture@drainscope\"]"
  gsettings set org.gnome.shell welcome-dialog-last-shown-version "999"
  gsettings set org.gnome.desktop.interface accent-color teal
  gsettings set org.gnome.desktop.interface enable-hot-corners false
  gsettings set org.gnome.desktop.interface color-scheme "'"$SCHEME"'"
  wallpaper=file:///run/host/usr/share/backgrounds/gnome/blobs-$([ "'"$SCHEME"'" = prefer-dark ] && echo d || echo l).svg
  gsettings set org.gnome.desktop.background picture-uri "$wallpaper"
  gsettings set org.gnome.desktop.background picture-uri-dark "$wallpaper"
  gnome-shell --headless --wayland --no-x11 --virtual-monitor "${SIZE:-2880x1800}" > "'"$W"'/shell.log" 2>&1 &
  shell=$!
  echo "$DBUS_SESSION_BUS_ADDRESS" > "'"$W"'/bus"
  for _ in $(seq "${TIMEOUT:-240}"); do
    [ -e "'"$W"'/done" ] || [ -e "'"$W"'/stop" ] && break
    sleep 1
  done
  kill $shell; wait $shell 2>/dev/null
  kill %1 2>/dev/null
'
