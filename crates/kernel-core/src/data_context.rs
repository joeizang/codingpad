//! Typed code generated from a connection's schema.
//!
//! Level 3 database access is generated, not hand-written: tables become
//! SQLAlchemy 2.0 `Mapped[]` models in Python and EF Core entities plus a
//! `DbContext` in C#. Foreign keys become relationships/navigation properties.
//! Generation is deterministic so a schema refresh only rewrites the file when
//! the structure actually changed.
#[cfg(test)]
use crate::databases::schema::SchemaColumn;
use crate::databases::schema::{ForeignKey, Schema, SchemaTable};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Host-side connection details handed to a kernel for a generated context.
///
/// Passwords are read from the OS vault at run time and never written to a
/// generated file or the script. They live only in the kernel's memory.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default = "default_tls")]
    pub tls: String,
    #[serde(default)]
    pub trust_server_certificate: bool,
}

fn default_tls() -> String {
    "prefer".into()
}

/// Generated text for one connection, with the connection details the kernel
/// needs to open it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneratedContext {
    pub python: String,
    pub csharp: String,
    pub connection: ConnectionInfo,
}

#[derive(Clone, Debug)]
struct ColumnShape {
    name: String,
    data_type: String,
    references: Vec<ForeignKey>,
    primary_key: bool,
}

#[derive(Clone, Debug)]
struct Relationship {
    /// The entity that owns the foreign key.
    child: String,
    /// Property on the child that points at the parent.
    child_property: String,
    /// Property on the parent that contains the children.
    parent_property: String,
    parent: String,
    target_table: String,
    /// Child columns in foreign-key order.
    columns: Vec<String>,
    /// Referenced parent columns in foreign-key order.
    target_columns: Vec<String>,
}

#[derive(Clone, Debug)]
struct TableShape {
    schema: String,
    name: String,
    class_name: String,
    table_args: Option<String>,
    columns: Vec<ColumnShape>,
    relationships: Vec<Relationship>,
}

/// Render Python (SQLAlchemy 2.0) and C# (EF Core) data contexts.
pub fn generate(schema: &Schema) -> GeneratedContext {
    let tables = shape(schema);
    GeneratedContext {
        python: python_models(&tables),
        csharp: csharp_models(&tables, schema.kind),
        ..GeneratedContext::default()
    }
}

#[cfg(test)]
fn sample_schema() -> Schema {
    use crate::databases::DatabaseKind;
    Schema {
        kind: DatabaseKind::Sqlite,
        tables: vec![
            SchemaTable {
                schema: "main".into(),
                name: "customers".into(),
                columns: vec![
                    SchemaColumn {
                        name: "id".into(),
                        data_type: "INTEGER".into(),
                        references: vec![],
                        primary_key: true,
                    },
                    SchemaColumn {
                        name: "name".into(),
                        data_type: "TEXT".into(),
                        references: vec![],
                        primary_key: false,
                    },
                ],
            },
            SchemaTable {
                schema: "main".into(),
                name: "orders".into(),
                columns: vec![
                    SchemaColumn {
                        name: "id".into(),
                        data_type: "INTEGER".into(),
                        references: vec![],
                        primary_key: true,
                    },
                    SchemaColumn {
                        name: "customer_id".into(),
                        data_type: "INTEGER".into(),
                        references: vec![ForeignKey {
                            schema: "main".into(),
                            table: "customers".into(),
                            column: "id".into(),
                            constraint: "fk1".into(),
                            ordinal: 1,
                        }],
                        primary_key: false,
                    },
                    SchemaColumn {
                        name: "total".into(),
                        data_type: "DECIMAL(10,2)".into(),
                        references: vec![],
                        primary_key: false,
                    },
                ],
            },
        ],
    }
}

