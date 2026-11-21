---
status: accepted
---

# Database access in three levels, with typed access for Python and C# in the MVP

Every language gets **Level 2** in the MVP: a ready-connected `db` client that returns rows you can `dump()`. **Level 3**, LINQPad-style typed access generated from the schema, is in the MVP only for the first two languages: C# via **EF Core** (generated `DbContext`, foreign keys as navigation properties) and Python via **SQLAlchemy 2.0** (typed `Mapped[]` models, sqlacodegen-style generation). TypeScript (planned: **Drizzle**, using `drizzle-kit pull`) and Rust get Level 3 in phase 2, because building typed access four times over was the biggest cost in the plan.

## Considered options
- **SQLModel** (Python): rejected. It is a thin FastAPI-oriented layer over SQLAlchemy, and complex queries drop down to SQLAlchemy anyway.
- **TypeORM** (TypeScript): rejected. It uses older decorator-based models, has weaker typing, and is developed more slowly.
- **Kysely** (TypeScript): the fallback if Drizzle's SQL Server support (only in the 1.0 beta as of late 2025) is not stable by phase 2.
- **linq2db** (C#): lighter and faster than EF Core, but far less familiar to C# developers.

## Consequences
- For Rust Level 2, the host opens the connection named in the script header with rusqlite, tokio-postgres, or Tiberius. This reuses the SQL runner's SQLite, PostgreSQL and SQL Server drivers and keeps connection passwords in the host. The owner approved the driver choice in [D73](../04-open-questions.md). Rust Level 3 is deferred to phase 2.
