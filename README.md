# aws-tui

A terminal UI for browsing AWS service models and running audited `aws` CLI commands.

Instead of remembering the right incantation, type a fuzzy query (e.g. `ec2 describe-instance-status`), inspect the exact command and its parameters before running it, and view the output as a table or syntax-highlighted JSON — with every executed command written to an audit log.

## Features

- **Fuzzy search** over all service/operation names from the AWS botocore models (nucleo-powered, 200 result cap).
- **Intent preview** — before anything runs, see the exact `aws ...` command line, whether it passed validation, all input parameters (with required ones marked), and the docs for the selected operation.
- **Params editor** — `Enter` prompts for required params; press `e` on a result to edit optional params plus `--query`/`--output` before run. Empty optional values are skipped.
- **Audited execution** — every proposal and execution is appended to `audit.jsonl` (with provenance: CLI version, model path + sha256, api version) before the command runs. Runs in the background with cancel (`x`), timeout (`AWS_TUI_TIMEOUT_SECS`, default 120s), and bounded throttling retries (`AWS_TUI_MAX_RETRIES`, default 3); credential, throttling, and validation failures are surfaced distinctly.
- **Region & profile auto-detection** — precedence: `AWS_REGION` → `AWS_DEFAULT_REGION` → profile `region`/`sso_region` in `~/.aws/config` → `us-east-1`. Override any time with the pickers.
- **Output views** — structured table (auto) or raw syntax-highlighted JSON; toggle with `v`.
- **JMESPath follow-ups** — start a query with `>` to run a predefined projection (region / tags / id) over the last result.
- **Plans as data** — human-written `plans/*.json` files (`Plan { steps[], target, risks, preconditions, rollback, expected }`) validated against the operation index; press `p` on a result to inspect read-only. No model execution.
- **Incident workspace** — human-written `incidents/*.json` files (evidence timeline, hypotheses, linked plan, approvals) validated and shown read-only; press `i` on a result. Nothing executes from the workspace.
- **Task inbox** — incidents and plans persisted in `history.redb`, listed active-first with status transitions; press `t` to browse, `Enter` to load, `a` to advance status. Nothing executes from the inbox.
- **Small-terminal friendly** — layout uses `Min` constraints so it stays usable at 80×24.

## Requirements