fn shape(schema: &Schema) -> Vec<TableShape> {
    let mut class_names = BTreeSet::new();
    let mut tables = Vec::with_capacity(schema.tables.len());
    for table in &schema.tables {
        let class_name = unique_class_name(&table.name, &mut class_names);
        let columns = table
            .columns
            .iter()
            .map(|column| ColumnShape {
                name: column.name.clone(),
                data_type: column.data_type.clone(),
                references: column.references.clone(),
                primary_key: column.primary_key,
            })
            .collect::<Vec<_>>();
        let mut columns = columns;
        if !columns.iter().any(|column| column.primary_key) {
            mark_primary_keys(&table.name, &mut columns);
        }
        tables.push(TableShape {
            schema: table.schema.clone(),
            name: table.name.clone(),
            class_name,
            table_args: python_schema_args(schema, table),
            columns,
            relationships: Vec::new(),
        });
    }

    let mut relationships = Vec::new();
    for (index, table) in schema.tables.iter().enumerate() {
        let child = tables[index].class_name.clone();
        for column in &table.columns {
            for reference in &column.references {
                let Some(parent_index) = tables.iter().position(|candidate| {
                    candidate.name == reference.table
                        && same_schema(&candidate.schema, &reference.schema)
                }) else {
                    continue;
                };
                let parent = tables[parent_index].class_name.clone();
                if child == parent {
                    // Self-references still get a navigation property, but the
                    // collection name needs a distinct suffix.
                }
                let child_property = lower_snake(&parent);
                let parent_property = plural_snake(&child);
                relationships.push(Relationship {
                    child: child.clone(),
                    child_property,
                    parent_property,
                    parent,
                    target_table: reference.table.clone(),
                    columns: vec![column.name.clone()],
                    target_columns: vec![reference.column.clone()],
                });
            }
        }
    }

    // Merge single-column references that share a composite foreign key. The
    // schema answers one row per column pair, so a composite key (a, b) -> (x, y)
    // arrives as two identical parent/child pairs with different columns.
    let mut merged: Vec<Relationship> = Vec::new();
    for relationship in relationships {
        if let Some(existing) = merged.iter_mut().find(|candidate| {
            candidate.child == relationship.child && candidate.parent == relationship.parent
        }) {
            existing.columns.push(relationship.columns[0].clone());
            existing
                .target_columns
                .push(relationship.target_columns[0].clone());
        } else {
            merged.push(relationship);
        }
    }
    for relationship in merged {
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.class_name == relationship.child)
        {
            table.relationships.push(relationship.clone());
        }
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.class_name == relationship.parent)
        {
            // The parent needs the reverse collection property; it is rendered
            // from the child's relationship, so no extra copy is stored here.
            let _ = table;
        }
    }

    tables
}

fn same_schema(a: &str, b: &str) -> bool {
    a.is_empty() || b.is_empty() || a == b
}

fn mark_primary_keys(table_name: &str, columns: &mut [ColumnShape]) {
    if columns.is_empty() {
        return;
    }
    let lower_table = table_name.to_ascii_lowercase();
    let singular = singularize(&lower_table);
    let candidate = columns.iter().position(|column| {
        let name = column.name.to_ascii_lowercase();
        name == "id" || name == format!("{singular}_id") || name == format!("{lower_table}_id")
    });
    let index = candidate.unwrap_or(0);
    columns[index].primary_key = true;
}

fn python_schema_args(schema: &Schema, table: &SchemaTable) -> Option<String> {
    use crate::databases::DatabaseKind;
    let default = match schema.kind {
        DatabaseKind::Sqlite => "main",
        DatabaseKind::Postgres => "public",
        DatabaseKind::SqlServer => "dbo",
    };
    if table.schema.is_empty() || table.schema == default {
        None
    } else {
        Some(format!("{{\"schema\": {}}}", py_string(&table.schema)))
    }
}

