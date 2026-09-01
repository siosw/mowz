# mowz

`mowz` gives coding agents a small, bounded NDJSON view of production logs instead of dumping broad searches into their context. It queries a named VictoriaLogs or Railway project with backend-native syntax; results default to the last hour and 3 rows (maximum 100).

## Install

```sh
brew install siosw/tap/mowz
# macOS/Linux alternative:
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/siosw/mowz/releases/latest/download/mowz-installer.sh | sh
```

## Configure safely

Create `.mowz.toml` in the repository (mowz finds the nearest copy while walking up from the current directory). Keep tokens out of the file: use an environment variable as below, or an [`op://` 1Password reference](https://developer.1password.com/docs/cli/secret-references/). Give the token least-privilege backend access; fixed filters constrain mowz, but are not authorization boundaries.

VictoriaLogs through Grafana ([LogsQL](https://docs.victoriametrics.com/victorialogs/logsql/); `scope_filter` requires the VictoriaLogs datasource plugin v0.18.1+):

```toml
[projects.api]
type = "victoria_logs"
url = "https://grafana.example.com"
datasource_uid = "victoria-logs"
token = { env = "GRAFANA_TOKEN" }
scope_filter = "_stream:{environment=\"production\"}"
```

Railway, safely limited to one service ([find IDs and learn filter syntax](https://docs.railway.com/observability/logs)):

```toml
[projects.worker]
type = "railway"
environment_id = "00000000-0000-0000-0000-000000000000"
scope = "service"
service_id = "00000000-0000-0000-0000-000000000000"
token = { env = "RAILWAY_TOKEN" }
auth = "project_token"
```

Use `auth = "bearer"` for Railway account/workspace tokens. To intentionally query every service in one environment, use `scope = "environment"` and omit `service_id`. Check discovery without resolving secrets with `mowz projects`.

## Give your agent the skill

Point your agent's documented skill mechanism at [`skills/mowz/SKILL.md`](skills/mowz/SKILL.md). If it needs a copied file, `mowz skill` prints the identical bundled skill to stdout; redirect that output to the location required by your agent or skill manager.

Then let the agent run bounded queries from the configured repository:

```sh
mowz query --from now-15m --to now worker '@level:error'
```
