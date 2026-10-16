# Product: Agentic Cloud Operations System

## Main feature

An autonomous AWS operations engineer: give the system an incident, operational objective, or required work, and it gathers context, plans changes, executes through governed tools and coding agents, verifies the outcome, and records what it learned.

Observe → Understand → Execute → Verify.

Status: vision, not implemented. The current repo is a keyboard-first AWS CLI operation console. In this reframed product it becomes the governed execution tool and TUI control room, not the full agentic system. All execution still shells out to the `aws` CLI (`src/tui/app.rs:208`, `src/aws/runner.rs:10`); there are no agents, workflow DAGs, memory stores, model gateways, or verification postconditions.

## Sub features

### A. Implemented foundation (what exists today)

- Model-driven operation catalog: botocore `service-2.json` load with provenance and skip-with-diagnostic (`src/aws/loader.rs:70`, `src/aws/loader.rs:151`).
- Fuzzy operation search over `"<service> <operation>"` with 200-result cap plus natural-language scoring (`src/aws/index.rs:72`, `src/aws/builder.rs:74`, `src/tui/app.rs:147`).
- Intent preview and validation: command line, `[OK]/[ERR]`, required-marked params, docs; JMESPath compile and unknown/duplicate/empty param rejection (`src/tui/ui.rs:188`, `src/tui/app.rs:172`, `src/aws/builder.rs:313`).
- Params editor: required + optional params with `e` to edit, plus `--query`/`--output` with allowlist validation (`src/tui/app.rs:285`, `src/tui/app.rs:306`, `src/tui/app.rs:659`).
- Audited execution: `propose` then `executed` JSONL with argv, provenance, exit/duration/output hash, user/host/session, reference link (`src/ops/audit.rs:105`, `src/ops/audit.rs:126`).
- Account/region context: profiles/regions from config plus env precedence, `F2`/`F3` pickers (`src/aws/awsconfig.rs:62`, `src/aws/awsconfig.rs:184`, `src/tui/app.rs:591`).
- Output inspection: blocks tree + JSON, drill in/out with auto-drill, `>` follow-ups over cached JSON (`src/tui/app.rs:395`, `src/tui/app.rs:438`, `src/tui/app.rs:485`, `src/tui/ui.rs:322`).
- Keyboard-first small-terminal layout (`src/tui/ui.rs:11`, `src/tui/app.rs:808`).

### B. Planned agentic sub features (not implemented)

- Signal and work ingestion: CloudWatch, CloudTrail, Health, tickets, Git, manual requests.
- Agent reasoning and planning: triage, retrieval, diagnosis with competing hypotheses and confidence, task decomposition.
- Multi-agent roles with bounded responsibilities: supervisor, incident investigator, infrastructure planner, coding agent, policy/verification agent. Logical roles first; separate workers only when isolation or review improves results.
- Open-model layer: model gateway, interchangeable open-weight reasoning models, vLLM/Ollama backends, coding-agent adapters, routing by task/latency/cost, validated structured output, fallbacks, evals. Define `ReasoningModel`, `CodingAgent`, `ToolExecutor` interfaces; no provider conversation-ID coupling.
- Memory and references: episodic (what happened), semantic (what is known), procedural (how to do it), working (what is happening now), plus versioned reference corpus. Exact-ID + structured + keyword + semantic + dependency retrieval, all with provenance. PostgreSQL + vector/search index + object storage + Git; promote observations to trusted procedures only via validation or human review.
- Workflow engine: versioned DAGs/state machines with dependencies, preconditions, approvals, retries with backoff, idempotency keys, deadlines, locks, compensation, postconditions, replay, pause/resume. Durable checkpoints across restarts; evaluate Temporal or equivalent.
- Execution workers: scoped AWS tools, shell runners, IaC, isolated coding-agent workspaces (repo explore, reproduce, minimal diff, tests, PR, no self-merge or prod deploy bypass), test runners.
- Verification and learning: explicit success criteria, health checks, audit trail, incident closeout with symptoms, root cause, evidence, what worked/failed, workflow version, recovery result, follow-ups and runbook proposals.
- TUI as agent control room: command center, task inbox, incident workspace, workflow inspector, agent monitor, approval queue, memory explorer, infrastructure explorer, coding workspace, audit history. Operator moves from objective to supporting evidence or tool call.
- Autonomy levels L0–L4 (observe, recommend, approve-and-execute, bounded autonomy, adaptive), scoped by action, account, environment, resource, risk, and time. Confidence is not authorization; critical decisions use deterministic policy code.
- Trust hierarchy: policy first, then approved workflows/runbooks, current state/config, verified history, official docs, model hypotheses last. Treat logs, tickets, and retrieved docs as untrusted. Every action carries why, evidence, references, blast radius, permitting policy, success criteria, and failure handling.

## Other

- Suggested stack: Ratatui TUI as client of a persistent backend, AWS SDK adapters, supervisor protocol, durable workflows, vLLM/Ollama, isolated coding workers, PostgreSQL, vector retrieval, S3-compatible artifacts, EventBridge/ticket/Git ingestion, deterministic policy service, sandboxed execution, traces and model/tool telemetry.
- MVP slice: ingest alarms plus manual incidents for Lambda, ECS, or CloudFormation failures; collect metrics/logs/CloudTrail/deployments; retrieve runbooks and similar incidents; generate a structured plan with an open-weight model; enforce policy; show evidence and plan in the TUI; require approval; execute via scoped tools; verify postconditions; persist incident, trace, and memory proposal. Measure diagnosis time, recovery time, false positives, violations, interventions, and cost per incident.
- Non-goals for MVP: 50-service coverage, cross-account inventory, dependency graphs, custom dashboards, plugin system, offline index. Depth on a few workflows beats shallow breadth.
- Design principle: durable memory-backed orchestrator with replaceable AI workers; agents reason, memory contextualizes, references authorize, workflows execute reliably, policy bounds, verification proves completion.