fn python_models(tables: &[TableShape]) -> String {
    let mut reverse = BTreeMap::<String, Vec<&Relationship>>::new();
    for table in tables {
        for relationship_shape in &table.relationships {
            reverse
                .entry(relationship_shape.parent.clone())
                .or_default()
                .push(relationship_shape);
        }
    }

    let mut out = String::from(
        "# Generated by CodingPad from the attached connection's schema.\n\
         # Re-run the script or refresh the schema to regenerate this file.\n\
         # Do not edit by hand.\n\
         from __future__ import annotations\n\n\
         from datetime import date, datetime, time\n\
         from decimal import Decimal\n\
         from typing import Any\n\
         from uuid import UUID\n\n\
         from sqlalchemy import Boolean, Date, DateTime, Float, ForeignKey, ForeignKeyConstraint, Integer, JSON, LargeBinary, Numeric, String, Text, Time, Uuid\n\
         from sqlalchemy.orm import DeclarativeBase, Mapped, Session, mapped_column, relationship\n\n\n\
         class Base(DeclarativeBase):\n    pass\n\n\n",
    );
    for table in tables {
        out.push_str("class ");
        out.push_str(&table.class_name);
        out.push_str("(Base):\n");
        out.push_str(&format!("    __tablename__ = {}\n", py_string(&table.name)));
        if let Some(args) = python_table_args(table, tables) {
            out.push_str(&format!("    __table_args__ = {args}\n"));
        }
        if table.columns.is_empty() {
            out.push_str("    pass\n\n");
            continue;
        }
        let composite: BTreeSet<String> = table
            .relationships
            .iter()
            .filter(|relationship_shape| relationship_shape.columns.len() > 1)
            .flat_map(|relationship_shape| relationship_shape.columns.iter().cloned())
            .collect();
        for column in &table.columns {
            let (python_type, sqlalchemy_type) = python_column(column);
            let attribute = py_identifier(&column.name);
            let args = python_column_args(table, column, &sqlalchemy_type, &composite);
            out.push_str(&format!(
                "    {attribute}: Mapped[{python_type}] = mapped_column({args})\n"
            ));
        }
        for relationship_shape in &table.relationships {
            let foreign_keys = if relationship_shape.columns.len() > 1 {
                format!(
                    ", foreign_keys=[{}]",
                    relationship_shape
                        .columns
                        .iter()
                        .map(|column| py_identifier(column))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            };
            out.push_str(&format!(
                "    {}: Mapped[{} | None] = relationship(back_populates={}{foreign_keys})\n",
                relationship_shape.child_property,
                relationship_shape.parent,
                py_string(&relationship_shape.parent_property)
            ));
        }
        if let Some(children) = reverse.get(&table.class_name) {
            for relationship_shape in children {
                out.push_str(&format!(
                    "    {}: Mapped[list[{}]] = relationship(back_populates={})\n",
                    relationship_shape.parent_property,
                    relationship_shape.child,
                    py_string(&relationship_shape.child_property)
                ));
            }
        }
        out.push('\n');
    }
    out.push_str(&data_context_python(tables));
    out
}

fn data_context_python(tables: &[TableShape]) -> String {
    let mut out = String::from(
        "\nclass DataContext:\n    \"\"\"Typed tables for the attached connection.\"\"\"\n\n",
    );
    out.push_str("    def __init__(self, session: Session):\n        self.session = session\n\n    @classmethod\n    def connect(cls) -> \"DataContext\":\n        from codingpad import _connect_data_context\n        return cls(_connect_data_context())\n\n");
    for table in tables {
        out.push_str(&format!(
            "    {}: type[{}] = {}\n",
            table.class_name, table.class_name, table.class_name
        ));
    }
    out.push('\n');
    out.push_str("__all__ = [\n    \"Base\",\n    \"DataContext\",\n");
    for table in tables {
        out.push_str(&format!("    {},\n", py_string(&table.class_name)));
    }
    out.push_str("]\n");
    out
}

fn python_table_args(table: &TableShape, tables: &[TableShape]) -> Option<String> {
    let constraints = table
        .relationships
        .iter()
        .filter(|relationship_shape| relationship_shape.columns.len() > 1)
        .map(|relationship_shape| {
            let columns = relationship_shape
                .columns
                .iter()
                .map(|column| py_string(column))
                .collect::<Vec<_>>()
                .join(", ");
            let parent = tables
                .iter()
                .find(|candidate| candidate.class_name == relationship_shape.parent);
            let targets = relationship_shape
                .target_columns
                .iter()
                .map(|column| {
                    let mut parts = Vec::new();
                    if let Some(parent) = parent {
                        let default =
                            matches!(parent.schema.as_str(), "main" | "public" | "dbo" | "");
                        if !default {
                            parts.push(parent.schema.clone());
                        }
                        parts.push(parent.name.clone());
                    } else {
                        parts.push(relationship_shape.target_table.clone());
                    }
                    parts.push(column.clone());
                    py_string(&parts.join("."))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("ForeignKeyConstraint([{columns}], [{targets}])")
        })
        .collect::<Vec<_>>();
    match (constraints.is_empty(), table.table_args.as_ref()) {
        (true, None) => None,
        (true, Some(args)) => Some(args.clone()),
        (false, None) => Some(format!("({},)", constraints.join(", "))),
        (false, Some(args)) => {
            let mut items = constraints;
            items.push(args.clone());
            Some(format!("({},)", items.join(", ")))
        }
    }
}

fn python_column(column: &ColumnShape) -> (String, String) {
    let kind = base_type(&column.data_type);
    let (python, sqlalchemy) = match kind.as_str() {
        "smallint" | "integer" | "bigint" | "int" | "int2" | "int4" | "int8" | "serial"
        | "bigserial" | "smallserial" | "tinyint" => ("int".into(), "Integer".into()),
        "numeric" | "decimal" | "money" | "smallmoney" | "number" => {
            ("Decimal".into(), "Numeric".into())
        }
        "real" | "float" | "float4" | "float8" | "double" | "double precision" => {
            ("float".into(), "Float".into())
        }
        "boolean" | "bool" | "bit" => ("bool".into(), "Boolean".into()),
        "blob" | "bytea" | "binary" | "varbinary" | "image" | "rowversion" => {
            ("bytes".into(), "LargeBinary".into())
        }
        "date" => ("date".into(), "Date".into()),
        "time" | "time without time zone" | "time with time zone" => ("time".into(), "Time".into()),
        "timestamp"
        | "timestamp without time zone"
        | "timestamptz"
        | "timestamp with time zone"
        | "datetime"
        | "datetime2"
        | "datetimeoffset"
        | "smalldatetime" => ("datetime".into(), "DateTime".into()),
        "uuid" | "uniqueidentifier" => ("UUID".into(), "Uuid".into()),
        "json" | "jsonb" => ("dict[str, Any]".into(), "JSON".into()),
        "text" | "ntext" | "varchar" | "nvarchar" | "char" | "nchar" | "character varying"
        | "character" | "citext" | "xml" | "name" | "interval" => ("str".into(), "Text".into()),
        _ => ("Any".into(), "Text".into()),
    };
    if column.primary_key {
        (python, sqlalchemy)
    } else {
        (format!("{python} | None"), sqlalchemy)
    }
}

fn python_column_args(
    table: &TableShape,
    column: &ColumnShape,
    sqlalchemy_type: &str,
    composite: &BTreeSet<String>,
) -> String {
    let mut args = Vec::new();
    args.push(sqlalchemy_type.to_owned());
    if column.primary_key {
        args.push("primary_key=True".into());
    } else if !column.references.is_empty() && !composite.contains(&column.name) {
        let target = &column.references[0];
        let mut parts = Vec::new();
        if !target.schema.is_empty() && target.schema != table.schema {
            parts.push(target.schema.clone());
        }
        parts.push(target.table.clone());
        parts.push(target.column.clone());
        args.push(format!("ForeignKey({})", py_string(&parts.join("."))));
    }
    if !column.primary_key {
        args.push("nullable=True".into());
    }
    args.join(", ")
}

fn csharp_models(tables: &[TableShape], kind: crate::databases::DatabaseKind) -> String {
    use crate::databases::DatabaseKind;
    let mut out = String::from(
        "// <auto-generated>\n\
         // Generated by CodingPad from the attached connection's schema.\n\
         // Refresh the schema to regenerate this file. Do not edit by hand.\n\
         // </auto-generated>\n\
         #nullable enable\n\
         global using CodingPad.Data;\n\
         using System;\n\
         using System.Collections.Generic;\n\
         using System.Text.Json;\n\
         using Microsoft.EntityFrameworkCore;\n\n\
         namespace CodingPad.Data;\n\n",
    );
    for table in tables {
        out.push_str(&format!("public sealed class {}\n{{\n", table.class_name));
        for column in &table.columns {
            let (cs_type, attribute) = csharp_column(column);
            out.push_str(&format!(
                "    public {cs_type} {attribute} {{ get; set; }}\n"
            ));
        }
        for relationship_shape in &table.relationships {
            out.push_str(&format!(
                "    public {}? {} {{ get; set; }}\n",
                relationship_shape.parent,
                csharp_identifier(&relationship_shape.child_property)
            ));
        }
        // Reverse collections, one per child relationship.
        let mut seen = BTreeSet::new();
        for child in tables {
            for relationship_shape in &child.relationships {
                if relationship_shape.parent == table.class_name
                    && seen.insert(relationship_shape.parent_property.clone())
                {
                    out.push_str(&format!(
                        "    public List<{}> {} {{ get; set; }} = new();\n",
                        relationship_shape.child,
                        csharp_identifier(&relationship_shape.parent_property)
                    ));
                }
            }
        }
        out.push_str("}\n\n");
    }

    out.push_str("public sealed class DataContext : DbContext\n{\n");
    for table in tables {
        out.push_str(&format!(
            "    public DbSet<{}> {} => Set<{}>();\n",
            table.class_name,
            plural_pascal(&table.class_name),
            table.class_name
        ));
    }
    out.push('\n');
    out.push_str(
        "    protected override void OnConfiguring(DbContextOptionsBuilder options)\n    {\n\
             var json = Environment.GetEnvironmentVariable(\"CODINGPAD_DATABASE_CONNECTION\");\n\
             if (string.IsNullOrWhiteSpace(json))\n\
                 throw new InvalidOperationException(\"No database connection is attached. Add one in Connections… and name it in the script header.\");\n\
             using var document = JsonDocument.Parse(json);\n\
             var config = document.RootElement;\n",
    );
    match kind {
        DatabaseKind::Sqlite => out.push_str(
            "        var builder = new Microsoft.Data.Sqlite.SqliteConnectionStringBuilder\n        {\n            DataSource = Text(config, \"path\"),\n        };\n        options.UseSqlite(builder.ConnectionString);\n",
        ),
        DatabaseKind::Postgres => out.push_str(
            "        var builder = new Npgsql.NpgsqlConnectionStringBuilder\n        {\n            Host = Text(config, \"host\"),\n            Port = Number(config, \"port\") ?? 5432,\n            Database = Text(config, \"database\"),\n            Username = Text(config, \"user\"),\n            Password = Text(config, \"password\"),\n            SslMode = Enum.TryParse<Npgsql.SslMode>(Text(config, \"tls\"), true, out var sslMode) ? sslMode : Npgsql.SslMode.Prefer,\n            TrustServerCertificate = Bool(config, \"trustServerCertificate\"),\n        };\n        options.UseNpgsql(builder.ConnectionString);\n",
        ),
        DatabaseKind::SqlServer => out.push_str(
            "        var builder = new Microsoft.Data.SqlClient.SqlConnectionStringBuilder\n        {\n            DataSource = Text(config, \"host\") + \",\" + (Number(config, \"port\") ?? 1433),\n            InitialCatalog = Text(config, \"database\"),\n            UserID = Text(config, \"user\"),\n            Password = Text(config, \"password\"),\n            TrustServerCertificate = Bool(config, \"trustServerCertificate\"),\n        };\n        builder[\"Encrypt\"] = Text(config, \"tls\") == \"disable\" ? \"Optional\" : \"Mandatory\";\n        options.UseSqlServer(builder.ConnectionString);\n",
        ),
    }
    out.push_str(
        "    }\n\n\
         private static string Text(JsonElement config, string name) =>\n\
             config.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String\n\
                 ? value.GetString() ?? \"\"\n\
                 : \"\";\n\n\
         private static int? Number(JsonElement config, string name) =>\n\
             config.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.Number && value.TryGetInt32(out var number)\n\
                 ? number\n\
                 : null;\n\n\
         private static bool Bool(JsonElement config, string name) =>\n\
             config.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.True;\n\n\
         protected override void OnModelCreating(ModelBuilder modelBuilder)\n    {\n",
    );
    for table in tables {
        out.push_str(&format!(
            "        modelBuilder.Entity<{}>(entity =>\n        {{\n",
            table.class_name
        ));
        let schema = match kind {
            DatabaseKind::Sqlite => None,
            DatabaseKind::Postgres if table.schema.is_empty() || table.schema == "public" => None,
            DatabaseKind::SqlServer if table.schema.is_empty() || table.schema == "dbo" => None,
            _ => Some(table.schema.as_str()),
        };
        match schema {
            Some(schema) => out.push_str(&format!(
                "            entity.ToTable({}, {});\n",
                cs_string(&table.name),
                cs_string(schema)
            )),
            None => out.push_str(&format!(
                "            entity.ToTable({});\n",
                cs_string(&table.name)
            )),
        }
        let keys = table
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| csharp_identifier(&column.name))
            .collect::<Vec<_>>();
        if keys.len() == 1 {
            out.push_str(&format!("            entity.HasKey(e => e.{});\n", keys[0]));
        } else if keys.len() > 1 {
            out.push_str(&format!(
                "            entity.HasKey(e => new {{ {} }});\n",
                keys.iter()
                    .map(|key| format!("e.{key}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for column in &table.columns {
            let property = csharp_identifier(&column.name);
            if property != column.name {
                out.push_str(&format!(
                    "            entity.Property(e => e.{property}).HasColumnName({});\n",
                    cs_string(&column.name)
                ));
            }
        }
        for relationship_shape in &table.relationships {
            let child_property = csharp_identifier(&relationship_shape.child_property);
            let parent_property = csharp_identifier(&relationship_shape.parent_property);
            let foreign_key = if relationship_shape.columns.len() == 1 {
                format!("e.{}", csharp_identifier(&relationship_shape.columns[0]))
            } else {
                format!(
                    "new {{ {} }}",
                    relationship_shape
                        .columns
                        .iter()
                        .map(|column| format!("e.{}", csharp_identifier(column)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            out.push_str(&format!(
                "            entity.HasOne(e => e.{child_property}).WithMany(e => e.{parent_property}).HasForeignKey(e => {foreign_key});\n"
            ));
        }
        out.push_str("        });\n");
    }
    out.push_str("    }\n}\n");
    out
}

fn csharp_column(column: &ColumnShape) -> (String, String) {
    let kind = base_type(&column.data_type);
    let base = match kind.as_str() {
        "smallint" | "smallserial" => "short",
        "tinyint" => "byte",
        "integer" | "int" | "int4" | "serial" => "int",
        "bigint" | "int8" | "bigserial" => "long",
        "numeric" | "decimal" | "money" | "smallmoney" | "number" => "decimal",
        "real" | "float4" => "float",
        "float" | "float8" | "double" | "double precision" => "double",
        "boolean" | "bool" | "bit" => "bool",
        "blob" | "bytea" | "binary" | "varbinary" | "image" | "rowversion" => "byte[]",
        "date" => "DateOnly",
        "time" | "time without time zone" | "time with time zone" => "TimeOnly",
        "timestamp"
        | "timestamp without time zone"
        | "datetime"
        | "datetime2"
        | "smalldatetime" => "DateTime",
        "timestamptz" | "timestamp with time zone" | "datetimeoffset" => "DateTimeOffset",
        "uuid" | "uniqueidentifier" => "Guid",
        "json" | "jsonb" => "JsonDocument",
        _ => "string",
    };
    let cs_type = if column.primary_key {
        base.to_owned()
    } else {
        format!("{base}?")
    };
    (cs_type, csharp_identifier(&column.name))
}

fn base_type(data_type: &str) -> String {
    data_type
        .trim()
        .trim_end_matches("[]")
        .split(['(', ' '])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn unique_class_name(name: &str, used: &mut BTreeSet<String>) -> String {
    let base = pascal_case(&singularize(name));
    if used.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn singularize(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "children" => return "child".into(),
        "people" => return "person".into(),
        "mice" => return "mouse".into(),
        _ => {}
    }
    if lower.ends_with("ies") && lower.len() > 3 {
        return format!("{}y", &name[..name.len() - 3]);
    }
    for suffix in ["ches", "shes", "xes", "zes", "ses"] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return name[..name.len() - 2].to_owned();
        }
    }
    if lower.ends_with('s') && !lower.ends_with("ss") && lower.len() > 1 {
        return name[..name.len() - 1].to_owned();
    }
    name.to_owned()
}

fn plural_pascal(class_name: &str) -> String {
    pascal_case(&plural_snake(class_name))
}

fn plural_snake(class_name: &str) -> String {
    let snake = lower_snake(class_name);
    match snake.as_str() {
        "child" => "children".into(),
        "person" => "people".into(),
        "mouse" => "mice".into(),
        _ if snake.ends_with('s') => format!("{snake}_items"),
        _ if snake.ends_with('y') => format!("{}ies", &snake[..snake.len() - 1]),
        _ => format!("{snake}s"),
    }
}

fn lower_snake(value: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for character in value.chars() {
        if character == '_' || character == '-' || character == ' ' {
            if !out.ends_with('_') {
                out.push('_');
            }
            previous_lower = false;
        } else if character.is_ascii_uppercase() {
            if previous_lower && !out.ends_with('_') {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
            previous_lower = false;
        } else {
            out.push(character);
            previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
        }
    }
    let out = out.trim_matches('_').to_owned();
    if out.is_empty() {
        "table".into()
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

fn pascal_case(value: &str) -> String {
    let mut out = String::new();
    let mut uppercase = true;
    for character in value.chars() {
        if character == '_' || character == '-' || character == ' ' {
            uppercase = true;
        } else if uppercase {
            out.extend(character.to_uppercase());
            uppercase = false;
        } else {
            out.push(character);
        }
    }
    if out.is_empty() {
        "Table".into()
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

fn py_identifier(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() || character == '_' {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if matches!(
        out.as_str(),
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    ) {
        out.push('_');
    }
    out
}

fn csharp_identifier(value: &str) -> String {
    let pascal = pascal_case(value);
    if matches!(
        pascal.as_str(),
        "Class"
            | "Event"
            | "Object"
            | "String"
            | "Int"
            | "Decimal"
            | "Double"
            | "Float"
            | "Bool"
            | "Base"
            | "Namespace"
            | "Default"
            | "Operator"
            | "Params"
            | "Struct"
            | "Interface"
            | "Enum"
            | "Delegate"
            | "Abstract"
            | "Virtual"
            | "Override"
            | "This"
            | "New"
            | "Public"
            | "Private"
            | "Protected"
            | "Internal"
            | "Static"
            | "Readonly"
            | "Const"
    ) {
        format!("@{pascal}")
    } else {
        pascal
    }
}

fn py_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("'{escaped}'")
}

fn cs_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_models_use_mapped_annotations_and_relationships() {
        let generated = generate(&sample_schema());
        let python = generated.python;
        assert!(python.contains("class Customer(Base):"));
        assert!(python.contains("__tablename__ = 'customers'"));
        assert!(python.contains("id: Mapped[int] = mapped_column(Integer, primary_key=True)"));
        assert!(python
            .contains("customer: Mapped[Customer | None] = relationship(back_populates='orders')"));
        assert!(python
            .contains("orders: Mapped[list[Order]] = relationship(back_populates='customer')"));
        assert!(python.contains("class DataContext:"));
    }

    #[test]
    fn csharp_models_use_navigation_properties_and_keys() {
        let generated = generate(&sample_schema());
        let csharp = generated.csharp;
        assert!(csharp.contains("public sealed class Customer"));
        assert!(csharp.contains("public sealed class DataContext : DbContext"));
        assert!(csharp.contains("public DbSet<Customer> Customers => Set<Customer>();"));
        assert!(csharp.contains("entity.HasKey(e => e.Id);"));
        assert!(csharp.contains("entity.HasOne(e => e.Customer).WithMany(e => e.Orders).HasForeignKey(e => e.CustomerId);"));
    }

    #[test]
    fn identifiers_are_safe_and_generation_is_stable() {
        let mut schema = sample_schema();
        schema.tables[0].columns.push(SchemaColumn {
            name: "select".into(),
            data_type: "character varying(20)".into(),
            references: vec![],
            primary_key: false,
        });
        let first = generate(&schema);
        let second = generate(&schema);
        assert_eq!(first.python, second.python);
        assert_eq!(first.csharp, second.csharp);
        assert!(first.python.contains("select: Mapped[str | None]"));
        assert!(first.csharp.contains("public string? Select { get; set; }"));
    }

    #[test]
    fn composite_foreign_keys_merge_into_one_relationship() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![
                SchemaTable {
                    schema: "public".into(),
                    name: "parents".into(),
                    columns: vec![
                        SchemaColumn {
                            name: "a".into(),
                            data_type: "integer".into(),
                            references: vec![],
                            primary_key: true,
                        },
                        SchemaColumn {
                            name: "b".into(),
                            data_type: "integer".into(),
                            references: vec![],
                            primary_key: true,
                        },
                    ],
                },
                SchemaTable {
                    schema: "public".into(),
                    name: "children".into(),
                    columns: vec![
                        SchemaColumn {
                            name: "id".into(),
                            data_type: "integer".into(),
                            references: vec![],
                            primary_key: true,
                        },
                        SchemaColumn {
                            name: "parent_a".into(),
                            data_type: "integer".into(),
                            references: vec![ForeignKey {
                                schema: "public".into(),
                                table: "parents".into(),
                                column: "a".into(),
                                constraint: "fk1".into(),
                                ordinal: 1,
                            }],
                            primary_key: false,
                        },
                        SchemaColumn {
                            name: "parent_b".into(),
                            data_type: "integer".into(),
                            references: vec![ForeignKey {
                                schema: "public".into(),
                                table: "parents".into(),
                                column: "b".into(),
                                constraint: "fk1".into(),
                                ordinal: 2,
                            }],
                            primary_key: false,
                        },
                    ],
                },
            ],
        };
        let generated = generate(&schema);
        assert_eq!(
            generated
                .python
                .matches("children: Mapped[list[Child]]")
                .count(),
            1
        );
        assert_eq!(
            generated
                .python
                .matches("children: Mapped[list[Child]] = relationship(back_populates='parent')")
                .count(),
            1
        );
        assert_eq!(
            generated
                .python
                .matches("parent: Mapped[Parent | None] = relationship(back_populates='children', foreign_keys=[parent_a, parent_b])")
                .count(),
            1
        );
        assert!(generated.python.contains(
            "ForeignKeyConstraint(['parent_a', 'parent_b'], ['parents.a', 'parents.b'])"
        ));
    }
}
