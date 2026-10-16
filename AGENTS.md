# Repository Guidelines

Default to concise, high-effort responses. Lead with answers, skip filler, and challenge flawed
assumptions directly.

## Environment

- Tooling: Rust 1.89+, `flatc` (FlatBuffers) on `PATH`, Python 3 for the ingestion helper tests.
- Original-engine runs (`ark-cli battle …`) require Windows x64 and an installed official client;
  everything else builds and tests on any platform.
- Give the user commands for long-running experiments, downloads, or dangerous and irreversible
  operations instead of running them.

## Project Structure & Architecture

Ark CLI is a Rust 2021 workspace (MSRV 1.89), with Cargo packages under `src/`. Each package's
README describes its boundaries.

- `src/ark_cli/` (`arknights-cli`): independently distributable `ark-cli` executable; solver,
  ingestion and battle command parsing, progress display and exit policy. This is the default
  workspace application.
- `src/ark_solver/` (`ark-solver`): library with `config`, `data`, `ingest`, `simulation` and
  `{maa_job,verification_artifact,mechanics,planning,replay,workflow}` modules in its `src/`.
  Its `tests/`, `examples/`, `schema/` and `tools/` hold consumer-local fixtures and build inputs.
  Module-boundary checks live in `src/ark_solver/tests/architecture_boundaries.rs`.
- `src/battle_runtime/` (`ark-battle-runtime`): original battle execution, MAA workflows, squad and stage
  resolution, battle programs, proof and repeat comparison in an owned Windows Unity Player.
- `src/ark_unity/` (`ark-unity`): Unity asset processing, Player build and Windows compatibility;
  no Python worker or dependency on the solver's simulated battle types.
- `src/battle_runtime_probe/` (`ark-battle-runtime-probe`): portable source provider for the isolated
  Cargo `no_std` plugin. The host materializes `template/` and builds the DLL with its own
  aborting-panic/no-CRT profile; importing the provider never builds the Windows DLL.

Keep conceptual boundaries as modules. Introduce another package only for demonstrated independent
reuse or delivery. Keep `vendor/*` as pinned upstream submodules.

## Architecture and Correctness Rules

- Keep `simulation` free of rendering, Arknights, MAA, asset, and other game-specific rules.
- Planning, pruning, ranking, and search propose candidates but never certify success. Only a
  complete replay from independently reconstructed inputs may report `ExpectedVictory` or
  `ExpectedDefeat`. A replay whose sole omissions are Conservative Beneficial Omissions counts as
  complete for `ExpectedVictory`, because leaving out a reviewed benefit only understates the
  strategy; `ExpectedDefeat` still requires that replay omitted nothing it reached. A Declared
  Approximation omits nothing — replay executes the effect at a value the model declares — so it is
  inventoried and disclosed with the verdict rather than withholding it; only evidence that its
  departure exceeds a margin the verdict depends on withholds one.
- Preserve distinct outcomes: unsupported mechanics, incomplete replay, proposal-budget
  exhaustion, infeasibility, defeat, and victory are not interchangeable.
- Original-engine results (`original_win` / `original_loss`) are observations of the original code
  in the owned Player, never `ExpectedVictory` / `ExpectedDefeat`.
- The MAA job file is the canonical executable format. `.rgs` is internal versioned verification
  evidence; PRTS Plus is external source/publisher provenance only.
- Enemy and operator semantics come from decoding revision-pinned data. A reviewed conclusion may
  live in a decoder that matches an exact serialized shape and carries its provenance; it may never
  live as a hand-written catalog keyed by unit identity. A record whose behavior no decoder claims is
  Unsupported, not hand-modelled.
- Do not add backward-compatibility shims, obsolete re-exports, duplicate types, or removed-code
  comments.

## Build and Validation

- `git submodule update --init --recursive -- vendor/ark-unpacker`: initialize code dependencies.
- `cargo run -p arknights-cli -- --help`: list application workflows (`ingest`, `battle`, …).
- `cargo fmt --all -- --check`: verify formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: run strict linting.
- `cargo test --workspace`: run the complete suite before integration.

Tests requiring a complete local gamedata checkout or the installed client are explicitly ignored
when that input is absent. Run focused checks while iterating, then the root gates before
declaring work complete. Run `python3 -m unittest discover -s src/ark_solver/tools/tests` when
changing ingestion helpers. Run `cargo test -p ark-battle-runtime -p ark-unity` when changing the
battle runtime; original-engine regression runs are described in `src/battle_runtime/README.md`.

## Coding Style

Use hard tabs (four columns) unless an existing file uses spaces. Keep lines near 100 characters,
follow Rust naming conventions, and add explicit types to changed code. Prefer small composable
systems, traits, and pure helpers over deeply stateful abstractions. Add INFO-level progress and
timing for expensive operations without over-logging. Fix lint and type errors properly; suppress
only as a documented last resort.

## Commit and Pull Request Guidelines

Use scoped Conventional Commit subjects such as `feat(ark-solver): add wave preview`,
`fix(simulation): preserve same-tick ordering`, or `test(ingest): verify snapshot integrity`. Keep
unrelated formatting separate from behavior changes. Push or create a PR only when explicitly
asked; never force-push without explicit authorization.

PRs should summarize behavior, link relevant issues, list validation commands and results, and
include screenshots for UI changes. Never commit client files, extracted assets, credentials,
`CLAUDE.md`, `.cursorrules`, or AI co-author trailers.
