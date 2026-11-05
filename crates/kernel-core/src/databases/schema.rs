//! Read-only catalog discovery; independent of script execution and its row caps.
use super::*;
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub kind: DatabaseKind,
    pub tables: Vec<SchemaTable>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SchemaTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<SchemaColumn>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaColumn {
    pub name: String,
    pub data_type: String,
    pub references: Vec<ForeignKey>,
    #[serde(default)]
    pub primary_key: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ForeignKey {
    pub schema: String,
    pub table: String,
    pub column: String,
    #[serde(default)]
    pub constraint: String,
    #[serde(default)]
    pub ordinal: u32,
}

// One row per column/reference pair. Catalog joins preserve composite-key order.
const POSTGRES: &str = r#"
SELECT n.nspname::text, t.relname::text, a.attname::text,
       pg_catalog.format_type(a.atttypid,a.atttypmod),
       rn.nspname::text, rt.relname::text, ra.attname::text,
       CASE WHEN EXISTS (SELECT 1 FROM pg_catalog.pg_constraint p WHERE p.conrelid=t.oid AND p.contype='p' AND a.attnum=ANY(p.conkey)) THEN '1' ELSE '0' END,
       COALESCE(f.conname,''), COALESCE(array_position(f.conkey,a.attnum),0)::text
FROM pg_catalog.pg_class t
JOIN pg_catalog.pg_namespace n ON n.oid=t.relnamespace
JOIN pg_catalog.pg_attribute a ON a.attrelid=t.oid AND a.attnum>0 AND NOT a.attisdropped
LEFT JOIN pg_catalog.pg_constraint f ON f.conrelid=t.oid AND f.contype='f' AND a.attnum=ANY(f.conkey)
LEFT JOIN pg_catalog.pg_class rt ON rt.oid=f.confrelid
LEFT JOIN pg_catalog.pg_namespace rn ON rn.oid=rt.relnamespace
LEFT JOIN pg_catalog.pg_attribute ra ON ra.attrelid=rt.oid AND ra.attnum=f.confkey[array_position(f.conkey,a.attnum)]
WHERE t.relkind IN ('r','p','v','m','f') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg\_temp\_%' ESCAPE '\'
ORDER BY n.nspname,t.relname,a.attnum
"#;
const SQLSERVER: &str = r#"
SELECT s.name,t.name,c.name,
       ty.name + CASE WHEN ty.name IN ('varchar','char','varbinary','binary','nvarchar','nchar') THEN '(' + CASE WHEN c.max_length=-1 THEN 'max' ELSE CAST(c.max_length / CASE WHEN ty.name IN ('nvarchar','nchar') THEN 2 ELSE 1 END AS varchar(10)) END + ')'
       WHEN ty.name IN ('decimal','numeric') THEN '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' ELSE '' END,
       rs.name,rt.name,rc.name,
       CASE WHEN EXISTS (SELECT 1 FROM sys.index_columns ic JOIN sys.indexes i ON i.object_id=ic.object_id AND i.index_id=ic.index_id WHERE i.is_primary_key=1 AND ic.object_id=t.object_id AND ic.column_id=c.column_id) THEN '1' ELSE '0' END,
       COALESCE(f.name,''), CAST(COALESCE(f.constraint_column_id,0) AS varchar(10))
FROM sys.objects t JOIN sys.schemas s ON s.schema_id=t.schema_id
JOIN sys.columns c ON c.object_id=t.object_id
JOIN sys.types ty ON ty.user_type_id=c.user_type_id
LEFT JOIN sys.foreign_key_columns f ON f.parent_object_id=t.object_id AND f.parent_column_id=c.column_id
LEFT JOIN sys.objects rt ON rt.object_id=f.referenced_object_id
LEFT JOIN sys.schemas rs ON rs.schema_id=rt.schema_id
LEFT JOIN sys.columns rc ON rc.object_id=rt.object_id AND rc.column_id=f.referenced_column_id
WHERE t.type IN ('U','V') AND t.is_ms_shipped=0
ORDER BY s.name,t.name,c.column_id
"#;

type CatalogRow = [Option<String>; 10];

fn is_primary_key(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "t"
    )
}

