# ctx

`ctx` is a token-efficient CLI for agents querying production context.
It provides one command for searching logs across a project's configured backends.

## Goals

- Minimize tokens returned to agents without hiding relevant failures.
- Query every backend configured for a named project.
- Return predictable, compact JSON suitable for programmatic consumption.
- Bound responses with a default time window, hard result limit, selected fields,
  truncation, and deduplication.
- Keep backend credentials out of project configuration.
- Add metrics and traces after the logs workflow is proven.

## Usage

```sh
ctx <project> <query>
```

The query is sent unchanged to each backend configured for the project.
Results are attributed to their backend and emitted as compact JSON only.
The first slice supports one backend; independent fan-out and partial failure
reporting are planned follow-up work.

## Configuration

Projects and their backends are declared in a repository-local `.ctx.yaml` file.
Credentials are supplied through environment variables referenced by that file.

```yaml
projects:
  api:
    backends:
      - name: production
        type: victoria_logs
        url: https://grafana.example.com
        datasource_uid: victoria-logs
        token_env: GRAFANA_TOKEN
```

The initial implementation requires exactly one VictoriaLogs backend for the
selected project. It queries the last hour and emits at most 100 entries.

Backend support:

- VictoriaLogs through the Grafana API (implemented)
- Railway deployment logs through the Railway API (planned)

## Non-goals

- A dashboard or other graphical interface
- Log ingestion, storage, or retention
- Alerting or monitoring automation
- A human-readable output mode
- Translating between backend query languages
- A unified query language in the first version

## Distribution

`ctx` is implemented in Rust and released with cargo-dist installers for
Homebrew and shell installation.
