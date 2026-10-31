# WillCrock

Family site: a Rust FastCGI binary on shared hosting.

## Checks

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs the same checks on every push.

## Photos

`bin/photos-sync` syncs `~/willcrock.com/photos` with the server (no deletes either way) and tells the server which files the Mac has. Set `WILLCROCK_REMOTE=user@host` first; `--dry-run` shows what would move.

On the server, `willcrock.fcgi photos scan` indexes the folder and `willcrock.fcgi photos unsynced` lists files not yet pulled to the Mac (also shown on the Photos page).
