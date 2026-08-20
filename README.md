# mowz

`mowz` is a token-efficient CLI for agents querying production context.
It provides a focused command for searching logs in a project's configured
backend.

## Goals

- Minimize tokens returned to agents without hiding relevant failures.
- Query the backend configured for a named project.
- Return predictable NDJSON suitable for programmatic consumption.
- Bound responses with a default time window, hard result limit, selected fields,
  and deduplication.
- Let projects choose which configuration values are checked in and which are
  resolved from secrets.
- Add metrics and traces after the logs workflow is proven.

## Usage

```sh
mowz query [--from <time>] [--to <time>] [--limit <rows>] <project> <query>
```

The query is sent unchanged to VictoriaLogs and environment-scoped Railway
backends. A service-scoped Railway backend adds its configured service filter.
The time window defaults to `--from now-1h --to now`. Relative values use
Grafana-style syntax such as `now-6h`; seconds (`s`), minutes (`m`), hours
(`h`), days (`d`), and weeks (`w`) are supported. Railway also accepts RFC 3339
timestamps, while VictoriaLogs time values are passed through to Grafana.
Each result is emitted as one compact JSON log object per line (NDJSON).
The result limit defaults to 3 rows and can be changed with `--limit`; accepted
values are 1 through 100.
Backend, error, and truncation metadata are omitted; query and configuration
failures are reported as normal command errors instead of NDJSON records.
Each project is configured with one backend.

List configured projects and their backends with:

```sh
mowz projects
```

The command emits one compact JSON object per project in name order, without
resolving any backend configuration values or secrets:

```json
{"project":"api","backend":"victoria_logs"}
{"project":"worker","backend":"railway"}
```

## Configuration

Projects and their backend are declared in a `.mowz.toml` file. Starting from
the current directory, `mowz` searches parent directories and uses the nearest
configuration it finds. It stops before checking the home directory, or at the
filesystem root when the current directory is outside the home directory. It
does not merge configurations.

Every string-valued backend field accepts either a literal string or a secret
source. Ordinary strings remain literal and are not reinterpreted:

```toml
url = "https://grafana.example.com"
```

Secret sources can use the environment, 1Password, or both (these are
alternative definitions of the same field):

```toml
token = { env = "GRAFANA_TOKEN" }
```

```toml
token = { op = "op://production/grafana/token" }
```

```toml
token = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }
```

For a source containing both `env` and `op`, `mowz` reads the environment
variable first. It runs `op read --no-newline <reference>` only when that
variable is missing. An environment variable that is present but empty is not
treated as missing; for `token`, the existing empty-token validation rejects
the result. Resolved values are always used literally and are never recursively
interpreted as another environment variable or 1Password reference.

The combined form lets the same committed `.mowz.toml` work in both settings:

- **Local:** install and authenticate the 1Password `op` CLI, then run
  `mowz query ...`. If the environment variable is absent, `mowz` uses the
  1Password fallback.
- **Amp orb:** configure the named environment variable as an Amp project
  secret, then run `mowz query ...`. The environment value wins, so the orb
  does not need the `op` CLI.

Direct 1Password resolution requires `op` to be installed, authenticated, and
able to access the configured reference. Literal values are checked into the
configuration file, so do not use a literal for a value that must remain
secret.

```toml
[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN", op = "op://production/grafana/token" }
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
token = { env = "RAILWAY_TOKEN", op = "op://production/railway/token" }
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
token = { env = "RAILWAY_TOKEN", op = "op://production/railway/token" }
auth = "project_token"
```

Use `auth = "project_token"` for Railway project tokens, which are sent with the
`Project-Access-Token` header. Use `auth = "bearer"` for account or workspace
tokens. Railway queries use its [log filter syntax](https://docs.railway.com/observability/logs).
Returned Railway entries include `serviceId` and `deploymentId` when Railway
supplies those source tags.

Each project requires one VictoriaLogs or Railway backend.

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

The standard Agent Skill is distributed as
[`skills/mowz/SKILL.md`](skills/mowz/SKILL.md) for skill managers to consume.
The same hand-authored file is embedded in the binary and can be printed to
standard output without loading configuration:

```sh
mowz skill
```

Installation and discovery paths are owned by the agent or skill manager; mowz
does not install the skill itself.
