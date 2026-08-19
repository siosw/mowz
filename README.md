# mowz

`mowz` is a token-efficient CLI for agents querying production context.
It provides one command for searching logs across a project's configured backends.

## Goals

- Minimize tokens returned to agents without hiding relevant failures.
- Query every backend configured for a named project.
- Return predictable NDJSON suitable for programmatic consumption.
- Bound responses with a default time window, hard result limit, selected fields,
  and deduplication.
- Keep backend credentials out of project configuration.
- Add metrics and traces after the logs workflow is proven.

## Usage

```sh
mowz <project> <query>
```

The query is sent unchanged to VictoriaLogs and environment-scoped Railway
backends. Service-scoped Railway backends add their configured service filter.
Each result is emitted as one compact JSON log object per line (NDJSON).
Backend, error, and truncation metadata are omitted; query and configuration
failures are reported as normal command errors instead of NDJSON records.
The first slice supports one backend; independent fan-out and partial failure
reporting are planned follow-up work.

## Configuration

Projects and their backends are declared in a repository-local `.mowz.toml` file.
Credentials are supplied through environment variables referenced by that file.

```toml
[projects.api]

[[projects.api.backends]]
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

[[projects.api.backends]]
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

[[projects.api.backends]]
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

The current implementation requires exactly one VictoriaLogs or Railway
backend for the selected project. It queries the last hour and emits at most
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
