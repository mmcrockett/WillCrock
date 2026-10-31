# WillCrock

Family site: a Rust FastCGI binary on shared hosting.

## Checks

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs the same checks on every push.

## Photos

`bin/photos-sync --login user@host --local-dir DIR` syncs DIR (e.g. `~/willcrock.com/photos`) with the server (no deletes either way) and tells the server which files the Mac has. With no options it prints usage.

On the server, `willcrock.fcgi photos scan` indexes the folder and `willcrock.fcgi photos unsynced` lists files not yet pulled to the Mac (also shown on the Photos page).
