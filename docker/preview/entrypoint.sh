#!/bin/sh
set -eu

data_dir=/var/lib/willcrock-preview
db_path="$data_dir/preview.sqlite3"
preview_token=willcrock-preview-token

mkdir -p "$data_dir"
export WILLCROCK_CONFIG=/etc/willcrock-preview.toml
/usr/local/bin/willcrock.fcgi migrate

sqlite3 "$db_path" <<'SQL'
CREATE TABLE IF NOT EXISTS preview_seed (
  id INTEGER PRIMARY KEY CHECK (id = 1)
);
SQL

seeded=$(sqlite3 "$db_path" 'SELECT EXISTS(SELECT 1 FROM preview_seed WHERE id = 1)')
now=$(date +%s)
if [ "$seeded" = 0 ]; then
    sqlite3 "$db_path" <<SQL
BEGIN IMMEDIATE;
INSERT INTO users (email, name, is_admin, created_at)
VALUES ('preview@example.test', 'Preview user', 0, $now);
INSERT INTO user_access (user_id, feature)
SELECT id, 'recipes' FROM users WHERE email = 'preview@example.test';
INSERT INTO recipes (name, content) VALUES ('Tomato soup', '# Tomato soup

## Ingredients

- 1 tbsp olive oil
- 1 onion, diced
- 4 ripe tomatoes

## Method

1. Warm the oil and soften the onion.
2. Add tomatoes and simmer for 20 minutes.
3. Blend until smooth and season to taste.');
INSERT INTO preview_seed (id) VALUES (1);
COMMIT;
SQL
fi

token_hash=$(printf '%s' "$preview_token" | sha256sum | cut -d ' ' -f 1)
sqlite3 "$db_path" <<SQL
INSERT INTO sessions (user_id, token_hash, created_at, last_seen_at, expires_at)
SELECT id, '$token_hash', $now, $now, $((now + 315360000))
FROM users WHERE email = 'preview@example.test'
ON CONFLICT(token_hash) DO UPDATE SET
  user_id = excluded.user_id,
  last_seen_at = excluded.last_seen_at,
  expires_at = excluded.expires_at;
SQL

chown -R www-data:www-data "$data_dir"
exec apache2ctl -D FOREGROUND
