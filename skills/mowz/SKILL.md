---
name: mowz
description: Queries bounded production logs from named VictoriaLogs and Railway projects with the mowz CLI. Use when investigating incidents, errors, deployments, or other production behavior with mowz.
license: GPL-3.0-or-later
compatibility: Requires the mowz CLI, backend access, and jq for the processing examples.
---

# Query production logs with mowz

Use `mowz` to retrieve a small, machine-readable slice of logs from a named
project. Keep investigations focused: choose a narrow time range and query,
inspect the bounded result, then refine.

## Run a query

Run commands from the directory containing the repository-local `.mowz.toml`:

```sh
mowz query --from now-30m --to now api '<backend query>'
```

The positional `api` value is a project name from `[projects.api]` in
`.mowz.toml`. It selects that project's configured backend and credentials. Do
not substitute a backend URL for the project name.

The time window defaults to `--from now-1h --to now`. Prefer the shortest useful
window, such as `now-10m`, before broadening it. Relative units are seconds (`s`),
minutes (`m`), hours (`h`), days (`d`), and weeks (`w`). RFC 3339 timestamps are
also accepted. `mowz` enforces a hard limit of 100 returned entries, so a full
result is a reason to narrow the window or filter rather than assume all matches
were returned.

## Write the backend's query language

Queries are sent to the configured backend; `mowz` does not translate between
query languages.

- Railway projects use Railway log filter syntax, for example
  `@level:error AND "connection refused"`. For a service-scoped project, `mowz`
  also adds the configured `@service:<service_id>` constraint.
- VictoriaLogs projects use LogsQL, for example
  `_stream:{app="api"} AND "connection refused"`. A configured `scope_filter`
  is sent separately as a VictoriaLogs extra filter; it does not rewrite the
  LogsQL expression.

If unsure which syntax applies, inspect the selected project's `type` in
`.mowz.toml` before composing the query.

## Process NDJSON with jq

Successful output is NDJSON: one compact JSON object per line. Keep it as a
stream when filtering or selecting fields. For example, a Railway project named
`api` emits fields such as `message` and `serviceId`:

```sh
mowz query --from now-15m api '@level:error' |
  jq -c 'select(.message | contains("timeout")) | {timestamp, message, serviceId}'
```

Use slurp mode only when an array is actually useful:

```sh
mowz query --from now-15m api '@level:error' | jq -s 'group_by(.serviceId) | map({serviceId: .[0].serviceId, count: length})'
```

Do not parse command errors as log records: query and configuration failures are
reported as command errors, not as NDJSON objects.

## Understand configuration and secrets

`mowz` reads only `.mowz.toml` in its current working directory. Each backend
string field can be configured as:

```toml
url = "https://grafana.example.com"                    # literal
token = { env = "GRAFANA_TOKEN" }                     # environment
token = { op = "op://production/grafana/token" }      # 1Password
token = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }
```

Literal strings are used literally, even if they look like an environment name
or `op://` reference. For a source with both `env` and `op`, the environment
variable wins. The 1Password fallback runs `op read --no-newline <reference>`
only when that variable is absent; a present but empty variable does not fall
back. Resolved values are never recursively interpreted.

Before querying, verify that the working directory has the intended project and
that its referenced environment variable is available, or that the 1Password
CLI is installed, authenticated, and authorized for the configured reference.
Never print resolved secret values while debugging configuration.
