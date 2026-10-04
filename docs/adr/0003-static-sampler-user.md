# 0003 — The sampler runs as a static system user, not `DynamicUser`

- Status: accepted
- Date: 2026-10-05

## Context

PLAN.md planned `DynamicUser=yes` for the sampler: an ephemeral UID allocated by systemd while the service runs. The sampler must also own a well-known name on the system bus, and the bus policy grants that per user (`<policy user="…"><allow own="…"/>`).

D-Bus brokers resolve the user names in policy files when they load the configuration. A dynamic user exists only while its service runs, so at policy-load time the name usually doesn't resolve, and the policy can't reliably grant ownership. The service is D-Bus activated, so it is never running when the broker first loads the policy.

## Decision

Create a dedicated system user, `drainscope-sampler`, through `sysusers.d` (`data/sysusers/drainscope.conf`), and run the service as it with `User=` and `Group=`. Everything else from the privilege model stays the same: the only capability is `CAP_DAC_READ_SEARCH` (ambient and bounding set), `NoNewPrivileges=yes`, and the full sandbox in `data/systemd/drainscope-sampler.service`.

## Consequences

- Name ownership works the standard way, with a policy entry for a user that always exists.
- The sampler still isn't root and gains no other privilege; `systemd-analyze security` rates the unit 0.6 ("SAFE"), checked in CI against a 2.0 threshold.
- One persistent system user is created at install time. As usual for sysusers, uninstalling keeps it.
