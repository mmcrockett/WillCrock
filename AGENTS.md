# AGENTS.md

## Routes

- Admin-only pages and actions live under `/admin`. The `/admin/*` guard in `src/routes.rs` is the single gate, so don't put an admin feature at another path and rely on a per-handler `is_admin` check.
- Feature pages (SMS, Pickle) gate on `user.can(Feature::X)` in `src/routes.rs` and in the nav/tile in `src/pages.rs`; admins pass implicitly. Add a new feature to `Feature` in `src/auth.rs` and it shows up as a toggle on the Users page and in `willcrock user grant|revoke`.

## Rust tooling

- Check for a working local toolchain with `command -v cargo && cargo --version`. If it works, use the local Cargo commands in the README.
- If Cargo is unavailable, check Docker with `docker info`. If Docker is unavailable or the daemon is not running, report that instead of repeatedly trying Cargo.
- From the checkout root, run tests in Docker with:

      docker run --rm -v "$(pwd):/app" -v willcrock-cargo:/usr/local/cargo/registry -v willcrock-target:/app/target -w /app rust:latest cargo test -q

- To run all README checks, install rustfmt and clippy in the same disposable container:

      docker run --rm -v "$(pwd):/app" -v willcrock-cargo:/usr/local/cargo/registry -v willcrock-target:/app/target -w /app rust:latest sh -c 'rustup component add rustfmt clippy && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test'

- If Docker warns that a `linux/amd64` image is running on an ARM host, it may work through emulation but can be slower. On a `linux/arm64` Docker host, `docker pull --platform linux/arm64 rust:latest` selects a native image for subsequent commands. Alternatively, add `--platform linux/amd64` to `docker run` to explicitly use the cached x86 image under emulation.
