---
name: sqlx-migrations
description: >-
  Write and test SQLite migrations with sqlx
  without breaking existing rows.
tags:
  - rust
  - sqlx
  - sqlite
---
# sqlx migrations

1. Add `migrations/NNNN_name.sql`.
2. Run `sqlx migrate run` against a scratch DB.
3. Never edit a migration that has shipped.
