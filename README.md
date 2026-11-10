# WillCrock

Family site: a Rust FastCGI binary on shared hosting.

## Checks

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs the same checks on every push.

## Photos

`bin/photos-sync --login user@host --local-dir DIR` syncs DIR (e.g. `~/willcrock.com/photos`) with the server and tells the server which files the Mac has. It does not generally delete photos; when a photo is moved, it removes the old local path only if that file matches the new path byte-for-byte. With no options it prints usage.

On the server, `willcrock.fcgi photos scan` indexes the folder and `willcrock.fcgi photos unsynced` lists files not yet pulled to the Mac (also shown on the Photos page).

`/photos` remains the years/tags landing page. Year/tag pages show thumbnail galleries with a larger viewer and original downloads, all requiring Photos access. JPEG previews are generated on demand and cached under `[photos].dir/.gallery-v1/`, outside the public web root. The cache is disposable and excluded by the sync helper. HEIC photos currently offer an original download instead of a browser preview; GIF previews show the first frame.

The Photos page can upload multiple JPEG, PNG, GIF, WebP, or HEIC photos, up to 25 MB each. It uses EXIF dates when available and otherwise the current Central year and month; each newly uploaded photo's month can be corrected after upload. Photos are stored in the matching `YYYY/YYYYMM00/` month folder, just like other library photos. Identical files already in the library are skipped. Uploads are indexed immediately and picked up by the next Mac sync. To move existing indexed uploads out of `uploads/`, apply database migrations with `willcrock migrate`, then run `willcrock.fcgi photos migrate-uploads` on the server and sync as usual. Photo dates and tags are stored in the database, so back up the database as well as the files.

Interrupted file moves resume automatically before the next scan, migration, upload, or date correction, preserving photo IDs, tags, and shared-album membership. Conflicting file contents stop recovery with an error rather than overwriting either file.

Admins can apply or remove the special `admin-only` tag in the photo tag picker. Tagged photos are hidden from every non-admin, regardless of feature access, including gallery counts, tag suggestions, previews, and original downloads. Only admins can edit these photos. The tag is preserved by date changes and scans; scan/sync backups still include the complete library.

Public share links never show `admin-only` photos, even to signed-in admins. Adding the tag hides a photo from existing share links; removing it restores the photo to those unexpired albums. Tagged photos cannot be selected for new shared albums. This does not revoke files already downloaded or restrict separate untagged copies.
