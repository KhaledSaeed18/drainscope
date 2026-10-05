# CLAUDE.md — drainscope

Per-app battery and energy usage for the Linux desktop. Rust daemons plus a TypeScript (GJS) GNOME UI.
**PLAN.md is the source of truth** for architecture, milestones and the privilege model. Read it before non-trivial work and keep it updated when decisions change.

## Repo map

```
crates/model    core: domain types, identity rules, attribution model, calibration (NO I/O)
crates/sys      system integration: sysfs/procfs/cgroupfs readers, D-Bus clients (UPower, logind, Sampler1)
crates/store    repository: SQLite schema, migrations, the Store repository (ONLY crate importing rusqlite)
crates/dbus     D-Bus contracts: zbus interfaces/proxies generated from data/dbus/*.xml
crates/access   polkit authorization and per-user rate limits shared by the privileged services
bins/sampler    privileged system service (Sampler1), D-Bus activated
bins/daemon     user service, composition root, Monitor1 server
bins/cli        `drainscope` CLI, talks only to Monitor1
xtask           dev tooling: record-fixture, validate, install-dev, dist
ui/shared       TS: typed Monitor1 client, GVariant decoders, formatting (runs under Node for tests)
ui/extension    TS → GJS: GNOME Shell 50 extension
ui/app          TS → GJS: libadwaita app (M3)
data/           systemd units, D-Bus XML/policy/activation, polkit policy, SELinux (M5)
testdata/       traces/: short curated traces (committed); local/: long or personal traces (git-ignored)
docs/adr/       architecture decision records
```

## Commands

```bash
# Rust
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace -- --ignored          # live-hardware tests, this machine only
cargo build -p xtask && sudo target/debug/xtask record-fixture <name> --secs 600   # RAPL is root-only
cargo xtask spike-attribute testdata/traces/<name>.jsonl.gz                       # M0 throwaway
cargo xtask validate                           # accuracy harness, run on battery (M1)
cargo xtask install-dev | uninstall-dev        # installs units/policy to /usr/local (sudo)
cargo xtask dist [--rpm]                       # tarball (vendored), SRPM [+ RPMs], EGO zip → target/dist/

# TypeScript (from ui/)
pnpm install
pnpm typecheck                                 # tsc in every package
pnpm lint                                      # eslint, type-aware, whole workspace
pnpm test                                      # vitest in every package
pnpm --filter @drainscope/extension build       # esbuild → dist/, unminified ESM
pnpm --filter @drainscope/extension install-dev # also copy to ~/.local/share/gnome-shell/extensions/
pnpm --filter @drainscope/app build             # esbuild → app/dist/drainscope-app (gjs -m)
pnpm --filter @drainscope/app install-dev       # also install to ~/.local (bin, desktop entry, icon)

# Checks
systemd-analyze security --offline=yes data/systemd/drainscope-sampler.service   # must stay ≤ 2.0
```

Before saying a task is done: run fmt, clippy (`-D warnings`), the tests for the touched crates/packages, and `pnpm typecheck && pnpm lint && pnpm test` (from `ui/`) if UI code changed. Report failures verbatim; don't hide them.

## Architecture rules (enforced in review)

1. **Layering:** system integration (`sys`) → core (`model`) → interface (`dbus`, bins, `ui`). Storage is a repository layer (`store`).
   - `model` performs **no I/O** and depends on no runtime crates (no tokio, zbus, rusqlite, procfs). Add a port trait only when a fake is genuinely needed; prefer fixture trees and in-memory stores.
   - `sys` is the **only** place that reads `/sys`, `/proc`, `/dev`, or cgroupfs.
   - `store` is the **only** crate that imports `rusqlite` (the equivalent of "no ORM outside the repository layer").
   - D-Bus *server* code lives only in bins; `crates/dbus` holds contracts, not logic.
   - The CLI and UI never read the database or sysfs directly; they go through Monitor1. (Exception: `drainscope doctor` uses `sys` read-only probes.)
   - Attribution logic lives only in `model::attribution`. Bins wire things together; they don't compute.
2. **Native interfaces only.** Product code never spawns processes to read system state (`upower`, `busctl`, `cat`, `ps`, `powertop`, `journalctl`…). Use sysfs/procfs/cgroupfs/D-Bus. Exceptions need an ADR in `docs/adr/`. `xtask` dev tooling may shell out.
3. **Privileges.** Nothing runs as root.
   - The sampler runs as the `drainscope-sampler` system user (ADR 0003) with only `CAP_DAC_READ_SEARCH`. Every Sampler1 call is polkit-authorized, rate-limited and quantized.
   - Never widen the sampler's capabilities, readable paths, D-Bus policy, or polkit defaults without an ADR and explicit approval from the maintainer.
   - The daemon, CLI and UI are fully unprivileged.
   - No component opens network sockets; units set `IPAddressDeny=any`.
4. **D-Bus contracts.** `data/dbus/*.xml` is authoritative. Interfaces are versioned (`…Sampler1`, `…Monitor1`); within a version, changes are additive only. Ask before changing any interface.
5. **Model versioning.** Any change to attribution output bumps `MODEL_VERSION`, updates `docs/attribution-model.md`, and re-runs `xtask validate` (results go in `docs/validation.md`).

