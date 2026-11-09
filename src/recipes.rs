use rusqlite::{params, Connection, OptionalExtension};

pub struct Recipe {
    pub id: i64,
    pub name: String,
    pub archived: bool,
}

pub fn parse_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Recipe name is required.");
    }
    Ok(name.to_string())
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Recipe>> {
    conn.prepare(
        "SELECT id, name, archived_at FROM recipes \
         ORDER BY archived_at IS NOT NULL, name COLLATE NOCASE, id",
    )?
    .query_map([], |r| {
        Ok(Recipe {
            id: r.get(0)?,
            name: r.get(1)?,
            archived: r.get::<_, Option<i64>>(2)?.is_some(),
        })
    })?
    .collect()
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Recipe>> {
    conn.query_row(
        "SELECT id, name, archived_at FROM recipes WHERE id = ?1",
        [id],
        |r| {
            Ok(Recipe {
                id: r.get(0)?,
                name: r.get(1)?,
                archived: r.get::<_, Option<i64>>(2)?.is_some(),
            })
        },
    )
    .optional()
}

pub fn add(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute("INSERT INTO recipes (name) VALUES (?1)", [name])?;
    Ok(conn.last_insert_rowid())
}

pub fn rename(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE recipes SET name = ?1 WHERE id = ?2",
        params![name, id],
    )
}

pub fn set_archived(
    conn: &Connection,
    id: i64,
    archived: bool,
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE recipes SET archived_at = ?1 WHERE id = ?2",
        params![archived.then_some(now), id],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    #[test]
    fn names_are_trimmed_and_required() {
        assert_eq!(parse_name("  Tomato soup  ").unwrap(), "Tomato soup");
        assert_eq!(parse_name(" \t ").unwrap_err(), "Recipe name is required.");
    }

    #[test]
    fn recipes_are_shared_ordered_and_can_be_renamed_and_archived() {
        let conn = open_memory();
        let soup = add(&conn, "Tomato soup").unwrap();
        add(&conn, "Baked pasta").unwrap();

        assert_eq!(
            list(&conn)
                .unwrap()
                .into_iter()
                .map(|recipe| recipe.name)
                .collect::<Vec<_>>(),
            ["Baked pasta", "Tomato soup"]
        );
        assert_eq!(get(&conn, soup).unwrap().unwrap().name, "Tomato soup");
        assert_eq!(rename(&conn, soup, "Roasted tomato soup").unwrap(), 1);
        assert_eq!(
            get(&conn, soup).unwrap().unwrap().name,
            "Roasted tomato soup"
        );
        assert_eq!(set_archived(&conn, soup, true, 10).unwrap(), 1);
        assert_eq!(
            list(&conn)
                .unwrap()
                .into_iter()
                .map(|recipe| (recipe.name, recipe.archived))
                .collect::<Vec<_>>(),
            [
                ("Baked pasta".to_string(), false),
                ("Roasted tomato soup".to_string(), true)
            ]
        );
        assert!(get(&conn, soup).unwrap().unwrap().archived);
        assert_eq!(set_archived(&conn, soup, false, 11).unwrap(), 1);
        assert!(!get(&conn, soup).unwrap().unwrap().archived);
    }

    #[test]
    fn deleted_ids_are_not_reused() {
        let conn = open_memory();
        let old_id = add(&conn, "Old recipe").unwrap();
        conn.execute("DELETE FROM recipes WHERE id = ?1", [old_id])
            .unwrap();
        let new_id = add(&conn, "New recipe").unwrap();
        assert_ne!(old_id, new_id);
    }

    #[test]
    fn names_are_unique_without_regard_to_case() {
        let conn = open_memory();
        add(&conn, "Tomato soup").unwrap();
        assert!(add(&conn, "tomato SOUP").is_err());
        let pasta = add(&conn, "Pasta").unwrap();
        assert!(rename(&conn, pasta, "TOMATO SOUP").is_err());
    }
}
