---
name: mowz
description: Queries bounded production logs from named VictoriaLogs and Railway projects with the mowz CLI. Use when investigating incidents, errors, deployments, or other production behavior with mowz.
license: GPL-3.0-or-later
compatibility: Requires the mowz CLI, backend access, and jq for the processing examples.
---

# Query production logs

Use `mowz` to retrieve a small NDJSON slice from a named production project.
Start narrow, inspect the result, then refine the time window or filter. A full
result is a prompt to narrow the query, not proof that all matches were returned.

## Workflow

1. Select the project and query language.
2. Set the shortest useful time window.
3. Run the query.
4. Process the NDJSON with `jq`.
5. Refine until the result answers the investigation.

## 1. Select the project and query language

Run `mowz` from the directory containing `.mowz.toml` or one of its descendants.
Starting from the current directory, `mowz` searches upward and uses the nearest
configuration. It does not merge configurations and stops before checking the
home directory (or at the filesystem root when outside `$HOME` or `HOME` is
unavailable).

Read the selected `[projects.<name>]` entry before writing a query. The project
name selects the backend, scope, and credentials.

| Project `type` | Write | Example |
|---|---|---|
| `railway` | Railway log filter syntax | `@level:error AND "connection refused"` |
| `victoria_logs` | VictoriaLogs LogsQL | `_stream:{app="api"} AND "connection refused"` |

Queries are backend-native. `mowz` does not translate Railway filters to LogsQL
or LogsQL to Railway filters.

For service-scoped Railway projects, `mowz` adds the configured
`@service:<service_id>` constraint. For VictoriaLogs projects, a configured
`scope_filter` is sent separately as an extra filter. Neither behavior rewrites
the query you provide.

## 2. Set the time window

Use `--from` and `--to`. They default to `now-1h` and `now`. Results default to
3 entries; use `--limit` to request between 1 and 100 when needed.

```sh
mowz query --from now-15m --to now api '<backend query>'
```

Relative times accept seconds (`s`), minutes (`m`), hours (`h`), days (`d`),
and weeks (`w`). RFC 3339 timestamps are also accepted.

Prefer the shortest window that could contain the event. If `mowz` reaches the
selected result limit, narrow the time window or add a selective backend filter
before drawing conclusions.

## 3. Run the query

Pass the `.mowz.toml` project name, not a backend URL:

```sh
mowz query --from now-30m --to now api '@level:error'
```

Success: stdout contains one compact JSON object per log entry, one entry per
line. An empty stdout means the backend returned no entries for that query and
window.

Failure: configuration and query errors are command errors, not NDJSON records.
Read stderr and correct the project, credentials, time range, or backend syntax
before processing stdout.

## 4. Process NDJSON with jq

Keep output streaming when filtering or selecting fields. For a Railway project
named `api`:

```sh
mowz query --from now-15m api '@level:error' |
  jq -c 'select(.message | contains("timeout")) | {timestamp, message, serviceId}'
```

VictoriaLogs preserves field names from Grafana frames. Inspect keys before
assuming a schema:

```sh
mowz query --from now-15m api '_stream:{app="api"}' | jq -c 'keys'
```

Use `jq -s` only when the operation needs one array, such as aggregation:

```sh
mowz query --from now-15m api '@level:error' |
  jq -s 'group_by(.serviceId) | map({serviceId: .[0].serviceId, count: length})'
```

## Resolve configuration failures

Every string-valued backend field in `.mowz.toml` is either a literal or a
secret source:

```toml
url = "https://grafana.example.com"
token = { env = "GRAFANA_TOKEN" }
token = { op = "op://production/grafana/token" }
token = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }
```

Resolution follows these rules:

1. Use literal strings exactly as written. Never reinterpret them as environment
   names or `op://` references.
2. When `env` is configured, use that variable if it is present.
3. When the variable is absent and `op` is configured, run
   `op read --no-newline <reference>`.
4. Treat a present but empty variable as present. Do not fall back to
   1Password; token validation will reject an empty token.
5. Use resolved values literally. Never resolve them a second time.

If 1Password resolution fails, confirm that `op` is installed, authenticated,
and authorized for the configured reference. Do not print resolved values while
debugging secrets.

## Guardrails

- Do not mix Railway and LogsQL syntax.
- Do not broaden a query before checking whether a narrower window is enough.
- Do not treat a result that reaches the selected limit as complete.
- Do not parse stderr as NDJSON.
- Do not expose literal, environment, or 1Password secret values.

## Report

State the project, backend query, and time window used. Summarize what the
returned entries show. Say when the selected limit makes the conclusion
incomplete, and name the next narrower query when more evidence is needed.
