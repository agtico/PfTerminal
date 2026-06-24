# Workflow Strategy

PFTerminal keeps a narrow automatic workflow surface. The inherited upstream
Codex full-suite workflows assume upstream model/provider defaults and create
irrelevant red checks on this fork, so they are manual-only here.

## Automatic Checks

- `pfterminal-ci.yml` is the primary deploy smoke check. It builds the real
  `pfterminal` and compatibility `codex` binaries, validates installer/package
  helper scripts, builds a PFTerminal package archive, and smoke-tests the
  extracted archive.
- Small hygiene workflows such as spelling or dependency policy can remain
  enabled when they give direct signal without requiring upstream OpenAI test
  assumptions.

## Manual Upstream Suites

- `bazel.yml`, `rust-ci.yml`, `rust-ci-full.yml`, `sdk.yml`, and
  `v8-canary.yml` are retained for manual debugging against upstream Codex
  compatibility issues.
- Do not treat those suites as PFTerminal deploy blockers unless their test
  harness has first been adapted to PFTerminal providers and model metadata.

## Manual Release Builds

- `pfterminal-release.yml` is the narrow package builder for the
  standalone PFTerminal installer. It is manual-only, builds
  `aarch64-apple-darwin`, `x86_64-apple-darwin`, and
  `x86_64-unknown-linux-gnu`, and uploads the exact
  `pfterminal-package-*.tar.gz` archives plus `pfterminal-package_SHA256SUMS`
  consumed by `scripts/install/install.sh`.
- Run it in build-only mode for compatibility checks. Use `publish_release`
  only when the current Cargo version is ready to become a GitHub release.

## Rule Of Thumb

- If a build/test/clippy check can be expressed in Bazel, prefer putting the PR-time version in `bazel.yml`.
- Keep `rust-ci.yml` fast enough that it usually does not dominate PR latency.
- Reserve `rust-ci-full.yml` for heavyweight Cargo-native coverage that Bazel does not replace yet.
