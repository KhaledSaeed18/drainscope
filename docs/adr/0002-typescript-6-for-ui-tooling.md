# 0002 — Pin TypeScript 6.0 for the UI workspace

- Status: accepted
- Date: 2026-10-04

## Context

TypeScript 7.0 (the native Go-based compiler) is `latest` on npm. typescript-eslint 8.71 (latest) declares `typescript >=4.8.4 <6.1.0` as its peer range, because its type-aware rules depend on the JavaScript compiler API that TypeScript 7 doesn't expose in the same form.

Our conventions depend on type-aware linting (`strict-type-checked`, the `no-unsafe-*` rules, a ban on type assertions). Dropping it to get TypeScript 7 would remove the enforcement of "no `any`, no unchecked casts".

## Decision

Use `typescript ~6.0.3` (the last JavaScript-based release) for type checking and linting in `ui/`. Bundling uses esbuild, which doesn't depend on the TypeScript version.

## Consequences

- Type-aware linting keeps working with released tooling.
- Revisit once typescript-eslint supports TypeScript 7 (watch its peer range). The upgrade should be a version bump plus fixes for any new diagnostics.
