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
Backends are queried independently; a failure is reported alongside successful
results rather than failing the whole request.

## Configuration

Projects and their backends are declared in a repository-local `.ctx.yaml` file.
Credentials are supplied through environment variables referenced by that file.

Initial backends:

- VictoriaLogs through the Grafana API
- Railway deployment logs through the Railway API

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
