# Repository Guidelines

## Project Structure & Module Organization
`src/` holds the Rust runtime for the builder: `main.rs` wires CLI args from `src/args.rs`, orchestrates RPC logic (`src/rpc.rs`), bundle plumbing (`src/bundle.rs`), and persistence (`src/state.rs`). Shared primitives and limits live in `src/primitives.rs` and `src/limits.rs`. Integration-style scenarios live under `src/tests/`. Supporting dashboards are stored in `grafana/`, and reusable utilities live in `tools/fbutil/`, which is part of the Cargo workspace declared in `Cargo.toml`. Top-level `Makefile` and `rustfmt.toml` define automation and style, respectively.

## Build, Test, and Development Commands
- `cargo build --release` produces the optimized `target/release/unichain-builder` binary used in playground workflows.
- `cargo run --release -- node --builder.playground --metrics=localhost:5559` starts the builder with metrics enabled; add `RUST_LOG=debug` for verbose trace.
- `cargo check --workspace --all-targets --all-features` (or `make check`) catches type errors quickly.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` (or `make clippy`) enforces lint hygiene aligned with `Cargo.toml`’s lints block.
- `cargo +nightly fmt --all` (or `make fmt`) enforces the repo’s `rustfmt.toml` settings.
- `cargo test --workspace --all-features` (or `make test`) runs unit and integration suites.

## Coding Style & Naming Conventions
Use `rustfmt` nightly with the provided config: hard tabs, width 80, grouped imports (`group_imports = "StdExternalCrate"`), and normalized doc comments. Follow Rust 2024 idioms—prefer `snake_case` for functions/modules, `CamelCase` for types, and uppercase `SCREAMING_SNAKE_CASE` for constants. Keep modules small and colocate tests via `#[cfg(test)] mod tests` when practical. Logging relies on `tracing`; prefer structured fields over string concatenation.

## Testing Guidelines
Unit tests typically live alongside modules under `#[cfg(test)]`, while scenario tests belong in `src/tests/`. Use `tokio::test` for async contexts. New integration tests should be named after the behavior under test (e.g., `ensures_bundle_timeout`). Run the full suite with `cargo test --workspace --all-features`; builder playground workflows should also be validated manually following `README.md`. Aim to keep newly added modules covered by both happy-path and failure-path assertions.

## Commit & Pull Request Guidelines
Recent commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) optionally referencing PR numbers. Mirror that style and keep messages imperative and scoped to one concern. PRs should include: concise summary, linked issue or ticket, reproduction/verification steps (commands above), and screenshots for Grafana/dashboard changes under `grafana/`. Mention any protocol or API compatibility impacts and document configuration updates (e.g., new CLI flags) in `README.md` before requesting review. Continuous integration expects clean `cargo fmt`, `clippy`, and `cargo test` runs; cite the command outputs when posting the PR description.