fn assemble(kind: DatabaseKind, rows: Vec<CatalogRow>) -> Schema {
    let mut tables = BTreeMap::<(String, String), SchemaTable>::new();
    for [schema, table, column, data_type, target_schema, target_table, target_column, primary_key, constraint, ordinal] in
        rows
    {
        let schema = schema.unwrap_or_default();
        let name = table.unwrap_or_default();
        let table = tables
            .entry((schema.clone(), name.clone()))
            .or_insert_with(|| SchemaTable {
                schema,
                name,
                columns: vec![],
            });
        let name = column.unwrap_or_default();
        let index = table
            .columns
            .iter()
            .position(|c| c.name == name)
            .unwrap_or_else(|| {
                table.columns.push(SchemaColumn {
                    name,
                    data_type: data_type.unwrap_or_default(),
                    references: vec![],
                    primary_key: primary_key.as_deref().is_some_and(is_primary_key),
                });
                table.columns.len() - 1
            });
        if let Some(target) = target_table {
            table.columns[index].references.push(ForeignKey {
                schema: target_schema.unwrap_or_default(),
                table: target,
                column: target_column.unwrap_or_default(),
                constraint: constraint.unwrap_or_default(),
                ordinal: ordinal
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0),
            });
        }
    }
    Schema {
        kind,
        tables: tables.into_values().collect(),
    }
}

impl SqlManager {
    pub async fn schema(&self, name: &str) -> Result<Schema, String> {
        let connection = self.connections.by_name(name).ok_or_else(|| {
            format!("Connection \"{name}\" was not found. Add it in Connections…")
        })?;
        let discover = async {
            match connection.kind {
                DatabaseKind::Sqlite => {
                    let path = sqlite_path(&connection)?;
                    tokio::task::spawn_blocking(move || sqlite_schema(&path))
                        .await
                        .map_err(|e| e.to_string())?
                }
                DatabaseKind::Postgres => {
                    let (client, _) = self.connect_postgres(&connection).await?;
                    let rows = client
                        .query(POSTGRES, &[])
                        .await
                        .map_err(|e| pg_error("Could not read the schema", e))?;
                    Ok(assemble(
                        connection.kind,
                        rows.iter()
                            .map(|r| std::array::from_fn(|i| r.get(i)))
                            .collect(),
                    ))
                }
                DatabaseKind::SqlServer => {
                    let mut client = self.connect_sqlserver(&connection).await?;
                    let rows = client
                        .simple_query(SQLSERVER)
                        .await
                        .map_err(|e| e.to_string())?
                        .into_first_result()
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(assemble(
                        connection.kind,
                        rows.iter()
                            .map(|r| {
                                std::array::from_fn(|i| r.get::<&str, _>(i).map(str::to_owned))
                            })
                            .collect(),
                    ))
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), discover)
            .await
            .map_err(|_| "Schema discovery timed out; try Refresh.".to_owned())?
    }
}

impl SqlManager {
    /// Generate models only. Code intelligence does not need a password or a
    /// live connection beyond catalog discovery.
    pub async fn generated_models(
        &self,
        name: &str,
    ) -> Result<crate::data_context::GeneratedContext, String> {
        let schema = self.schema(name).await?;
        Ok(crate::data_context::generate(&schema))
    }

