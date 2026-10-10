# Packaging and releases

## Artifacts

```bash
cargo xtask dist          # tarball + SRPM + extensions.gnome.org zip, in target/dist/
cargo xtask dist --rpm    # also build binary RPMs locally (test build, uses the toolchain on PATH)
```

`dist` refuses to run with uncommitted changes (the tarball is built from `HEAD`) or when the spec's `License:` tag no longer matches the bundled crates' licenses.

- **`drainscope-<version>.tar.gz`** — the git tree, the Rust crates vendored for an offline build, `LICENSE.dependencies` (each bundled crate and its license), and the prebuilt GNOME Shell extension bundle (so the RPM build needs no Node.js).
- **SRPM** from [`drainscope.spec`](drainscope.spec), with seven binary packages:
  - `drainscope`: the user daemon, the CLI, the user unit, the session-bus activation file and the D-Bus interface XML.
  - `drainscope-sampler`: the sandboxed RAPL sampler, its system unit, sysusers entry, D-Bus policy and activation file, and the polkit action.
  - `drainscope-probe`: the sandboxed eBPF probe, with the same set of files.
  - `drainscope-selinux` (noarch): the policy modules for the sampler and the probe, pulled in automatically on SELinux systems.
  - `drainscope-app` (noarch): the desktop app.
  - `gnome-shell-extension-drainscope` (noarch).
  - plus debuginfo and debugsource packages.
- **`drainscope@khaledsaeed18.github.io.shell-extension.zip`** for extensions.gnome.org.

Accounts, keys and tokens for each channel (GitHub, COPR, extensions.gnome.org, Flathub, Fedora) are in [docs/distribution.md](../docs/distribution.md).

## Releasing

1. Bump `version` in the root `Cargo.toml`, `Version:` and `%changelog` in the spec, and `version-name` in `ui/extension/metadata.json`; commit.
2. `cargo xtask dist --rpm` and install the RPMs on a test machine
   (`sudo dnf install target/dist/rpmbuild/RPMS/*/*.rpm`; remove a development install first with `sudo target/release/xtask uninstall-dev`).
3. Tag and publish the GitHub release with the tarball (the spec's `Source0` points there):
   `git tag -a vX.Y.Z -m "drainscope X.Y.Z" && git push origin vX.Y.Z && gh release create vX.Y.Z <tarball, SRPM, RPMs, zip>` (v0.1.0 to v0.1.4 were published this way)
4. COPR (once: a Fedora account and an API token in `~/.config/copr`; see docs/distribution.md):
   `copr-cli build drainscope target/dist/rpmbuild/SRPMS/drainscope-X.Y.Z-1.*.src.rpm` (the project exists: https://copr.fedorainfracloud.org/coprs/khaledsaeed18/drainscope/)
5. Validate the COPR packages on each Fedora version with `packaging/validate-copr.sh X.Y.Z` in a toolbox (instructions at the top of the script): versions, signatures, file integrity, the database lock and the extension in a nested GNOME Shell.
6. extensions.gnome.org: upload the zip at https://extensions.gnome.org/upload/. Its description must say that the extension needs the drainscope daemon (packaged separately).

Users then install with:

```bash
sudo dnf copr enable khaledsaeed18/drainscope
sudo dnf install drainscope drainscope-sampler drainscope-probe drainscope-app gnome-shell-extension-drainscope
systemctl --user enable --now drainscope.service
```
