# Kineto Documentation

This directory contains the product and technical design baseline for Kineto. It describes intended product boundaries and active architectural decisions; implementation evidence should update ordinary design docs, while durable decisions are recorded as ADRs.

## Product

- [Product vision](product/vision.md)

## Architecture

- [Architecture overview](architecture/overview.md)
- [Native desktop boundary](architecture/native-boundary.md)
- [Project format](architecture/project-format.md)
- [Providers and MCP](architecture/providers-and-mcp.md)

## Workflow

- [Production pipeline](workflow/production-pipeline.md)
- [Artifact state machine](workflow/state-machine.md)

## Schemas and fixtures

- [`schemas/project.schema.json`](../schemas/project.schema.json) — decoded `project.toml` v1 model
- [`schemas/artifact.schema.json`](../schemas/artifact.schema.json) — artifact lifecycle/provenance model
- [`schemas/selection.schema.json`](../schemas/selection.schema.json) — stable-ID candidate selection
- [`schemas/job.schema.json`](../schemas/job.schema.json) — job/write-ahead intent model
- [`schemas/scene.schema.json`](../schemas/scene.schema.json) — canonical scene record
- [`schemas/shot.schema.json`](../schemas/shot.schema.json) — canonical shot record
- [`fixtures/projects/minimal`](../fixtures/projects/minimal) — committed golden canonical project
- [`kineto-schema-contracts`](../crates/kineto-schema-contracts) — test-only executable schema/golden-project contract

## Playbooks

- [WabiSabi UI consumer playbook](playbooks/wabisabi-ui-consumer.md)
- [Native performance playbook](playbooks/native-performance.md)

## Roadmap

- [MVP roadmap](roadmap/mvp.md)

## Architecture Decision Records

- [ADR-0001: Local-first Flutter UI with Rust Engine](adr/0001-local-first-flutter-rust.md) — original process-boundary decision, superseded for desktop by ADR-0005
- [ADR-0002: Human Approval Artifact Graph](adr/0002-human-approval-artifact-graph.md)
- [ADR-0003: Local Engine Security Boundary](adr/0003-local-engine-security-boundary.md) — desktop stdio/session portion superseded by ADR-0005; future network transport requirements remain relevant
- [ADR-0004: Canonical Project State and Derived Local Index](adr/0004-canonical-project-state.md)
- [ADR-0005: In-process Rust Engine with Typed Flutter FFI](adr/0005-in-process-rust-ffi.md)
- [ADR-0006: Provider Plugins as Configured Local Processes](adr/0006-provider-plugins.md)
- [ADR-0007: MCP Client for Director-Requested References](adr/0007-mcp-client-references.md)
- [ADR-0008: The Director's Own Agent CLIs, and Provider Choice in Settings](adr/0008-directors-own-agents-and-provider-choice.md)
- [ADR-0009: Connections: Any Service, by the Protocol It Speaks](adr/0009-connections-by-protocol.md)
- [ADR-0010: Kineto as an MCP Server for Outside Agents](adr/0010-mcp-server-for-outside-agents.md)

Accepted ADRs remain immutable records. A changed decision is documented by a superseding ADR rather than rewriting history.
