# WillCrock

Family site: a Rust FastCGI binary served by DreamHost shared hosting.

## Build

GitHub Actions builds a static `x86_64-unknown-linux-musl` binary on every push and uploads it as the `willcrock-fcgi` artifact (`willcrock.fcgi` + `.htaccess`).

## Manual deploy

1. Download the artifact: `gh run download -n willcrock-fcgi -D dist`
2. Upload: `scp dist/willcrock.fcgi dist/.htaccess <user>@<server>:~/willcrock.com/`
3. On the host: `chmod 755 ~/willcrock.com/willcrock.fcgi`
4. Remove or rename any default `index.html` in `~/willcrock.com/`.
5. Check: `curl -i https://willcrock.com/` returns `hello world`.

Later deploys: upload to `willcrock.fcgi.new`, then `mv` it over `willcrock.fcgi`. If the old process lingers, `pkill -f willcrock.fcgi`.

If it 500s, see `~/logs/willcrock.com/https/error.log` on the host.
