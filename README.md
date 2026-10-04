# drainscope

Per-app battery and energy usage for the Linux desktop.

> "Firefox used 14% of your battery since you unplugged."
> Windows, macOS and Android have had this for years. Linux hasn't.

drainscope measures energy from hardware counters (RAPL via powercap) and the battery. It attributes that energy to apps, terminal workloads and system services through cgroup v2, DRM fdinfo and systemd scopes, keeps local history, and shows it in GNOME. No component runs as root.

**Status:** early development. The M0 feasibility spike is done ([findings](docs/adr/0001-feasibility.md)); M1 (daemon, sampler, CLI) is next. Nothing usable yet. See [PLAN.md](PLAN.md) for the architecture, privilege model and roadmap.

## Building (development)

Requirements: Fedora 44 (or another systemd + cgroup v2 distro), Rust stable, gcc, Node 24 and pnpm.

```bash
cargo build --workspace
cd ui && pnpm install && pnpm typecheck && pnpm lint && pnpm test
```

## License

[GPL-3.0-or-later](LICENSE)
