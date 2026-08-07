# aws-tui

A terminal UI for browsing AWS service models and running audited `aws` CLI commands.

Instead of remembering the right incantation, type a fuzzy query (e.g. `ec2 describe-instance-status`), inspect the exact command and its parameters before running it, and view the output as a table or syntax-highlighted JSON — with every executed command written to an audit log.

## Features

- **Fuzzy search** over all service/operation names from the AWS botocore models (nucleo-powered, 200 result cap).
- **Intent preview** — before anything runs, see the exact `aws ...` command line, whether it passed validation, all input parameters (with required ones marked), and the docs for the selected operation.
- **Audited execution** — every proposal and execution is appended to `audit.jsonl` (with provenance: CLI version, model path + sha256, api version) before the command runs.
- **Region & profile auto-detection** — precedence: `AWS_REGION` → `AWS_DEFAULT_REGION` → profile `region`/`sso_region` in `~/.aws/config` → `us-east-1`. Override any time with the pickers.
- **Output views** — structured table (auto) or raw syntax-highlighted JSON; toggle with `v`.
- **JMESPath follow-ups** — start a query with `>` to run a predefined projection (region / tags / id) over the last result.
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

To project over the previous result, type `>` followed by a hint:

```
> region   # Subnets[].AvailabilityZone
> tag      # Subnets[].Tags[]
> id       # Subnets[].SubnetId
```

### Output

- **Table view** (default for parseable JSON): union of all object keys as headers; scalar arrays become a single `value` column; nested objects are compacted to a preview.
- **Raw JSON**: pretty-printed with syntax highlighting. Toggle with `v` while the output pane is focused.

## Audit log

Every command proposal and execution is appended as JSONL to:

```
<data_dir>/aws-tui/audit.jsonl
```

where `<data_dir>` is the platform data dir (e.g. `~/.local/share` on Linux). Each record includes the command line, exit code, duration, output hash, provenance (CLI version, model path + sha256, API version), and the invoking user/host/session.

## Development

```sh
cargo test    # unit + integration tests
```

## License

Unspecified.
