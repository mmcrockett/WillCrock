use crate::cli;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Deserialize;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

const USAGE: &str = "usage: willcrock photos import-tags [--dry-run|--apply] < accepted.json";

#[derive(Deserialize)]
struct Manifest {
    label: String,
    photos: Vec<Photo>,
}

#[derive(Deserialize)]
struct Photo {
    relative_path: String,
    sha256: String,
    #[serde(default = "accepted_decision")]
    decision: String,
}

fn accepted_decision() -> String {
    "accepted".into()
}

struct Entry {
    path: String,
    already_tagged: bool,
}

struct Report {
    label: String,
    entries: Vec<Entry>,
}

fn parse_args(args: &[String]) -> Result<bool, String> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--dry-run" => Ok(false),
        [flag] if flag == "--apply" => Ok(true),
        _ => Err(USAGE.into()),
    }
}

pub(super) fn run(args: &[String]) {
    let apply = parse_args(args).unwrap_or_else(|e| cli::fail(e));
    let (cfg, conn) = super::open_db();
    let Some(photos) = cfg.photos else {
        cli::fail("no [photos] section in config");
    };
    let report = import(
        &conn,
        Path::new(&photos.dir),
        std::io::stdin().lock(),
        apply,
    )
    .unwrap_or_else(|e| cli::fail(e));
    for entry in &report.entries {
        let action = if entry.already_tagged {
            "unchanged"
        } else if apply {
            "added"
        } else {
            "would add"
        };
        println!("{action} {}\t{}", report.label, entry.path);
    }
    let unchanged = report.entries.iter().filter(|e| e.already_tagged).count();
    let added = report.entries.len() - unchanged;
    if apply {
        println!("added {added}, already tagged {unchanged}");
    } else {
        println!("dry run: would add {added}, already tagged {unchanged}; no changes made");
    }
}

