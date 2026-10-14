# Contributing

Thanks for helping. Bug reports, measurements from hardware drainscope hasn't seen yet (AMD laptops especially), and pull requests are all welcome. Before you change code, read [Architecture](docs/architecture.md). It explains how the parts fit together and the rules that keep them small and unprivileged.

## What you need

- Fedora 44 or 45, or another distribution with systemd, cgroup v2 and polkit
- Rust stable and gcc; clang and `libbpf-devel` for the eBPF probe
- Node.js and pnpm for the extension and the app
- `selinux-policy-devel`, optional, to build the SELinux modules

## Build and install from source

Don't combine this with the COPR packages: `install-dev` puts units in `/etc`, which override the packaged ones. Run `uninstall-dev` first to go back to the packages.

```bash
cargo build --release -p drainscope-sampler -p drainscope-probe -p drainscope-daemon -p drainscope-cli -p xtask
sudo target/release/xtask install-dev        # installs to /usr/local and /etc only
systemctl --user daemon-reload
systemctl --user enable --now drainscope.service
drainscope doctor
```

With `selinux-policy-devel` installed, `install-dev` also loads the SELinux modules that confine the sampler and the probe.

The GNOME Shell extension (log out and back in afterwards) and the desktop app:

```bash
cd ui && pnpm install
pnpm --filter @drainscope/extension install-dev
gnome-extensions enable drainscope@khaledsaeed18.github.io
pnpm --filter @drainscope/app install-dev     # installs to ~/.local
```

Remove everything with `sudo target/release/xtask uninstall-dev` and `systemctl --user disable --now drainscope.service`.

## Checks

CI runs these checks. Run them before you send a change:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                          # includes replays of recorded hardware traces
cd ui && pnpm typecheck && pnpm lint && pnpm test
```

`cargo xtask validate` measures the model's accuracy on real hardware. Run it unplugged, with the laptop otherwise idle. Its results go in [docs/validation.md](docs/validation.md).

## Guidelines

- **Keep the layers.** Only `crates/sys` reads `/sys`, `/proc` and cgroupfs. Only `crates/store` uses SQLite. Attribution logic lives in `crates/model`, which does no I/O.
- **No helper programs.** Product code reads kernel and D-Bus interfaces directly. It never runs `upower`, `ps`, `cat` or similar tools.
- **Privileges stay as they are.** Changes to the system services' capabilities, D-Bus policy or polkit defaults need a design record and the maintainer's agreement first.
- **D-Bus interfaces are additive.** Within `Monitor1`, `Sampler1` and `Probe1`, existing methods and signatures never change.
- **Attribution changes are versioned.** A change to the attribution output bumps `MODEL_VERSION` and updates [the attribution model](docs/attribution-model.md) and [the validation results](docs/validation.md).
- **Strict TypeScript.** No `any`, no `as` casts and no non-null `!`. Logic belongs in `ui/shared`, where it's tested under Node.
- **Commits** follow [Conventional Commits](https://www.conventionalcommits.org/): `feat(model): …`, `fix(sampler): …`. Keep each one small and focused.
- **Design decisions** are recorded in [docs/adr/](docs/adr/).

## Reporting a problem

Please include the output of `drainscope doctor` and, if the daemon misbehaves, `journalctl --user -u drainscope.service -n 50`.
