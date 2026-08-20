# mowz

`mowz` is a token-efficient CLI for agents querying production context.
It provides one command for searching logs in a project's configured backend.

## Goals

- Minimize tokens returned to agents without hiding relevant failures.
- Query the backend configured for a named project.
- Return predictable NDJSON suitable for programmatic consumption.
- Bound responses with a default time window, hard result limit, selected fields,
  and deduplication.
- Keep backend credentials out of project configuration.
- Add metrics and traces after the logs workflow is proven.

## Usage

```sh
mowz [--from <time>] [--to <time>] <project> <query>
```

The query is sent unchanged to VictoriaLogs and environment-scoped Railway
backends. A service-scoped Railway backend adds its configured service filter.
The time window defaults to `--from now-1h --to now`. Relative values use
Grafana-style syntax such as `now-6h`; seconds (`s`), minutes (`m`), hours
(`h`), days (`d`), and weeks (`w`) are supported. Railway also accepts RFC 3339
timestamps, while VictoriaLogs time values are passed through to Grafana.
Each result is emitted as one compact JSON log object per line (NDJSON).
Backend, error, and truncation metadata are omitted; query and configuration
failures are reported as normal command errors instead of NDJSON records.
Each project is configured with one backend.

## Configuration

Projects and their backend are declared in a repository-local `.mowz.toml` file.
Credentials are supplied through environment variables referenced by that file.

```toml
[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token_env = "GRAFANA_TOKEN"
scope_filter = "_stream:{environment=\"production\"}"
```

`scope_filter` is optional. When set, `mowz` sends it through the VictoriaLogs
Grafana query model as `extraFilters`, which the datasource applies using
VictoriaLogs `extra_filters` semantics. The user's LogsQL expression remains
unchanged and separate from this fixed filter. This requires VictoriaLogs
Grafana datasource plugin v0.18.1 or later.

The scope filter is an application safety boundary that limits queries made
through `mowz`; it is not credential-level authorization. Use credentials and
backend access controls with an appropriate least-privilege scope when a user
must not be able to bypass this application.

Railway logs use `environmentLogs` and are explicitly scoped to either one
service or every service in the configured environment. Existing Railway
configurations with a `service_id` remain service-scoped; `scope = "service"`
is shown below for clarity. The user's filter is combined with a fixed
`@service:<service_id>` constraint.

```toml
[projects.api]
type = "railway"
environment_id = "00000000-0000-0000-0000-000000000000"
scope = "service"
service_id = "00000000-0000-0000-0000-000000000000"
token_env = "RAILWAY_TOKEN"
auth = "project_token"
```

To search all services in one environment, set `scope = "environment"` and
omit `service_id`. Environment scope is never inferred from a missing service
ID.

```toml
[projects.api]
type = "railway"
environment_id = "00000000-0000-0000-0000-000000000000"
scope = "environment"
token_env = "RAILWAY_TOKEN"
auth = "project_token"
```

Use `auth = "project_token"` for Railway project tokens, which are sent with the
`Project-Access-Token` header. Use `auth = "bearer"` for account or workspace
tokens. Railway queries use its [log filter syntax](https://docs.railway.com/observability/logs).
Returned Railway entries include `serviceId` and `deploymentId` when Railway
supplies those source tags.

Each project requires one VictoriaLogs or Railway backend. `mowz` emits at most
100 entries.

Backend support:

- VictoriaLogs through the Grafana API (implemented)
- Railway service- and environment-scoped logs through the Railway API (implemented)

## Non-goals

- A dashboard or other graphical interface
- Log ingestion, storage, or retention
- Alerting or monitoring automation
- A human-readable output mode
- Translating between backend query languages
- A unified query language in the first version

## Distribution

`mowz` is implemented in Rust and released with cargo-dist installers for
Homebrew and shell installation.
