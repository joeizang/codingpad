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
    identity: bool,
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
    /// Primary-key columns of the parent for a self-reference.
    remote_columns: Vec<String>,
}

type ForeignKeyGroups = BTreeMap<(String, String, String), Vec<(u32, String, String)>>;

#[derive(Clone, Debug)]
struct TableShape {
    schema: String,
    name: String,
    class_name: String,
    table_args: Option<String>,
    columns: Vec<ColumnShape>,
    relationships: Vec<Relationship>,
    no_primary_key: bool,
}

/// Render Python (SQLAlchemy 2.0) and C# (EF Core) data contexts.
pub fn generate(schema: &Schema) -> GeneratedContext {
    let tables = shape(schema);
    GeneratedContext {
        python: python_models(&tables, schema.kind),
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
                        identity: true,
                    },
                    SchemaColumn {
                        name: "name".into(),
                        data_type: "TEXT".into(),
                        references: vec![],
                        primary_key: false,
                        identity: false,
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
                        identity: true,
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
                        identity: false,
                    },
                    SchemaColumn {
                        name: "total".into(),
                        data_type: "DECIMAL(10,2)".into(),
                        references: vec![],
                        primary_key: false,
                        identity: false,
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
                identity: column.identity,
            })
            .collect::<Vec<_>>();
        let no_primary_key = !columns.iter().any(|column| column.primary_key);
        tables.push(TableShape {
            schema: table.schema.clone(),
            name: table.name.clone(),
            class_name,
            table_args: python_schema_args(schema, table),
            columns,
            relationships: Vec::new(),
            no_primary_key,
        });
    }

    let mut used_properties = BTreeMap::<String, BTreeSet<String>>::new();
    for table in &tables {
        used_properties.insert(
            table.class_name.clone(),
            table
                .columns
                .iter()
                .map(|column| py_identifier(&column.name))
                .collect(),
        );
    }

    for (index, table) in schema.tables.iter().enumerate() {
        if tables[index].no_primary_key {
            continue;
        }
        let child = tables[index].class_name.clone();
        let mut groups: ForeignKeyGroups = BTreeMap::new();
        for column in &table.columns {
            for reference in &column.references {
                let identity = if reference.constraint.trim().is_empty() {
                    format!("{}.{}.{}", reference.schema, reference.table, column.name)
                } else {
                    reference.constraint.clone()
                };
                groups
                    .entry((reference.schema.clone(), reference.table.clone(), identity))
                    .or_default()
                    .push((
                        reference.ordinal,
                        column.name.clone(),
                        reference.column.clone(),
                    ));
            }
        }

        for ((target_schema, target_table, _), mut members) in groups {
            let Some(parent_index) = tables.iter().position(|candidate| {
                candidate.name == target_table && same_schema(&candidate.schema, &target_schema)
            }) else {
                continue;
            };
            if tables[parent_index].no_primary_key {
                continue;
            }
            members.sort_by_key(|(ordinal, _, _)| *ordinal);
            let parent = tables[parent_index].class_name.clone();
            let first_column = &members[0].1;
            let base = if members.len() == 1 {
                foreign_property(first_column, &parent)
            } else {
                lower_snake(&parent)
            };
            let child_property = unique_property(
                &base,
                used_properties.get_mut(&child).expect("child properties"),
            );
            let parent_base = plural_snake(&child);
            let parent_property = unique_property(
                &parent_base,
                used_properties.get_mut(&parent).expect("parent properties"),
            );
            let remote_columns = if child == parent {
                members
                    .iter()
                    .map(|(_, _, target)| target.clone())
                    .collect()
            } else {
                Vec::new()
            };
            let relationship = Relationship {
                child: child.clone(),
                child_property,
                parent_property,
                parent: parent.clone(),
                target_table: target_table.clone(),
                columns: members
                    .iter()
                    .map(|(_, column, _)| column.clone())
                    .collect(),
                target_columns: members
                    .iter()
                    .map(|(_, _, target)| target.clone())
                    .collect(),
                remote_columns,
            };
            tables[index].relationships.push(relationship);
        }
    }

    tables
}

fn foreign_property(column: &str, parent: &str) -> String {
    let lower = column.to_ascii_lowercase();
    for suffix in ["_id", "_key"] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return lower_snake(&column[..column.len() - suffix.len()]);
        }
    }
    lower_snake(parent)
}

