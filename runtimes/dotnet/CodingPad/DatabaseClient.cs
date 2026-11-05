using System.Data.Common;
using System.Text.Json;
using Microsoft.Data.SqlClient;
using Microsoft.Data.Sqlite;
using Npgsql;

namespace CodingPad;

/// <summary>Rows returned by a database query.</summary>
public sealed record DatabaseRows(IReadOnlyList<string> Columns, IReadOnlyList<object?[]> Rows);

/// <summary>A database client bound to the current script run.</summary>
public sealed class DatabaseClient : IDisposable
{
    private JsonElement? config;
    private DbConnection? connection;
    private bool closed;
    internal DatabaseClient(JsonElement? config) => this.config = config?.Clone();

    private DbConnection Open()
    {
        ObjectDisposedException.ThrowIf(closed, this);
        if (connection is not null) return connection;
        if (config is null) throw new InvalidOperationException("No connection is attached. Add a codingpad: connection=<name> header.");
        var settings = config.Value.GetProperty("settings");
        string? Get(string name) => settings.TryGetProperty(name, out var item) && item.ValueKind != JsonValueKind.Null ? item.ToString() : null;
        var password = config.Value.TryGetProperty("password", out var p) && p.ValueKind == JsonValueKind.String ? p.GetString() : null;
        var trust = settings.TryGetProperty("trustServerCertificate", out var certificate) && certificate.ValueKind == JsonValueKind.True;
        var tls = Get("tls") ?? "prefer";
        connection = Get("kind") switch
        {
            "sqlite" => new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = Get("path") }.ConnectionString),
            "postgres" => new NpgsqlConnection(new NpgsqlConnectionStringBuilder {
                Host = Get("host"), Port = int.TryParse(Get("port"), out var pgPort) ? pgPort : 5432,
                Database = Get("database"), Username = Get("user"), Password = password,
                SslMode = tls == "disable" ? SslMode.Disable : trust ? (tls == "require" ? SslMode.Require : SslMode.Prefer) : SslMode.VerifyFull
            }.ConnectionString),
            "sqlserver" => new SqlConnection(new SqlConnectionStringBuilder {
                DataSource = Get("host") + "," + (Get("port") ?? "1433"), InitialCatalog = Get("database"),
                UserID = Get("user"), Password = password ?? "", TrustServerCertificate = trust,
                Encrypt = tls == "require" ? SqlConnectionEncryptOption.Mandatory : SqlConnectionEncryptOption.Optional
            }.ConnectionString),
            _ => throw new InvalidOperationException("Unsupported database type.")
        };
        try { connection.Open(); return connection; }
        catch (NpgsqlException error) when (connection is NpgsqlConnection && tls == "prefer" && !trust &&
            error.Message.Contains("No SSL enabled connection from this host is configured", StringComparison.Ordinal))
        {
            var options = new NpgsqlConnectionStringBuilder(connection.ConnectionString) { SslMode = SslMode.Disable, Password = password };
            connection.Dispose();
            connection = new NpgsqlConnection(options.ConnectionString);
            try { connection.Open(); return connection; }
            catch { connection.Dispose(); connection = null; throw; }
        }
        catch { connection.Dispose(); connection = null; throw; }
    }

    private DbCommand Command(string sql, IReadOnlyDictionary<string, object?>? parameters)
    {
        var command = Open().CreateCommand();
        command.CommandText = sql;
        if (parameters is not null)
            foreach (var (name, value) in parameters)
            {
                var parameter = command.CreateParameter();
                parameter.ParameterName = name;
                parameter.Value = value ?? DBNull.Value;
                command.Parameters.Add(parameter);
            }
        return command;
    }

    /// <summary>Execute a query and return rows for dumping.</summary>
    public DatabaseRows Query(string sql, IReadOnlyDictionary<string, object?>? parameters = null)
    {
        using var command = Command(sql, parameters);
        using var reader = command.ExecuteReader();
        var columns = Enumerable.Range(0, reader.FieldCount).Select(reader.GetName).ToArray();
        var rows = new List<object?[]>();
        while (reader.Read())
        {
            var values = new object[reader.FieldCount];
            reader.GetValues(values);
            rows.Add(values.Select(value => value is DBNull ? null : value).ToArray());
        }
        return new DatabaseRows(columns, rows);
    }

    /// <summary>Execute a statement and return its affected row count.</summary>
    public int Execute(string sql, IReadOnlyDictionary<string, object?>? parameters = null)
    {
        using var command = Command(sql, parameters);
        return command.ExecuteNonQuery();
    }

    public void Dispose()
    {
        closed = true;
        config = null;
        connection?.Dispose();
        connection = null;
    }
}
