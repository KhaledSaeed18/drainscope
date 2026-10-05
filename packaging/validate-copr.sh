#!/bin/bash
# Validates the COPR packages of a release inside a toolbox of the Fedora version to test:
#   toolbox create --release NN drainscope-fNN
#   toolbox run -c drainscope-fNN sudo dnf install -y gnome-shell mutter-devkit dbus-daemon dnf5-plugins
#   toolbox run -c drainscope-fNN packaging/validate-copr.sh X.Y.Z
# Installs (or upgrades to) the release from COPR, checks versions, signatures and files, tests
# the database lock with the packaged daemon, and runs the packaged extension in a nested
# GNOME Shell. Everything runs on scratch XDG directories, so the real database is untouched.
# Set DRAINSCOPE_SEED to a copy of a database so the nested tile shows real rows.
# Don't run two of these at once: nested shells share the crash-guard file in /run/user/UID.
set -u
command -v dbus-run-session >/dev/null || { echo "dbus-run-session missing: sudo dnf install dbus-daemon"; exit 1; }
V=$1
S=$(mktemp -d /tmp/drainscope-validate.XXXX)
UUID=drainscope@khaledsaeed18.github.io
PKGS="drainscope drainscope-sampler drainscope-probe drainscope-selinux drainscope-app gnome-shell-extension-drainscope"
echo "== $(cat /etc/fedora-release)"
sudo dnf copr enable -y khaledsaeed18/drainscope >/dev/null 2>&1
sudo dnf clean expire-cache >/dev/null 2>&1
if rpm -q drainscope >/dev/null; then sudo dnf upgrade --refresh -y -q $PKGS >/dev/null 2>&1; fi
sudo dnf install --refresh -y -q $PKGS >/dev/null 2>&1
echo "== packages"
for p in $PKGS; do rpm -q $p --qf '%{NAME}-%{VERSION}-%{RELEASE}  %{SIGPGP:pgpsig}\n' | sed 's/, [A-Z][a-z][a-z] .* Key ID/ key/'; done
bad=0; for p in $PKGS; do rpm -q $p --qf '%{VERSION}\n' | grep -qx "$V" || bad=1; done
echo "version $V on all packages: $([ $bad = 0 ] && echo yes || echo NO)"
echo "== rpm -V (file integrity; blank = clean)"; rpm -V $PKGS | grep -v '^.......T\.' | head
echo "== unit"; grep RestartPreventExitStatus /usr/lib/systemd/user/drainscope.service
echo "== extension metadata"; grep -E 'shell-version|version-name' /usr/share/gnome-shell/extensions/$UUID/metadata.json
echo "== lock test with /usr/bin/drainscope-daemon"
export XDG_STATE_HOME=$S/state
dbus-run-session -- bash -c "timeout 8 /usr/bin/drainscope-daemon" >$S/d1.log 2>&1 &
sleep 3
dbus-run-session -- bash -c 'timeout 8 /usr/bin/drainscope-daemon; echo "second daemon exit=$?"' 2>&1 | grep -E "another|exit=" | sed 's/^.*Z //' | cut -c1-90
wait
grep -o "serving.*Monitor" $S/d1.log
rm -f $S/state/drainscope/drainscope.db*; [ -n "${DRAINSCOPE_SEED:-}" ] && cp "$DRAINSCOPE_SEED" $S/state/drainscope/drainscope.db
echo "== nested $(gnome-shell --version), system-installed extension"
[ -e /run/user/$(id -u)/gnome-shell-disable-extensions ] && { echo "crash-guard file present, aborting"; exit 1; }
export XDG_DATA_HOME=$S/data XDG_CONFIG_HOME=$S/config XDG_CACHE_HOME=$S/cache
mkdir -p $XDG_DATA_HOME $XDG_CONFIG_HOME $XDG_CACHE_HOME
dbus-run-session -- bash -c '
  gsettings set org.gnome.shell disable-user-extensions false
  gsettings set org.gnome.shell enabled-extensions "[\"'$UUID'\"]"
  gnome-shell --devkit --wayland > "'$S'/shell.log" 2>&1 &
  pid=$!
  sleep 65
  gnome-extensions info '$UUID' | grep -E "Path|Version|State"
  busctl --user call io.github.khaledsaeed18.Drainscope.Monitor /io/github/khaledsaeed18/Drainscope/Monitor io.github.khaledsaeed18.Drainscope.Monitor1 GetSummary 2>&1 | cut -c1-120
  kill $pid; wait $pid 2>/dev/null; pkill -u "$(id -u)" -f "^/usr/bin/drainscope-daemon$" || true
' 2>/dev/null
echo "JS errors/warnings: $(grep -cE 'JS ERROR|JS WARNING' $S/shell.log)"
grep -E 'JS ERROR|JS WARNING' $S/shell.log | head -5