fn unique_property(base: &str, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.to_owned()) {
        return base.to_owned();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn same_schema(a: &str, b: &str) -> bool {
    a.is_empty() || b.is_empty() || a == b
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

fn python_models(tables: &[TableShape], kind: crate::databases::DatabaseKind) -> String {
    let mut reverse = BTreeMap::<String, Vec<&Relationship>>::new();
    for table in tables {
        for relationship_shape in &table.relationships {
            reverse
                .entry(relationship_shape.parent.clone())
                .or_default()
                .push(relationship_shape);
        }
    }

    let mut out = String::new();
    out.push_str("# Generated by CodingPad from the attached connection's schema.\n");
    out.push_str("# Re-run the script or refresh the schema to regenerate this file.\n");
    out.push_str("# Do not edit by hand.\n");
    out.push_str("# Tables without a primary key use every column as the mapper key; a row\n");
    out.push_str("# whose columns are all NULL may be returned as None.\n");
    out.push_str("from __future__ import annotations\n\n");
    out.push_str("import datetime as _datetime\n");
    out.push_str("import decimal as _decimal\n");
    out.push_str("import ipaddress as _ipaddress\n");
    out.push_str("import typing as _typing\n");
    out.push_str("import uuid as _uuid\n\n");
    out.push_str("import sqlalchemy as _sa\n");
    out.push_str("import sqlalchemy.orm as _orm\n");
    out.push_str("from sqlalchemy.dialects import postgresql as _pg\n\n\n");
    out.push_str("class _MSSQLUuid(_sa.types.TypeDecorator):\n");
    out.push_str("    impl = _sa.String(36)\n");
    out.push_str("    cache_ok = True\n\n");
    out.push_str("    def process_bind_param(self, value, dialect):\n");
    out.push_str("        return str(value) if value is not None else None\n\n");
    out.push_str("    def process_result_value(self, value, dialect):\n");
    out.push_str("        return _uuid.UUID(str(value)) if value is not None else None\n\n\n");
    out.push_str("def _parse_pg_array(value):\n");
    out.push_str("    if value is None or not isinstance(value, str):\n");
    out.push_str("        return value\n");
    out.push_str("    if not (value.startswith('{') and value.endswith('}')):\n");
    out.push_str("        return value\n");
    out.push_str("    if value == '{}':\n        return []\n");
    out.push_str("    items, current, quoted, escaped, was_quoted = [], [], False, False, False\n");
    out.push_str("    for character in value[1:-1]:\n");
    out.push_str("        if escaped:\n");
    out.push_str("            current.append(character); escaped = False\n");
    out.push_str("        elif character == '\\\\':\n");
    out.push_str("            escaped = True\n");
    out.push_str("        elif character == '\"':\n");
    out.push_str("            quoted = not quoted; was_quoted = True\n");
    out.push_str("        elif character == ',' and not quoted:\n");
    out.push_str("            text = ''.join(current)\n");
    out.push_str("            items.append(None if text == 'NULL' and not was_quoted else text)\n");
    out.push_str("            current, was_quoted = [], False\n");
    out.push_str("        else:\n");
    out.push_str("            current.append(character)\n");
    out.push_str("    text = ''.join(current)\n");
    out.push_str("    items.append(None if text == 'NULL' and not was_quoted else text)\n");
    out.push_str("    return items\n\n\n");
    out.push_str("class _PostgresArrayType(_sa.types.UserDefinedType):\n");
    out.push_str("    cache_ok = True\n\n");
    out.push_str("    def __init__(self, type_name):\n        self.type_name = type_name\n\n");
    out.push_str("    def get_col_spec(self, **kw):\n        return self.type_name + '[]'\n\n\n");
    out.push_str("class _PostgresEnumArray(_sa.types.TypeDecorator):\n");
    out.push_str("    impl = _sa.Text\n");
    out.push_str("    cache_ok = True\n\n");
    out.push_str("    def __init__(self, type_name):\n");
    out.push_str("        self.type_name = type_name\n");
    out.push_str("        super().__init__()\n\n");
    out.push_str("    def bind_expression(self, bindvalue):\n");
    out.push_str("        return _sa.cast(bindvalue, _PostgresArrayType(self.type_name))\n\n");
    out.push_str("    def process_result_value(self, value, dialect):\n");
    out.push_str("        return _parse_pg_array(value)\n\n\n");
    out.push_str("class _PostgresMoney(_sa.types.TypeDecorator):\n");
    out.push_str("    impl = _sa.Numeric\n    cache_ok = True\n\n");
    out.push_str("    def __init__(self, array=False):\n        self.array = array\n        super().__init__()\n\n");
    out.push_str("    def load_dialect_impl(self, dialect):\n        return dialect.type_descriptor(_sa.ARRAY(_sa.Numeric) if self.array else _sa.Numeric())\n\n");
    out.push_str("    def bind_expression(self, bindvalue):\n        return _sa.cast(bindvalue, _sa.ARRAY(_pg.MONEY) if self.array else _pg.MONEY)\n\n");
    out.push_str("    def column_expression(self, column):\n        return _sa.cast(column, _sa.ARRAY(_sa.Numeric) if self.array else _sa.Numeric)\n\n\n");
    out.push_str("class Base(_orm.DeclarativeBase):\n    pass\n\n\n");
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
            let (python_type, sqlalchemy_type) = python_column(column, kind);
            let attribute = py_identifier(&column.name);
            let args =
                python_column_args(column, &sqlalchemy_type, &composite, table.no_primary_key);
            out.push_str(&format!(
                "    {attribute}: _orm.Mapped[{python_type}] = _orm.mapped_column({args})\n"
            ));
        }
        for relationship_shape in &table.relationships {
            let mut options = Vec::new();
            options.push(format!(
                "foreign_keys=[{}]",
                relationship_shape
                    .columns
                    .iter()
                    .map(|column| py_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if !relationship_shape.remote_columns.is_empty() {
                options.push(format!(
                    "remote_side=[{}]",
                    relationship_shape
                        .remote_columns
                        .iter()
                        .map(|column| py_identifier(column))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let options = if options.is_empty() {
                String::new()
            } else {
                format!(", {}", options.join(", "))
            };
            out.push_str(&format!(
                "    {}: _orm.Mapped[{} | None] = _orm.relationship(back_populates={}{options})\n",
                relationship_shape.child_property,
                relationship_shape.parent,
                py_string(&relationship_shape.parent_property)
            ));
        }
        if let Some(children) = reverse.get(&table.class_name) {
            for relationship_shape in children {
                let foreign_keys = py_string(&format!(
                    "[{}]",
                    relationship_shape
                        .columns
                        .iter()
                        .map(|column| format!(
                            "{}.{}",
                            relationship_shape.child,
                            py_identifier(column)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                out.push_str(&format!(
                    "    {}: _orm.Mapped[list[{}]] = _orm.relationship(back_populates={}, foreign_keys={foreign_keys})\n",
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
    out.push_str("    def __init__(self, session: _orm.Session):\n        self.session = session\n\n    @classmethod\n    def connect(cls) -> \"DataContext\":\n        from codingpad import _connect_data_context\n        return cls(_connect_data_context())\n\n");
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
            format!("_sa.ForeignKeyConstraint([{columns}], [{targets}])")
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

fn python_column(
    column: &ColumnShape,
    database_kind: crate::databases::DatabaseKind,
) -> (String, String) {
    use crate::databases::DatabaseKind;
    let raw = column.data_type.trim();
    let array = raw.ends_with("[]");
    let kind = base_type(raw.trim_end_matches("[]"));
    let (python, sqlalchemy) = match kind.as_str() {
        "oid" if database_kind == DatabaseKind::Postgres => ("int", "_pg.OID"),
        "smallint" | "integer" | "bigint" | "int" | "int2" | "int4" | "int8" | "serial"
        | "bigserial" | "smallserial" | "tinyint" => ("int", "_sa.Integer"),
        "money" | "smallmoney" if database_kind == DatabaseKind::Postgres => {
            ("_decimal.Decimal", "_PostgresMoney()")
        }
        "numeric" | "decimal" | "money" | "smallmoney" | "number" => {
            ("_decimal.Decimal", "_sa.Numeric")
        }
        "real" | "float" | "float4" | "float8" | "double" | "double precision" => {
            ("float", "_sa.Float")
        }
        "boolean" | "bool" | "bit" => ("bool", "_sa.Boolean"),
        "blob" | "bytea" | "binary" | "varbinary" | "image" | "rowversion" => {
            ("bytes", "_sa.LargeBinary")
        }
        "date" => ("_datetime.date", "_sa.Date"),
        "time" | "time without time zone" => ("_datetime.time", "_sa.Time"),
        "time with time zone" | "timetz" => ("_datetime.time", "_sa.Time(timezone=True)"),
        "interval" => ("_datetime.timedelta", "_sa.Interval"),
        "timestamp" if database_kind == DatabaseKind::SqlServer => ("bytes", "_sa.LargeBinary"),
        "timestamp"
        | "timestamp without time zone"
        | "datetime"
        | "datetime2"
        | "smalldatetime" => ("_datetime.datetime", "_sa.DateTime"),
        "timestamptz" | "timestamp with time zone" | "datetimeoffset" => {
            ("_datetime.datetime", "_sa.DateTime(timezone=True)")
        }
        "uuid" => ("_uuid.UUID", "_sa.Uuid"),
        "uniqueidentifier" => {
            if database_kind == DatabaseKind::SqlServer {
                ("_uuid.UUID", "_MSSQLUuid")
            } else {
                ("_uuid.UUID", "_sa.Uuid")
            }
        }
        "json" | "jsonb" => ("dict[str, _typing.Any]", "_sa.JSON"),
        "inet" | "cidr" => (
            "_ipaddress.IPv4Address | _ipaddress.IPv6Address",
            if database_kind == DatabaseKind::Postgres {
                if kind == "cidr" {
                    "_pg.CIDR"
                } else {
                    "_pg.INET"
                }
            } else {
                "_sa.Text"
            },
        ),
        "macaddr" | "macaddr8" => (
            "str",
            if database_kind == DatabaseKind::Postgres {
                if kind == "macaddr8" {
                    "_pg.MACADDR8"
                } else {
                    "_pg.MACADDR"
                }
            } else {
                "_sa.Text"
            },
        ),
        "text" | "ntext" | "varchar" | "nvarchar" | "char" | "nchar" | "character varying"
        | "character" | "citext" | "xml" | "name" => ("str", "_sa.Text"),
        _ => ("_typing.Any", "_sa.Text"),
    };
    let (python, sqlalchemy) = if array {
        if kind == "money" && database_kind == DatabaseKind::Postgres {
            (
                "list[_decimal.Decimal]".to_owned(),
                "_PostgresMoney(array=True)".to_owned(),
            )
        } else if python == "_typing.Any" {
            (
                "list[str]".to_owned(),
                format!(
                    "_PostgresEnumArray({})",
                    py_string(raw.trim_end_matches("[]"))
                ),
            )
        } else {
            (
                format!("list[{python}]"),
                format!("_sa.ARRAY({sqlalchemy})"),
            )
        }
    } else {
        (python.to_owned(), sqlalchemy.to_owned())
    };
    if column.primary_key {
        (python, sqlalchemy)
    } else {
        (format!("{python} | None"), sqlalchemy)
    }
}

fn python_column_args(
    column: &ColumnShape,
    sqlalchemy_type: &str,
    composite: &BTreeSet<String>,
    no_primary_key: bool,
) -> String {
    let attribute = py_identifier(&column.name);
    let mut positional = Vec::new();
    if attribute != column.name {
        positional.push(py_string(&column.name));
    }
    positional.push(sqlalchemy_type.to_owned());
    if !composite.contains(&column.name) {
        for target in &column.references {
            let mut parts = Vec::new();
            let default = matches!(target.schema.as_str(), "main" | "public" | "dbo" | "");
            if !default {
                parts.push(target.schema.clone());
            }
            parts.push(target.table.clone());
            parts.push(target.column.clone());
            positional.push(format!("_sa.ForeignKey({})", py_string(&parts.join("."))));
        }
    }
    let mut keywords = Vec::new();
    if column.primary_key || no_primary_key {
        keywords.push("primary_key=True".to_owned());
        if column.primary_key && !column.identity {
            keywords.push("autoincrement=False".to_owned());
        }
    }
    if !column.primary_key && !no_primary_key {
        keywords.push("nullable=True".to_owned());
    }
    positional
        .into_iter()
        .chain(keywords)
        .collect::<Vec<_>>()
        .join(", ")
}

fn csharp_models(tables: &[TableShape], kind: crate::databases::DatabaseKind) -> String {
    use crate::databases::DatabaseKind;
    let names = csharp_names(tables);
    let mut out = String::from(
        "// <auto-generated>\n\
         // Generated by CodingPad from the attached connection's schema.\n\
         // Refresh the schema to regenerate this file. Do not edit by hand.\n\
         // </auto-generated>\n\
         #nullable enable\n\
         global using CodingPad.Data;\n\
         using System;\n\
         using System.Collections.Generic;\n\
         using System.Net;\n\
         using System.Net.NetworkInformation;\n\
         using System.Text.Json;\n\
         using Microsoft.EntityFrameworkCore;\n\n",
    );
    if kind == DatabaseKind::Postgres {
        out.push_str("using NpgsqlTypes;\n");
    }
    out.push_str("namespace CodingPad.Data;\n\n");
    for table in tables {
        out.push_str(&format!("public sealed class {}\n{{\n", table.class_name));
        for column in &table.columns {
            let cs_type = csharp_column(column, kind);
            let attribute = &names.columns[&table.class_name][&column.name];
            out.push_str(&format!(
                "    public {cs_type} {attribute} {{ get; set; }}\n"
            ));
        }
        for relationship_shape in &table.relationships {
            let attribute = &names.child_relations[&(
                table.class_name.clone(),
                relationship_shape.child_property.clone(),
            )];
            out.push_str(&format!(
                "    public {}? {attribute} {{ get; set; }}\n",
                relationship_shape.parent
            ));
        }
        for child in tables {
            for relationship_shape in &child.relationships {
                if relationship_shape.parent == table.class_name {
                    let attribute = &names.parent_relations[&(
                        table.class_name.clone(),
                        child.class_name.clone(),
                        relationship_shape.child_property.clone(),
                    )];
                    out.push_str(&format!(
                        "    public List<{}> {attribute} {{ get; set; }} = new();\n",
                        relationship_shape.child
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
            table.class_name, names.db_sets[&table.class_name], table.class_name
        ));
    }
    out.push('\n');
    out.push_str(
        "    protected override void OnConfiguring(DbContextOptionsBuilder options)\n    {\n\
             var json = CodingPad.ScriptHelpers.DatabaseConnection;\n\
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
            "        var tls = Text(config, \"tls\");\n        var trust = Bool(config, \"trustServerCertificate\");\n        var builder = new Npgsql.NpgsqlConnectionStringBuilder\n        {\n            Host = Text(config, \"host\"),\n            Port = Number(config, \"port\") ?? 5432,\n            Database = Text(config, \"database\"),\n            Username = Text(config, \"user\"),\n            Password = Text(config, \"password\"),\n            SslMode = tls == \"disable\" ? Npgsql.SslMode.Disable : trust ? (tls == \"prefer\" ? Npgsql.SslMode.Prefer : Npgsql.SslMode.Require) : Npgsql.SslMode.VerifyFull,\n        };\n        if (!trust && tls == \"prefer\")\n        {\n            try\n            {\n                using var probe = new Npgsql.NpgsqlConnection(builder.ConnectionString);\n                probe.Open();\n            }\n            catch (Exception error) when (error.Message.Contains(\"No SSL enabled connection\"))\n            {\n                builder.SslMode = Npgsql.SslMode.Disable;\n            }\n        }\n        options.UseNpgsql(builder.ConnectionString);\n",
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
            .map(|column| names.columns[&table.class_name][&column.name].clone())
            .collect::<Vec<_>>();
        if keys.is_empty() {
            out.push_str("            entity.HasNoKey();\n");
        } else if keys.len() == 1 {
            out.push_str(&format!("            entity.HasKey(e => e.{});\n", keys[0]));
        } else {
            out.push_str(&format!(
                "            entity.HasKey(e => new {{ {} }});\n",
                keys.iter()
                    .map(|key| format!("e.{key}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for column in &table.columns {
            let property = &names.columns[&table.class_name][&column.name];
            if csharp_ignored(column, kind) && !column_used(table, &column.name) {
                out.push_str(&format!("            entity.Ignore(e => e.{property});\n"));
                continue;
            }
            if property != &column.name {
                out.push_str(&format!(
                    "            entity.Property(e => e.{property}).HasColumnName({});\n",
                    cs_string(&column.name)
                ));
            }
            if kind == DatabaseKind::Postgres
                && base_type(&column.data_type) == "interval"
                && !column.data_type.trim().ends_with("[]")
            {
                out.push_str(&format!(
                    "            entity.Property(e => e.{property}).HasConversion(value => value.HasValue ? new NpgsqlTypes.NpgsqlInterval(0, 0, value.Value.Ticks) : (NpgsqlTypes.NpgsqlInterval?)null, value => value.HasValue ? TimeSpan.FromTicks(value.Value.Time + value.Value.Days * TimeSpan.TicksPerDay + value.Value.Months * 30 * TimeSpan.TicksPerDay) : (TimeSpan?)null);\n"
                ));
            }
        }
        for relationship_shape in &table.relationships {
            let child_property = &names.child_relations[&(
                table.class_name.clone(),
                relationship_shape.child_property.clone(),
            )];
            let parent_property = &names.parent_relations[&(
                relationship_shape.parent.clone(),
                table.class_name.clone(),
                relationship_shape.child_property.clone(),
            )];
            let foreign_key = if relationship_shape.columns.len() == 1 {
                format!(
                    "e.{}",
                    names.columns[&table.class_name][&relationship_shape.columns[0]]
                )
            } else {
                format!(
                    "new {{ {} }}",
                    relationship_shape
                        .columns
                        .iter()
                        .map(|column| format!("e.{}", names.columns[&table.class_name][column]))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let parent_table = tables
                .iter()
                .find(|candidate| candidate.class_name == relationship_shape.parent);
            let parent_keys = parent_table
                .map(|parent| {
                    parent
                        .columns
                        .iter()
                        .filter(|column| column.primary_key)
                        .map(|column| names.columns[&parent.class_name][&column.name].clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let target_keys = parent_table
                .map(|parent| {
                    relationship_shape
                        .target_columns
                        .iter()
                        .filter_map(|column| names.columns[&parent.class_name].get(column).cloned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let principal = if target_keys != parent_keys && !target_keys.is_empty() {
                let key = if target_keys.len() == 1 {
                    format!("e.{}", target_keys[0])
                } else {
                    format!(
                        "new {{ {} }}",
                        target_keys
                            .iter()
                            .map(|key| format!("e.{key}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                format!(".HasPrincipalKey(e => {key})")
            } else {
                String::new()
            };
            out.push_str(&format!(
                "            entity.HasOne(e => e.{child_property}).WithMany(e => e.{parent_property}).HasForeignKey(e => {foreign_key}){principal};\n"
            ));
        }
        out.push_str("        });\n");
    }
    out.push_str("    }\n}\n");
    out
}

fn csharp_ignored(column: &ColumnShape, kind: crate::databases::DatabaseKind) -> bool {
    use crate::databases::DatabaseKind;
    if kind != DatabaseKind::Postgres {
        return false;
    }
    let raw = column.data_type.trim();
    let array = raw.ends_with("[]");
    let base = base_type(raw.trim_end_matches("[]"));
    if array {
        matches!(
            base.as_str(),
            "interval"
                | "oid"
                | "point"
                | "line"
                | "lseg"
                | "box"
                | "path"
                | "polygon"
                | "circle"
                | "tsvector"
                | "tsquery"
        )
    } else {
        base == "oid"
    }
}

fn column_used(table: &TableShape, name: &str) -> bool {
    table
        .columns
        .iter()
        .any(|column| column.primary_key && column.name == name)
        || table.relationships.iter().any(|relationship| {
            relationship.columns.iter().any(|column| column == name)
                || relationship
                    .target_columns
                    .iter()
                    .any(|column| column == name)
        })
}

struct CSharpNames {
    columns: BTreeMap<String, BTreeMap<String, String>>,
    child_relations: BTreeMap<(String, String), String>,
    parent_relations: BTreeMap<(String, String, String), String>,
    db_sets: BTreeMap<String, String>,
}

fn csharp_names(tables: &[TableShape]) -> CSharpNames {
    let mut used: BTreeMap<String, BTreeSet<String>> = tables
        .iter()
        .map(|table| {
            let mut names = BTreeSet::new();
            names.insert(table.class_name.clone());
            (table.class_name.clone(), names)
        })
        .collect();
    let mut columns = BTreeMap::new();
    for table in tables {
        let mut table_columns = BTreeMap::new();
        for column in &table.columns {
            let base = csharp_identifier(&column.name);
            let name = unique_member(&base, used.get_mut(&table.class_name).unwrap());
            table_columns.insert(column.name.clone(), name);
        }
        columns.insert(table.class_name.clone(), table_columns);
    }
    let mut child_relations = BTreeMap::new();
    for table in tables {
        for relationship_shape in &table.relationships {
            let base = csharp_identifier(&relationship_shape.child_property);
            let name = unique_member(&base, used.get_mut(&table.class_name).unwrap());
            child_relations.insert(
                (
                    table.class_name.clone(),
                    relationship_shape.child_property.clone(),
                ),
                name,
            );
        }
    }
    let mut parent_relations = BTreeMap::new();
    for child in tables {
        for relationship_shape in &child.relationships {
            let base = csharp_identifier(&relationship_shape.parent_property);
            let name = unique_member(&base, used.get_mut(&relationship_shape.parent).unwrap());
            parent_relations.insert(
                (
                    relationship_shape.parent.clone(),
                    child.class_name.clone(),
                    relationship_shape.child_property.clone(),
                ),
                name,
            );
        }
    }
    let mut db_sets = BTreeMap::new();
    let mut data_context_members = BTreeSet::new();
    for table in tables {
        let base = plural_pascal(&table.class_name);
        let name = unique_member(&base, &mut data_context_members);
        db_sets.insert(table.class_name.clone(), name);
    }
    CSharpNames {
        columns,
        child_relations,
        parent_relations,
        db_sets,
    }
}

fn unique_member(base: &str, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.to_owned()) {
        return base.to_owned();
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

fn csharp_column(column: &ColumnShape, kind: crate::databases::DatabaseKind) -> String {
    use crate::databases::DatabaseKind;
    let raw = column.data_type.trim();
    let array = raw.ends_with("[]");
    let base_kind = base_type(raw.trim_end_matches("[]"));
    let base = match base_kind.as_str() {
        "smallint" | "smallserial" => "short",
        "tinyint" => "byte",
        "integer" | "int" | "int4" | "serial" => {
            if kind == DatabaseKind::Sqlite {
                "long"
            } else {
                "int"
            }
        }
        "bigint" | "int8" | "bigserial" => "long",
        "numeric" | "decimal" | "money" | "smallmoney" | "number" => "decimal",
        "real" | "float4" => "float",
        "float" | "float8" | "double" | "double precision" => "double",
        "boolean" | "bool" | "bit" => "bool",
        "blob" | "bytea" | "binary" | "varbinary" | "image" | "rowversion" => "byte[]",
        "date" => "DateOnly",
        "time" | "time without time zone" => "TimeOnly",
        "time with time zone" | "timetz" => "DateTimeOffset",
        "interval" => "TimeSpan",
        "timestamp" if kind == DatabaseKind::SqlServer => "byte[]",
        "timestamp"
        | "timestamp without time zone"
        | "datetime"
        | "datetime2"
        | "smalldatetime" => "DateTime",
        "timestamptz" | "timestamp with time zone" | "datetimeoffset" => "DateTimeOffset",
        "uuid" | "uniqueidentifier" => "Guid",
        "inet" => "IPAddress",
        "cidr" if kind == DatabaseKind::Postgres => "NpgsqlCidr",
        "cidr" => "IPAddress",
        "macaddr" | "macaddr8" => "PhysicalAddress",
        "oid" => "uint",
        "json" | "jsonb" if kind == DatabaseKind::Postgres => "JsonDocument",
        "json" | "jsonb" => "string",
        _ => "string",
    };
    let base = if array {
        format!("{base}[]")
    } else {
        base.to_owned()
    };
    if column.primary_key {
        base
    } else {
        format!("{base}?")
    }
}

fn base_type(data_type: &str) -> String {
    data_type
        .trim()
        .split('(')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn unique_class_name(name: &str, used: &mut BTreeSet<String>) -> String {
    let mut base = pascal_case(&singularize(name));
    if reserved_class_names().contains(base.as_str()) {
        base.push('2');
    }
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

fn reserved_class_names() -> BTreeSet<&'static str> {
    [
        "Base",
        "DataContext",
        "DbContext",
        "Task",
        "ValueTask",
        "Thread",
        "Timer",
        "Stream",
        "File",
        "Path",
        "Directory",
        "Console",
        "Process",
        "Stopwatch",
        "Random",
        "String",
        "Int32",
        "Int64",
        "Boolean",
        "Double",
        "Decimal",
        "DateTime",
        "DateTimeOffset",
        "TimeSpan",
        "DateOnly",
        "TimeOnly",
        "Guid",
        "JsonDocument",
        "JsonElement",
        "JsonSerializer",
        "Array",
        "Tuple",
        "ValueTuple",
        "Nullable",
        "Lazy",
        "Span",
        "Memory",
        "ReadOnlySpan",
        "ReadOnlyMemory",
        "IList",
        "IDictionary",
        "IReadOnlyList",
        "IReadOnlyDictionary",
        "IAsyncEnumerable",
        "IAsyncDisposable",
        "IDisposable",
        "IComparable",
        "IEquatable",
        "IFormattable",
        "IConvertible",
        "IServiceScope",
        "ILogger",
        "IConfiguration",
        "HttpRequest",
        "HttpResponse",
        "HttpContext",
        "List",
        "Dictionary",
        "HashSet",
        "Queue",
        "Stack",
        "StringBuilder",
        "Object",
        "Action",
        "Func",
        "Exception",
        "Environment",
        "Math",
        "Convert",
        "Uri",
        "IPAddress",
        "IPEndPoint",
        "PhysicalAddress",
        "Encoding",
        "Regex",
        "Type",
        "Attribute",
        "IEnumerable",
        "IQueryable",
        "DbSet",
        "ModelBuilder",
        "IServiceProvider",
        "HttpClient",
        "StreamReader",
        "StreamWriter",
        "FileStream",
        "MemoryStream",
        "CancellationToken",
        "CancellationTokenSource",
    ]
    .into_iter()
    .collect()
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
        _ if snake.ends_with('y')
            && snake.len() > 1
            && "aeiou".contains(snake.chars().nth(snake.len() - 2).unwrap()) =>
        {
            format!("{snake}s")
        }
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
        if character.is_ascii_alphanumeric() {
            if uppercase {
                out.push(character.to_ascii_uppercase());
                uppercase = false;
            } else {
                out.push(character);
            }
        } else {
            uppercase = true;
        }
    }
    let out = out.trim_matches('_').to_owned();
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
    ) || matches!(out.as_str(), "metadata" | "registry")
    {
        out.push('_');
    }
    out
}

fn csharp_identifier(value: &str) -> String {
    pascal_case(value)
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
        assert!(python
            .contains("id: _orm.Mapped[int] = _orm.mapped_column(_sa.Integer, primary_key=True)"));
        assert!(python.contains(
            "customer: _orm.Mapped[Customer | None] = _orm.relationship(back_populates='orders', foreign_keys=[customer_id])"
        ));
        assert!(python.contains(
            "orders: _orm.Mapped[list[Order]] = _orm.relationship(back_populates='customer', foreign_keys='[Order.customer_id]')"
        ));
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
            identity: false,
        });
        let first = generate(&schema);
        let second = generate(&schema);
        assert_eq!(first.python, second.python);
        assert_eq!(first.csharp, second.csharp);
        assert!(first.python.contains("select: _orm.Mapped[str | None]"));
        assert!(first.csharp.contains("public string? Select { get; set; }"));
    }

    fn table(name: &str, schema: &str, columns: Vec<SchemaColumn>) -> SchemaTable {
        SchemaTable {
            schema: schema.into(),
            name: name.into(),
            columns,
        }
    }

    fn column(name: &str, data_type: &str, primary_key: bool) -> SchemaColumn {
        SchemaColumn {
            name: name.into(),
            data_type: data_type.into(),
            references: vec![],
            primary_key,
            identity: false,
        }
    }

    fn reference(
        table: &str,
        column: &str,
        schema: &str,
        constraint: &str,
        ordinal: u32,
    ) -> ForeignKey {
        ForeignKey {
            schema: schema.into(),
            table: table.into(),
            column: column.into(),
            constraint: constraint.into(),
            ordinal,
        }
    }

    #[test]
    fn separate_foreign_keys_to_one_parent_stay_separate() {
        use crate::databases::DatabaseKind;
        let mut billing = column("billing_customer_id", "INTEGER", false);
        billing.references = vec![reference("customers", "id", "main", "billing", 1)];
        let mut shipping = column("shipping_customer_id", "INTEGER", false);
        shipping.references = vec![reference("customers", "id", "main", "shipping", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table(
                    "customers",
                    "main",
                    vec![column("id", "INTEGER", true), column("name", "TEXT", false)],
                ),
                table(
                    "invoices",
                    "main",
                    vec![column("id", "INTEGER", true), billing, shipping],
                ),
            ],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains(
            "billing_customer: _orm.Mapped[Customer | None] = _orm.relationship(back_populates='invoices', foreign_keys=[billing_customer_id])"
        ));
        assert!(generated.python.contains(
            "shipping_customer: _orm.Mapped[Customer | None] = _orm.relationship(back_populates='invoices_2', foreign_keys=[shipping_customer_id])"
        ));
        assert_eq!(
            generated
                .python
                .matches("_sa.ForeignKey('customers.id')")
                .count(),
            2
        );
        assert!(generated
            .csharp
            .contains("public Customer? BillingCustomer"));
        assert!(generated
            .csharp
            .contains("public Customer? ShippingCustomer"));
        assert_eq!(generated.csharp.matches("entity.HasOne").count(), 2);
    }

    #[test]
    fn self_references_get_remote_side_and_safe_names() {
        use crate::databases::DatabaseKind;
        let mut manager = column("manager_id", "INTEGER", false);
        manager.references = vec![reference("employees", "id", "main", "manager", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![table(
                "employees",
                "main",
                vec![column("id", "INTEGER", true), manager],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains(
            "manager: _orm.Mapped[Employee | None] = _orm.relationship(back_populates='employees', foreign_keys=[manager_id], remote_side=[id])"
        ));
        assert!(generated.python.contains(
            "employees: _orm.Mapped[list[Employee]] = _orm.relationship(back_populates='manager', foreign_keys='[Employee.manager_id]')"
        ));
        assert!(generated.csharp.contains("public Employee? Manager"));
        assert!(generated.csharp.contains("public List<Employee> Employees"));
    }

    #[test]
    fn junction_primary_keys_keep_their_foreign_keys() {
        use crate::databases::DatabaseKind;
        let mut order = column("order_id", "INTEGER", true);
        order.references = vec![reference("orders", "id", "main", "order", 1)];
        let mut item = column("item_id", "INTEGER", true);
        item.references = vec![reference("items", "id", "main", "item", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("orders", "main", vec![column("id", "INTEGER", true)]),
                table("items", "main", vec![column("id", "INTEGER", true)]),
                table("order_items", "main", vec![order, item]),
            ],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains(
            "order_id: _orm.Mapped[int] = _orm.mapped_column(_sa.Integer, _sa.ForeignKey('orders.id'), primary_key=True, autoincrement=False)"
        ));
        assert!(generated.python.contains(
            "item_id: _orm.Mapped[int] = _orm.mapped_column(_sa.Integer, _sa.ForeignKey('items.id'), primary_key=True, autoincrement=False)"
        ));
        assert!(generated
            .csharp
            .contains("entity.HasKey(e => new { e.OrderId, e.ItemId });"));
    }

    #[test]
    fn tables_without_a_primary_key_are_keyless_and_do_not_lose_rows() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![table(
                "logs",
                "main",
                vec![column("level", "TEXT", false), column("msg", "TEXT", false)],
            )],
        };
        let generated = generate(&schema);
        assert_eq!(
            generated.python.matches("primary_key=True").count(),
            2,
            "{}",
            generated.python
        );
        assert!(generated.csharp.contains("entity.HasNoKey();"));
        assert!(!generated.csharp.contains("entity.HasKey("));
    }

    #[test]
    fn names_are_sanitised_and_imported_types_are_not_shadowed() {
        use crate::databases::DatabaseKind;
        let mut schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("foo.bar", "main", vec![column("select", "TEXT", true)]),
                table(
                    "texts",
                    "main",
                    vec![
                        column("id", "INTEGER", true),
                        column("value", "TEXT", false),
                    ],
                ),
                table(
                    "tags",
                    "main",
                    vec![column("id", "INTEGER", true), column("tag", "TEXT", false)],
                ),
            ],
        };
        schema.tables[0].columns[0].references = vec![reference("texts", "id", "main", "fk", 1)];
        let generated = generate(&schema);
        assert!(generated.python.contains("class FooBar(Base):"));
        assert!(generated.python.contains("select: _orm.Mapped[str]"));
        assert!(generated.python.contains("class Text(Base):"));
        assert!(generated.python.contains("_sa.Text"));
        assert!(generated.csharp.contains("public sealed class FooBar"));
        assert!(generated.csharp.contains("public string? Tag2"));
    }

    #[test]
    fn target_schema_is_included_only_when_it_is_not_the_default() {
        use crate::databases::DatabaseKind;
        let mut sales_parent = column("id", "INTEGER", true);
        sales_parent.references = vec![];
        let mut public_parent = column("id", "INTEGER", true);
        public_parent.references = vec![];
        let mut to_sales = column("sales_id", "INTEGER", false);
        to_sales.references = vec![reference("parents", "id", "sales", "sales_fk", 1)];
        let mut to_public = column("public_id", "INTEGER", false);
        to_public.references = vec![reference("parents", "id", "public", "public_fk", 1)];
        let schema = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![
                table("parents", "sales", vec![sales_parent]),
                table("parents", "public", vec![public_parent]),
                table("kids", "sales", vec![to_sales, to_public]),
            ],
        };
        let generated = generate(&schema);
        assert!(generated
            .python
            .contains("_sa.ForeignKey('sales.parents.id')"));
        assert!(generated.python.contains("_sa.ForeignKey('parents.id')"));
    }

    #[test]
    fn database_specific_types_match_each_provider() {
        use crate::databases::DatabaseKind;
        let make = |kind| Schema {
            kind,
            tables: vec![table(
                "things",
                "main",
                vec![
                    column("id", "INTEGER", true),
                    column("data", "json", false),
                    column("at", "timestamp with time zone", false),
                ],
            )],
        };
        let sqlite = generate(&make(DatabaseKind::Sqlite));
        assert!(sqlite.csharp.contains("public long Id"));
        assert!(sqlite.csharp.contains("public string? Data"));
        let postgres = generate(&make(DatabaseKind::Postgres));
        assert!(postgres.csharp.contains("public JsonDocument? Data"));
        assert!(postgres.csharp.contains("public DateTimeOffset? At"));
    }

    #[test]
    fn a_column_with_two_foreign_keys_emits_both_constraints() {
        use crate::databases::DatabaseKind;
        let mut target = column("target_id", "INTEGER", false);
        target.references = vec![
            reference("a", "id", "main", "to_a", 1),
            reference("b", "id", "main", "to_b", 1),
        ];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("a", "main", vec![column("id", "INTEGER", true)]),
                table("b", "main", vec![column("id", "INTEGER", true)]),
                table("links", "main", vec![column("id", "INTEGER", true), target]),
            ],
        };
        let generated = generate(&schema);
        let line = generated
            .python
            .lines()
            .find(|line| line.contains("target_id:"))
            .unwrap();
        assert_eq!(line.matches("_sa.ForeignKey").count(), 2, "{line}");
    }

    #[test]
    fn self_reference_to_a_unique_non_primary_key_uses_the_referenced_column() {
        use crate::databases::DatabaseKind;
        let mut manager = column("manager_code", "TEXT", false);
        manager.references = vec![reference("employees", "code", "main", "manager", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![table(
                "employees",
                "main",
                vec![
                    column("id", "INTEGER", true),
                    column("code", "TEXT", false),
                    manager,
                ],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains("remote_side=[code]"));
    }

    #[test]
    fn postgres_money_and_enum_arrays_do_not_silently_corrupt_values() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![table(
                "things",
                "public",
                vec![
                    column("id", "integer", true),
                    column("price", "money", false),
                    column("moods", "mood[]", false),
                ],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains("_pg.MONEY"));
        assert!(generated.python.contains("_PostgresEnumArray"));
        assert!(generated.python.contains("list[str]"));
    }

    #[test]
    fn postgres_interval_cidr_and_oid_use_npgsql_types() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![table(
                "things",
                "public",
                vec![
                    column("id", "integer", true),
                    column("span", "interval", false),
                    column("net", "cidr", false),
                    column("oid", "oid", false),
                ],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains("using NpgsqlTypes;"));
        assert!(generated.csharp.contains("public TimeSpan? Span"));
        assert!(generated.csharp.contains("public NpgsqlCidr? Net"));
        assert!(generated.csharp.contains("entity.Ignore(e => e.Oid);"));
    }

    #[test]
    fn non_identity_integer_primary_keys_are_not_autoincremented() {
        use crate::databases::DatabaseKind;
        let mut id = column("id", "integer", true);
        id.identity = false;
        let schema = Schema {
            kind: DatabaseKind::SqlServer,
            tables: vec![table("orders", "dbo", vec![id])],
        };
        let generated = generate(&schema);
        assert!(generated.python.contains("autoincrement=False"));
    }

    #[test]
    fn postgres_interval_uses_a_converter_and_unsupported_columns_are_ignored() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![table(
                "things",
                "public",
                vec![
                    column("id", "integer", true),
                    column("span", "interval", false),
                    column("points", "point[]", false),
                    column("oid", "oid", false),
                ],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains("public TimeSpan? Span"));
        assert!(generated
            .csharp
            .contains("HasConversion(value => value.HasValue ? new NpgsqlTypes.NpgsqlInterval"));
        assert!(generated.csharp.contains("entity.Ignore(e => e.Points);"));
        assert!(generated.csharp.contains("entity.Ignore(e => e.Oid);"));
    }

    #[test]
    fn array_and_tuple_are_reserved_and_arrays_pluralises_correctly() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("arrays", "main", vec![column("id", "INTEGER", true)]),
                table("tuples", "main", vec![column("id", "INTEGER", true)]),
            ],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains("public sealed class Array2"));
        assert!(generated.csharp.contains("public sealed class Tuple2"));
        assert!(generated.csharp.contains("public DbSet<Array2> Array2s"));
    }

    #[test]
    fn two_children_pointing_at_one_parent_get_distinct_collections() {
        use crate::databases::DatabaseKind;
        let mut order_customer = column("customer_id", "INTEGER", false);
        order_customer.references = vec![reference("customers", "id", "main", "order_customer", 1)];
        let mut invoice_customer = column("customer_id", "INTEGER", false);
        invoice_customer.references =
            vec![reference("customers", "id", "main", "invoice_customer", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("customers", "main", vec![column("id", "INTEGER", true)]),
                table(
                    "orders",
                    "main",
                    vec![column("id", "INTEGER", true), order_customer],
                ),
                table(
                    "invoices",
                    "main",
                    vec![column("id", "INTEGER", true), invoice_customer],
                ),
            ],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains("public List<Order> Orders"));
        assert!(generated.csharp.contains("public List<Invoice> Invoices"));
        assert!(generated
            .python
            .contains("orders: _orm.Mapped[list[Order]]"));
        assert!(generated
            .python
            .contains("invoices: _orm.Mapped[list[Invoice]]"));
    }

    #[test]
    fn keyless_children_do_not_get_relationships() {
        use crate::databases::DatabaseKind;
        let mut customer = column("customer_id", "INTEGER", false);
        customer.references = vec![reference("customers", "id", "main", "log_customer", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table("customers", "main", vec![column("id", "INTEGER", true)]),
                table(
                    "logs",
                    "main",
                    vec![column("level", "TEXT", false), customer],
                ),
            ],
        };
        let generated = generate(&schema);
        assert!(!generated.python.contains("_orm.relationship"));
        assert!(!generated.csharp.contains("entity.HasOne"));
        assert!(generated.csharp.contains("entity.HasNoKey();"));
    }

    #[test]
    fn foreign_key_to_a_unique_non_primary_key_gets_a_principal_key() {
        use crate::databases::DatabaseKind;
        let mut code = column("customer_code", "TEXT", false);
        code.references = vec![reference("customers", "code", "main", "customer_code", 1)];
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![
                table(
                    "customers",
                    "main",
                    vec![column("id", "INTEGER", true), column("code", "TEXT", false)],
                ),
                table("orders", "main", vec![column("id", "INTEGER", true), code]),
            ],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains(".HasPrincipalKey(e => e.Code);"));
    }

    #[test]
    fn sql_server_and_postgres_specific_types_are_mapped() {
        use crate::databases::DatabaseKind;
        let sqlserver = Schema {
            kind: DatabaseKind::SqlServer,
            tables: vec![table(
                "things",
                "dbo",
                vec![
                    column("id", "uniqueidentifier", true),
                    column("rv", "timestamp", false),
                    column("t", "time with time zone", false),
                ],
            )],
        };
        let generated = generate(&sqlserver);
        assert!(generated.python.contains("_MSSQLUuid"));
        assert!(generated.python.contains("_sa.LargeBinary"));
        assert!(generated.python.contains("_sa.Time(timezone=True)"));
        assert!(generated.csharp.contains("public Guid Id"));
        assert!(generated.csharp.contains("public byte[]? Rv"));
        assert!(generated.csharp.contains("public DateTimeOffset? T"));

        let postgres = Schema {
            kind: DatabaseKind::Postgres,
            tables: vec![table(
                "things",
                "public",
                vec![
                    column("id", "integer", true),
                    column("t", "timetz", false),
                    column("span", "interval", false),
                    column("addr", "inet", false),
                    column("net", "cidr", false),
                    column("mac", "macaddr", false),
                ],
            )],
        };
        let generated = generate(&postgres);
        assert!(generated.python.contains("_sa.Interval"));
        assert!(generated.python.contains("_pg.INET"));
        assert!(generated.python.contains("_pg.CIDR"));
        assert!(generated.python.contains("_pg.MACADDR"));
        assert!(generated.csharp.contains("public TimeSpan? Span"));
        assert!(generated.csharp.contains("public IPAddress? Addr"));
        assert!(generated.csharp.contains("public PhysicalAddress? Mac"));
    }

    #[test]
    fn common_bcl_names_are_reserved_for_generated_classes() {
        use crate::databases::DatabaseKind;
        let schema = Schema {
            kind: DatabaseKind::Sqlite,
            tables: vec![table(
                "threads",
                "main",
                vec![column("id", "INTEGER", true)],
            )],
        };
        let generated = generate(&schema);
        assert!(generated.csharp.contains("public sealed class Thread2"));
        assert!(generated.python.contains("class Thread2(Base):"));
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
                            identity: true,
                        },
                        SchemaColumn {
                            name: "b".into(),
                            data_type: "integer".into(),
                            references: vec![],
                            primary_key: true,
                            identity: true,
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
                            identity: true,
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
                            identity: false,
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
                            identity: false,
                        },
                    ],
                },
            ],
        };
        let generated = generate(&schema);
        assert_eq!(
            generated
                .python
                .matches("children: _orm.Mapped[list[Child]]")
                .count(),
            1
        );
        assert_eq!(
            generated
                .python
                .matches("children: _orm.Mapped[list[Child]] = _orm.relationship(back_populates='parent', foreign_keys='[Child.parent_a, Child.parent_b]')")
                .count(),
            1
        );
        assert!(generated.python.contains(
            "parent: _orm.Mapped[Parent | None] = _orm.relationship(back_populates='children', foreign_keys=[parent_a, parent_b])"
        ));
        assert!(generated.python.contains(
            "_sa.ForeignKeyConstraint(['parent_a', 'parent_b'], ['parents.a', 'parents.b'])"
        ));
    }
}