    /// Generate a typed data context and the connection details a kernel needs
    /// to open the same database. Passwords are read from the vault and kept in
    /// memory; they never enter a script or a generated file.
    pub async fn data_context(
        &self,
        name: &str,
    ) -> Result<crate::data_context::GeneratedContext, String> {
        let connection = self.connections.by_name(name).ok_or_else(|| {
            format!("Connection \"{name}\" was not found. Add it in Connections…")
        })?;
        let schema = self.schema(name).await?;
        let password = match connection
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
        {
            None => None,
            Some(secret) => {
                let store = self.secrets.clone();
                let secret = secret.to_owned();
                let value = tokio::task::spawn_blocking(move || store.get(&secret))
                    .await
                    .map_err(|error| format!("The password read failed: {error}"))?
                    .map_err(|error| {
                        format!("Could not read the database password from the vault: {error}")
                    })?;
                Some(value)
            }
        };
        let mut generated = crate::data_context::generate(&schema);
        generated.connection = crate::data_context::ConnectionInfo {
            kind: match connection.kind {
                DatabaseKind::Sqlite => "sqlite",
                DatabaseKind::Postgres => "postgres",
                DatabaseKind::SqlServer => "sqlserver",
            }
            .into(),
            path: connection.path.clone(),
            host: connection.host.clone(),
            port: connection.port,
            database: connection.database.clone(),
            user: connection.user.clone(),
            password,
            tls: match connection.tls {
                TlsMode::Prefer => "prefer",
                TlsMode::Require => "require",
                TlsMode::Disable => "disable",
            }
            .into(),
            trust_server_certificate: connection.trust_server_certificate,
        };
        Ok(generated)
    }
}
fn sqlite_schema(path: &str) -> Result<Schema, String> {
    let read = || -> rusqlite::Result<Schema> {
        let db =
            SqliteConnection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // Keep all catalog reads on one snapshot if another connection changes DDL.
        db.execute_batch("BEGIN")?;
        let mut tables = db.prepare("SELECT name,type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT GLOB 'sqlite_*' ORDER BY name")?;
        let names = tables
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = vec![];
        for (name, object_type) in names {
            let object = (|| -> rusqlite::Result<SchemaTable> {
                let mut columns = db.prepare(
                    "SELECT name,type,pk FROM pragma_table_xinfo(?1) WHERE hidden != 1 ORDER BY cid",
                )?;
                let mut columns = columns
                    .query_map([&name], |r| {
                        Ok(SchemaColumn {
                            name: r.get(0)?,
                            data_type: r.get(1)?,
                            references: vec![],
                            primary_key: r.get::<_, i64>(2)? > 0,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut keys = db.prepare(
                    "SELECT \"from\",\"table\",\"to\",seq,id FROM pragma_foreign_key_list(?1)",
                )?;
                let keys = keys
                    .query_map([&name], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                for (from, target, to, seq, id) in keys {
                    // An omitted target means the matching column of the parent's primary key.
                    let to = match to {
                        Some(to) => Some(to),
                        None => db
                            .query_row(
                                "SELECT name FROM pragma_table_info(?1) WHERE pk=?2",
                                rusqlite::params![target, seq + 1],
                                |r| r.get(0),
                            )
                            .optional()?,
                    };
                    let Some(to) = to else { continue };
                    if let Some(column) = columns.iter_mut().find(|c| c.name == from) {
                        column.references.push(ForeignKey {
                            schema: "main".into(),
                            table: target,
                            column: to,
                            constraint: format!("fk{id}"),
                            ordinal: seq.max(0) as u32,
                        });
                    }
                }
                Ok(SchemaTable {
                    schema: "main".into(),
                    name,
                    columns,
                })
            })();
            match object {
                Ok(table) => result.push(table),
                // Views can remain in sqlite_schema after their base table is dropped.
                Err(_) if object_type == "view" => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(Schema {
            kind: DatabaseKind::Sqlite,
            tables: result,
        })
    };
    read().map_err(|e| format!("Could not read the SQLite schema: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_catalog_handles_quoted_names_composite_keys_and_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.db");
        let db = SqliteConnection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE parent (a INTEGER,b TEXT,PRIMARY KEY(a,b)); CREATE TABLE \"child's table\" (x INTEGER,y TEXT, total INTEGER GENERATED ALWAYS AS (x+1), FOREIGN KEY(x,y) REFERENCES parent); CREATE VIEW names AS SELECT b FROM parent;").unwrap();
        let schema = sqlite_schema(path.to_str().unwrap()).unwrap();
        let child = schema
            .tables
            .iter()
            .find(|t| t.name == "child's table")
            .unwrap();
        assert_eq!(child.columns.len(), 3);
        assert_eq!(child.columns[0].data_type, "INTEGER");
        assert_eq!(child.columns[0].references[0].column, "a");
        assert_eq!(child.columns[1].references[0].column, "b");
        assert_eq!(
            child.columns[0].references[0].constraint,
            child.columns[1].references[0].constraint
        );
        assert!(child.columns[0].references[0].ordinal < child.columns[1].references[0].ordinal);
        let parent = schema.tables.iter().find(|t| t.name == "parent").unwrap();
        assert!(parent.columns[0].primary_key && parent.columns[1].primary_key);
        assert!(schema.tables.iter().any(|t| t.name == "names"));
        db.execute_batch("ALTER TABLE parent ADD COLUMN added BLOB")
            .unwrap();
        assert_eq!(
            sqlite_schema(path.to_str().unwrap())
                .unwrap()
                .tables
                .iter()
                .find(|t| t.name == "parent")
                .unwrap()
                .columns
                .len(),
            3
        );
        let missing = dir.path().join("missing.db");
        assert!(sqlite_schema(missing.to_str().unwrap()).is_err());
        assert!(!missing.exists());
    }
    #[test]
    fn sqlite_catalog_keeps_tables_with_unresolvable_implicit_foreign_keys() {
        for parent in ["", "CREATE TABLE parent(a INTEGER);"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dangling.db");
            let db = SqliteConnection::open(&path).unwrap();
            db.execute_batch(&format!("PRAGMA foreign_keys=OFF; CREATE TABLE ok(id INTEGER PRIMARY KEY); {parent} CREATE TABLE child(x INTEGER REFERENCES parent);"))
                .unwrap();
            let schema = sqlite_schema(path.to_str().unwrap()).unwrap();
            assert!(schema.tables.iter().any(|t| t.name == "ok"));
            let child = schema.tables.iter().find(|t| t.name == "child").unwrap();
            assert_eq!(child.columns[0].name, "x");
            assert!(child.columns[0].references.is_empty());
        }
    }

    #[test]
    fn sqlite_catalog_keeps_healthy_objects_when_a_view_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("view.db");
        let db = SqliteConnection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE ok(id INTEGER PRIMARY KEY); CREATE VIEW valid AS SELECT id FROM ok; CREATE TABLE gone(a); CREATE VIEW broken AS SELECT a FROM gone; DROP TABLE gone;").unwrap();
        let schema = sqlite_schema(path.to_str().unwrap()).unwrap();
        for name in ["ok", "valid"] {
            let table = schema.tables.iter().find(|t| t.name == name).unwrap();
            assert_eq!(table.columns[0].name, "id");
        }
        assert!(!schema.tables.iter().any(|t| t.name == "broken"));
    }

    #[test]
    fn sqlite_catalog_only_excludes_the_reserved_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("names.db");
        let db = SqliteConnection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE sqliteusers(id INTEGER PRIMARY KEY AUTOINCREMENT); CREATE TABLE ok(id INTEGER);").unwrap();
        let schema = sqlite_schema(path.to_str().unwrap()).unwrap();
        assert_eq!(
            schema
                .tables
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ok", "sqliteusers"]
        );
    }

    #[test]
    fn server_catalog_rows_preserve_namespaces_and_multiple_references() {
        for kind in [DatabaseKind::Postgres, DatabaseKind::SqlServer] {
            let row = |schema: &str, target: &str| {
                [
                    Some(schema.into()),
                    Some("items".into()),
                    Some("id".into()),
                    Some("int".into()),
                    Some("other".into()),
                    Some(target.into()),
                    Some("key".into()),
                    Some("1".into()),
                    Some("fk".into()),
                    Some("1".into()),
                ]
            };
            let schema = assemble(
                kind,
                vec![row("a", "one"), row("a", "two"), row("b", "three")],
            );
            assert_eq!(schema.tables.len(), 2);
            assert_eq!(schema.tables[0].columns.len(), 1);
            assert_eq!(schema.tables[0].columns[0].references.len(), 2);
            assert!(schema.tables[0].columns[0].primary_key);
        }
    }
}