## Rust conventions

- Edition 2024, stable toolchain. `#![forbid(unsafe_code)]` in every crate. If unsafe is ever needed (e.g. M4 eBPF glue), it goes in one isolated module with an ADR.
- Errors: `thiserror` enums in library crates, `anyhow` only in `bins/*` and `xtask`. `clippy::unwrap_used` and `clippy::expect_used` are denied workspace-wide; tests are exempt via `clippy.toml`. Use `?`, `let … else`, or explicit handling.
- Lints are set once in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- Clippy: `all` + `pedantic` as warnings, CI uses `-D warnings`. Allow specific pedantic lints locally with a one-line justification.
- Async (tokio) only in bins and `crates/dbus`/`crates/sys` D-Bus clients. sysfs/procfs reads are synchronous and run per tick off the async executor (`spawn_blocking` or a dedicated collector thread).
- Units in names and types: counters are `u64` microjoules (`*_uj`), model math is `f64` joules (`*_j`), durations are `u64` nanoseconds (`*_ns`) on `CLOCK_MONOTONIC`, and stored timestamps are wall-clock seconds (`*_ts`). Never mix them without explicit conversion helpers from `model::units`.
- Every `sys` reader takes a `SysRoot` (base path) so it can run against `testdata/` fixtures. No hard-coded `/sys` or `/proc` outside `SysRoot::host()`.
- Logging via `tracing`; in services, output goes to journald via `tracing-journald`. Never log per-tick at `info`; use `debug`/`trace`.
- Dependencies: justify any new crate in the PR/commit message. Prefer small, well-maintained crates. `cargo deny` must pass.

## TypeScript conventions (ui/)

- pnpm only. TypeScript is pinned to `~6.0` because typescript-eslint doesn't support TypeScript 7 yet (ADR 0002). Imports are extensionless (`moduleResolution: Bundler`). No DOM lib: GJS has no DOM.
- TypeScript strict with `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noImplicitOverride`, `useUnknownInCatchVariables`.
- **No `any`. No `as` casts** (except `as const`). No non-null `!`. Enforced by typescript-eslint `strict-type-checked` + `stylistic-type-checked` and `consistent-type-assertions: never` (`ui/eslint.config.js`).
- `GLib.Variant` data must go through the typed decoders in `ui/shared/src/dbus/decode.ts`, which check `get_type_string()` before unpacking and return `Result`-style values. Never use raw `deepUnpack()` results directly.
- Keep logic in `ui/shared` (pure and Node-testable with vitest). GJS-specific glue in `extension/` and `app/` stays thin.
- GNOME Shell extension rules: GNOME 50 ESM (`gi://` imports, `resource:///org/gnome/shell/...`). Create nothing in the constructor; everything is created in `enable()` and destroyed or disconnected in `disable()`. All D-Bus calls are async and never block the Shell main loop. Output is unminified ESM (extensions.gnome.org review requirement).

## Traces and fixtures

- Format: gzipped JSON Lines. One header line, then one snapshot per line mapping kernel file paths (no leading `/`) to raw contents (`xtask/src/trace.rs`). Replays exercise the same parsers as live reads.
- Recorded: powercap counters, `power_supply`, `/proc/stat`, every cgroup's `cpu.stat`, DRM fdinfo + `comm`/`cgroup`/`stat` of DRM clients, and `comm` of terminal-scope processes. **Never** record command lines or environments.
- Commit only short traces (≲ 2 MB) to `testdata/traces/`; keep long ones in `testdata/local/`.

## Testing expectations

- `model`: unit tests + `proptest` invariants (energy conservation Σ = measured ± ε, non-negativity, wraparound, determinism).
- `sys`: fixture-tree tests; live tests are `#[ignore]`.
- `store`: in-memory SQLite; rollups must equal raw sums.
- D-Bus: zbus peer-to-peer connections, no real bus. Polkit sits behind a trait so it can be faked.
- `daemon`: trace-replay golden tests from `testdata/`. Update goldens only intentionally, and explain why in the commit.
- New identity rules (cgroup name → consumer) require a test case using a real cgroup path.

## Environment notes (dev machine)

Fedora 44, kernel 7.1, GNOME 50.3 Wayland, SELinux enforcing, systemd 259, cgroup v2. Intel i7-8550U: RAPL domains `package-0`, `core`, `uncore`, `dram`, `psys`, all `0400 root`. **`psys` is implausible here** (below `package`; ADR 0001), so never rely on it. True idle with the screen on: package ≈ 1.5 W, battery ≈ 5.1 W. Battery `power_now` lags load changes by 6–8 s. i915 exposes `drm-engine-*` in fdinfo. Two batteries (BAT0, BAT1). `mem_sleep` defaults to `deep`. 7 GB RAM: keep builds lean (`CARGO_BUILD_JOBS` if needed).

## Working agreement

- Implement milestone tasks from PLAN.md in order; each task should end with its **Verify** step satisfied.
- Ask before: changing the privilege model, D-Bus interfaces, DB schema (beyond migrations planned in PLAN.md), or adding a new language or runtime.
- Commits: Conventional Commits (`feat(model): …`, `fix(sampler): …`). Small and focused.
- Record significant decisions as ADRs (`docs/adr/NNNN-title.md`: context, decision, consequences).
