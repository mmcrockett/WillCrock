# AGENTS.md

## Routes

- Admin-only pages and actions live under `/admin`. The `/admin/*` guard in `src/routes.rs` is the single gate, so don't put an admin feature at another path and rely on a per-handler `is_admin` check.

## Rust tooling

- If there's no local Rust toolchain (`cargo` not on PATH), run it in docker with the `rust:latest` image. Mount the checkout at `/app` and use named volumes for the cargo registry and `target/`:

      docker run --rm -v <checkout>:/app -v willcrock-cargo:/usr/local/cargo/registry -v willcrock-target:/app/target -w /app rust:latest cargo test -q

- For `cargo fmt` and `cargo clippy`, first run `rustup component add rustfmt clippy` in the container, as `rust:latest` doesn't ship them.
