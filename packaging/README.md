# Packaging and releases

## Artifacts

```bash
cargo xtask dist          # tarball + SRPM + extensions.gnome.org zip, in target/dist/
cargo xtask dist --rpm    # also build binary RPMs locally (test build, uses the toolchain on PATH)
```

`dist` refuses to run with uncommitted changes (the tarball is built from `HEAD`) or when the spec's `License:` tag no longer matches the bundled crates' licenses.

- **`drainscope-<version>.tar.gz`** — the git tree, the Rust crates vendored for an offline build, `LICENSE.dependencies` (each bundled crate and its license), and the prebuilt GNOME Shell extension bundle (so the RPM build needs no Node.js).
- **SRPM** from [`drainscope.spec`](drainscope.spec), with three binary packages:
  - `drainscope`: the user daemon, the CLI, the user unit, the session-bus activation file and the D-Bus interface XML.
  - `drainscope-sampler`: the sandboxed RAPL sampler, its system unit, sysusers entry, D-Bus policy and activation file, and the polkit action.
  - `gnome-shell-extension-drainscope` (noarch).
- **`drainscope@khaledsaeed18.github.io.shell-extension.zip`** for extensions.gnome.org.

## Releasing

1. Bump `version` in the root `Cargo.toml`, `Version:` and `%changelog` in the spec, and `version-name` in `ui/extension/metadata.json`; commit.
2. `cargo xtask dist --rpm` and install the RPMs on a test machine
   (`sudo dnf install target/dist/rpmbuild/RPMS/*/*.rpm`; remove a development install first with `sudo target/release/xtask uninstall-dev`).
3. Tag and publish the GitHub release with the tarball (the spec's `Source0` points there):
   `git tag v0.1.0 && git push --tags && gh release create v0.1.0 target/dist/drainscope-0.1.0.tar.gz`
4. COPR (once: create the project at https://copr.fedorainfracloud.org and save an API token to `~/.config/copr`):
   `copr-cli build drainscope target/dist/rpmbuild/SRPMS/drainscope-0.1.0-1.*.src.rpm`
5. extensions.gnome.org: upload the zip at https://extensions.gnome.org/upload/. Its description must say that the extension needs the drainscope daemon (packaged separately).

Users then install with:

```bash
sudo dnf copr enable khaledsaeed18/drainscope
sudo dnf install drainscope drainscope-sampler gnome-shell-extension-drainscope
systemctl --user enable --now drainscope.service
```