fn import(conn: &Connection, root: &Path, input: impl Read, apply: bool) -> Result<Report, String> {
    let manifest: Manifest =
        serde_json::from_reader(input).map_err(|e| format!("invalid tag import JSON: {e}"))?;
    let tags = super::parse_tags(&manifest.label)?;
    let [label] = tags.as_slice() else {
        return Err("Import must specify exactly one tag label.".into());
    };
    if manifest.label.contains(',') {
        return Err("Import must specify exactly one tag label.".into());
    }
    if manifest.photos.is_empty() {
        return Err("Import must include at least one photo.".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("photo directory: {e}"))?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for photo in manifest.photos {
        let path = &photo.relative_path;
        if !seen.insert(path.clone()) {
            return Err(format!("duplicate photo path in import: {path}"));
        }
        if photo.decision != "accepted" {
            return Err(format!("photo is not accepted for import: {path}"));
        }
        if photo.sha256.len() != 64 || !photo.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("invalid SHA-256 for photo: {path}"));
        }
        let expected = photo.sha256.to_ascii_lowercase();
        let indexed: Option<(i64, String, bool)> = tx
            .query_row(
                "SELECT p.id, p.sha256, EXISTS(\
                   SELECT 1 FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id \
                   WHERE pt.photo_id = p.id AND t.name = ?2) \
                 FROM photos p WHERE p.path = ?1 AND p.missing_at IS NULL",
                params![path, label],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("{path}: {e}"))?;
        let Some((id, indexed_hash, already_tagged)) = indexed else {
            return Err(format!("photo is not indexed or is marked missing: {path}"));
        };
        if indexed_hash != expected {
            return Err(format!("indexed SHA-256 does not match import: {path}"));
        }
        let file = super::photo_file(&root, path).map_err(|e| format!("{path}: {e}"))?;
        if super::hash(&file).map_err(|e| format!("{path}: {e}"))? != expected {
            return Err(format!("file SHA-256 does not match import: {path}"));
        }
        entries.push((
            id,
            Entry {
                path: photo.relative_path,
                already_tagged,
            },
        ));
    }
    if apply {
        for (id, entry) in &entries {
            if !entry.already_tagged {
                super::add_tag(&tx, *id, label).map_err(|e| format!("{}: {e}", entry.path))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(Report {
        label: label.clone(),
        entries: entries.into_iter().map(|(_, entry)| entry).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, photos};
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;

    fn setup(name: &str) -> (Connection, PathBuf, Value) {
        let root = photos::tests::temp_dir(name);
        fs::write(root.join("one.jpg"), b"first photo").unwrap();
        fs::write(root.join("two.jpg"), b"second photo").unwrap();
        let mut conn = db::open_memory();
        photos::scan(&mut conn, &root, 100).unwrap();
        let photos = ["one.jpg", "two.jpg"].map(|path| {
            json!({
                "id": format!("review-{path}"),
                "relative_path": path,
                "sha256": photos::hash(&root.join(path)).unwrap()
            })
        });
        let manifest = json!({
            "label": " Hockey ",
            "root": "/a/local/path/that/is/not/the/server/library",
            "tagging_rule": "Visible hockey content only.",
            "photos": photos
        });
        (conn, root, manifest)
    }

    fn run_import(
        conn: &Connection,
        root: &Path,
        manifest: &Value,
        apply: bool,
    ) -> Result<Report, String> {
        import(conn, root, manifest.to_string().as_bytes(), apply)
    }

    fn tag_count(conn: &Connection, label: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id WHERE t.name = ?1",
            [label],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn assert_no_import(conn: &Connection) {
        assert_eq!(tag_count(conn, "hockey"), 0);
        let tags: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags WHERE name = 'hockey'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tags, 0);
    }

    #[test]
    fn dry_run_does_not_create_tags_and_apply_is_additive_and_idempotent() {
        let (conn, root, manifest) = setup("import-tags-additive");
        let id: i64 = conn
            .query_row("SELECT id FROM photos WHERE path = 'one.jpg'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for tag in ["family", "2026", "2026-11", photos::ADMIN_ONLY_TAG] {
            photos::add_tag(&tx, id, tag).unwrap();
        }
        tx.commit().unwrap();
        let preview = run_import(&conn, &root, &manifest, false).unwrap();
        assert_eq!(preview.label, "hockey");
        assert_eq!(preview.entries.len(), 2);
        assert!(preview.entries.iter().all(|entry| !entry.already_tagged));
        assert_no_import(&conn);

        let tx = conn.unchecked_transaction().unwrap();
        photos::add_tag(&tx, id, "hockey").unwrap();
        tx.commit().unwrap();
        for apply in [false, true] {
            let report = run_import(&conn, &root, &manifest, apply).unwrap();
            assert!(report.entries[0].already_tagged);
            assert!(!report.entries[1].already_tagged);
            assert_eq!(tag_count(&conn, "hockey"), if apply { 2 } else { 1 });
        }
        let again = run_import(&conn, &root, &manifest, true).unwrap();
        assert!(again.entries.iter().all(|entry| entry.already_tagged));
        assert_eq!(tag_count(&conn, "hockey"), 2);
        for tag in ["family", "2026", "2026-11", photos::ADMIN_ONLY_TAG] {
            assert_eq!(tag_count(&conn, tag), 1);
        }
        assert_eq!(fs::read(root.join("one.jpg")).unwrap(), b"first photo");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_manifests_never_write_tags() {
        let (conn, root, manifest) = setup("import-tags-invalid");
        let mut invalid = Vec::new();
        for label in ["", "a/b", ".", "2026", "2026-11", "a,b", "hockey,"] {
            let mut input = manifest.clone();
            input["label"] = json!(label);
            invalid.push(input);
        }
        for hash in ["bad".to_string(), "g".repeat(64)] {
            let mut input = manifest.clone();
            input["photos"][1]["sha256"] = json!(hash);
            invalid.push(input);
        }
        for decision in ["pending", "rejected"] {
            let mut input = manifest.clone();
            input["photos"][1]["decision"] = json!(decision);
            invalid.push(input);
        }
        let mut empty = manifest.clone();
        empty["photos"] = json!([]);
        invalid.push(empty);
        let mut duplicate = manifest.clone();
        duplicate["photos"][1] = duplicate["photos"][0].clone();
        invalid.push(duplicate);
        for input in invalid {
            assert!(run_import(&conn, &root, &input, true).is_err());
            assert_no_import(&conn);
        }
        for input in ["{", &format!("{manifest} trailing data")] {
            assert!(import(&conn, &root, input.as_bytes(), true).is_err());
            assert_no_import(&conn);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decisions_may_be_omitted_or_accepted_but_not_null() {
        let (conn, root, mut manifest) = setup("import-tags-null-decision");
        manifest["photos"][1]["decision"] = Value::Null;
        for apply in [false, true] {
            let error = run_import(&conn, &root, &manifest, apply).err().unwrap();
            assert!(error.contains("invalid tag import JSON"));
            assert_no_import(&conn);
        }
        manifest["photos"][1]["decision"] = json!("accepted");
        run_import(&conn, &root, &manifest, true).unwrap();
        assert_eq!(tag_count(&conn, "hockey"), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_stale_photos_abort_the_entire_batch() {
        for case in [
            "unindexed",
            "marked-missing",
            "deleted",
            "changed",
            "stale-index",
        ] {
            let (conn, root, manifest) = setup(&format!("import-tags-{case}"));
            match case {
                "unindexed" => {
                    conn.execute("DELETE FROM photos WHERE path = 'two.jpg'", [])
                        .unwrap();
                }
                "marked-missing" => {
                    conn.execute(
                        "UPDATE photos SET missing_at = 1 WHERE path = 'two.jpg'",
                        [],
                    )
                    .unwrap();
                }
                "deleted" => fs::remove_file(root.join("two.jpg")).unwrap(),
                "changed" => fs::write(root.join("two.jpg"), b"changed content").unwrap(),
                "stale-index" => {
                    conn.execute(
                        "UPDATE photos SET sha256 = 'stale' WHERE path = 'two.jpg'",
                        [],
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            for apply in [false, true] {
                assert!(run_import(&conn, &root, &manifest, apply)
                    .err()
                    .unwrap()
                    .contains("two.jpg"));
                assert_no_import(&conn);
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn paths_must_match_exactly_and_cannot_escape_the_library() {
        let (conn, root, manifest) = setup("import-tags-paths");
        fs::create_dir(root.join("nested")).unwrap();
        std::os::unix::fs::symlink(root.join("one.jpg"), root.join("link.jpg")).unwrap();
        for path in ["../one.jpg", "/one.jpg", "nested/../one.jpg", "link.jpg"] {
            let mut input = manifest.clone();
            input["photos"] = json!([{
                "relative_path": path,
                "sha256": manifest["photos"][0]["sha256"]
            }]);
            conn.execute(
                "INSERT INTO photos (path, album, size, mtime, sha256, created_at) \
                 VALUES (?1, '', 1, 0, ?2, 0)",
                params![path, manifest["photos"][0]["sha256"].as_str().unwrap()],
            )
            .unwrap();
            assert!(run_import(&conn, &root, &input, true).is_err());
            assert_no_import(&conn);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn matching_hash_does_not_tag_other_copies_or_resolve_a_missing_path() {
        let (mut conn, root, mut manifest) = setup("import-tags-copies");
        fs::copy(root.join("one.jpg"), root.join("two.jpg")).unwrap();
        photos::scan(&mut conn, &root, 200).unwrap();
        manifest["photos"].as_array_mut().unwrap().truncate(1);
        let hash = manifest["photos"][0]["sha256"]
            .as_str()
            .unwrap()
            .to_uppercase();
        manifest["photos"][0]["sha256"] = json!(hash);
        run_import(&conn, &root, &manifest, true).unwrap();
        assert_eq!(tag_count(&conn, "hockey"), 1);
        let view =
            photos::tag_view(&conn, &["hockey".into()], 1, photos::Visibility::Admin).unwrap();
        assert_eq!(view.photos[0].path, "one.jpg");
        manifest["photos"][0]["relative_path"] = json!("renamed.jpg");
        assert!(run_import(&conn, &root, &manifest, true).is_err());
        assert_eq!(tag_count(&conn, "hockey"), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_failure_rolls_back_tags_added_earlier_in_the_batch() {
        let (conn, root, manifest) = setup("import-tags-rollback");
        let second_id: i64 = conn
            .query_row("SELECT id FROM photos WHERE path = 'two.jpg'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute_batch(&format!(
            "CREATE TRIGGER reject_import BEFORE INSERT ON photo_tags \
             WHEN NEW.photo_id = {second_id} BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
        ))
        .unwrap();
        assert!(run_import(&conn, &root, &manifest, true)
            .err()
            .unwrap()
            .contains("test failure"));
        assert_no_import(&conn);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_requires_an_explicit_unambiguous_apply_flag() {
        assert!(!parse_args(&[]).unwrap());
        assert!(!parse_args(&["--dry-run".into()]).unwrap());
        assert!(parse_args(&["--apply".into()]).unwrap());
        for args in [
            vec!["--apply".into(), "--dry-run".into()],
            vec!["--aply".into()],
            vec!["accepted.json".into()],
        ] {
            assert!(parse_args(&args).is_err());
        }
    }
}