- A working `aws` CLI (default `aws` on `PATH`; override with `AWS_TUI_AWS_BIN`).
- The AWS botocore service models (see [Data source](#data-source)).
- An AWS credential setup the `aws` CLI understands (env vars, `~/.aws/config` profiles, SSO, etc.).

## Build & run

```sh
cargo build --release
./target/release/aws-tui
```

### Data source

The TUI reads service models (the `<service>/<version>/service-2.json` files) from one of:

1. `AWS_TUI_DATA_DIR` env var
2. `AWS_CLI_DATA_DIR` env var
3. A sibling `aws-cli/awscli/botocore/data` checkout (walked up from the current directory)

```sh
# Point at your installed botocore data
AWS_TUI_DATA_DIR=/usr/local/lib/python3.*/site-packages/botocore/data ./target/release/aws-tui
```

Run with `--dump` to list every loaded service, its model provenance, and operations instead of starting the TUI.

## Usage

| Key | Action |
| --- | --- |
| `F2` / `F3` | Open region / profile picker |
| `Tab` | Cycle focus: search → results → output |
| `Enter` | Run the selected operation (or the `>` follow-up query) |
| `e` | Edit params / `--query` / `--output` / paging for the selected operation (when results focused) |
| `h` | Open command history (browse, rerun re-validated) |
| `x` | Cancel a running command |
| `s` | Save output to `aws-tui-output-<ts>.json` (when output focused) |
| `?` | Key help overlay |
| `p` | Open the human-written plan (`plans/example.json` or `$AWS_TUI_PLAN`) read-only |
| `i` | Open the human-written incident workspace (`incidents/example.json` or `$AWS_TUI_INCIDENT`) read-only |
| `t` | Open the task inbox (stored incidents, `Enter` loads, `a` advances status) |
| `v` | Toggle output table / raw JSON (when output focused) |
| `↑` / `↓` | Move selection (or scroll output when focused) |
| `PgUp` / `PgDn` | Scroll details / output |
| `Esc` | Close picker → focus search → clear query |
| `q` / `Ctrl-C` | Quit |

### Query syntax

Type anything; fuzzy matching ranks services and operations together, e.g.:

```
ec2 describe-instance-status
sts get-caller-identity
describe-instance-status
```

Results show the service (e.g. `[ec2]`) and operation. Enter runs it. The status bar and footer show the resolved region, profile, and command result.

To project over the previous result, type `>` followed by any JMESPath expression:

```
> Subnets[].SubnetId
> Subnets[?AvailabilityZone=='us-east-1a'].SubnetId
```

Bare hints still work (`> region`, `> tag`, `> id`) and map to the subnet projections above. Invalid expressions report the JMESPath error instead of running anything.

Runs execute in the background so the UI stays responsive; `x` cancels the wait (the AWS-side operation is not implied to be canceled). Each run has a wall-clock timeout (default 120s, override with `AWS_TUI_TIMEOUT_SECS`). Press `h` on the results pane to browse history; `Enter` reruns the selected entry after re-validating it against the loaded models.

### Output

- **Table view** (default for parseable JSON): union of all object keys as headers; scalar arrays become a single `value` column; nested objects are compacted to a preview.
- **Raw JSON**: pretty-printed with syntax highlighting. Toggle with `v` while the output pane is focused.

### Plans

Write plans by hand as JSON (see `plans/example.json`). Each plan has `version: 1`, `title`, `target`, `steps[]`, plus `risks`, `preconditions`, `rollback`, and `expected`. Each step names a real `service`/`operation` with optional `params`, `query`, `output`, approval flag, preconditions, and expected outcome. The TUI validates the file against the loaded models and shows it read-only; nothing executes from the plan view. Override the file with `AWS_TUI_PLAN=/path/to/plan.json`.

### Incidents

Write incidents by hand as JSON (see `incidents/example.json`). Each incident has `version: 1`, `id`, `title`, `status` (`open`/`investigating`/`planned`/`approved`/`executing`/`verifying`/`closed`), plus optional `severity`, `account`, `region`, an evidence timeline (`signals[]` with `ts`/`source`/`summary`), `hypotheses[]` with confidence and supporting notes, an optional `plan_path`, and `approvals[]`. The workspace validates the file, resolves the linked plan against the loaded models, and shows evidence, hypotheses, plan, and approvals read-only; nothing executes from the workspace. Override the file with `AWS_TUI_INCIDENT=/path/to/incident.json`.

### Task inbox

The inbox persists incidents and plans in `history.redb` next to the audit log, so state survives restarts. First open seeds the store from the example files when empty. Statuses move along `open` → `investigating` → `planned` → `approved` → `executing` → `verifying` → `closed` (plus documented back-edges); illegal jumps are rejected with an explanation. `Enter` loads the selected incident into the workspace; `a` advances its status.

## Audit log

Every command proposal and execution is appended as JSONL to:

```
<data_dir>/aws-tui/audit.jsonl
```

where `<data_dir>` is the platform data dir (e.g. `~/.local/share` on Linux). Each record includes the command line, exit code, duration, output hash, provenance (CLI version, model path + sha256, API version), and the invoking user/host/session. Files rotate to `audit.1.jsonl` past the size cap (default 10MB, `AWS_TUI_AUDIT_MAX_MB`).

## Configuration

Optional TOML file at `$AWS_TUI_CONFIG` or `~/.config/aws-tui/config.toml`:

```toml
default_region = "us-east-1"
default_profile = "dev"
timeout_secs = 120
max_retries = 3
read_only = false
allow_prod_mutations = false
prod_markers = ["prod", "production"]
audit_max_mb = 10
```

Environment always wins (`AWS_TUI_TIMEOUT_SECS`, `AWS_TUI_MAX_RETRIES`, `AWS_TUI_READ_ONLY=1`, `AWS_TUI_ALLOW_PROD_MUTATIONS=1`, `AWS_REGION`/`AWS_PROFILE`).

## Safety

- Read-only mode (`read_only` / `AWS_TUI_READ_ONLY=1`) blocks non-`describe`/`list`/`get` operations.
- Production guard: mutations against profiles/regions matching `prod_markers` require typing the operation name, unless explicitly allowed.
- Secret-looking param values (`password`, `token`, `secret`, …) render as `***` in previews and status; the local audit log keeps full values. History lists never show param values.

## Development

```sh
cargo test    # unit + integration tests
```

## License

MIT. See [LICENSE](LICENSE).
