# Agent interfaces: CLI, RPC, and MCP

Research date: 2026-11-05. Status: source-backed comparison and recommendation, not an implemented or approved transport decision. Scope: agents calling Canban, especially completion/blockage reports. Preserve the agreed one-Session model and simplified MVP policies.

Inspected revisions: herdr-core 0.1.18 at [c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a](https://github.com/herdrdev/herdr-core/tree/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a), herdr-app 0.1.13 at [e77f112ac2ea136b84fe510efa42d41163bfd776](https://github.com/herdrdev/herdr-app/tree/e77f112ac2ea136b84fe510efa42d41163bfd776). Both research checkouts were clean. MCP references are the official, versioned 2025-11-25 specification, read on the research date.

## Recommendation

For Canban MVP, provide **a thin CLI to shell-capable agents, backed by the same typed application operations used by the native UI**. Use local JSON-RPC IPC for the CLI-to-running-app boundary if that remains the selected local transport. Keep direct RPC available to programmatic adapters. Add MCP as a thin optional adapter when a target agent host needs native MCP tools.

This is not a choice between three separate product APIs:

```text
Canban native Rust / NSH UI -------- typed Rust call ------+
                                                         |
Shell-capable agent -> Canban CLI -- local IPC/RPC -------+--> shared handlers
                                                         |      |
Tool-capable host -> MCP adapter -- optional later -------+      +-- cards / timeline
                                                                +-- report callback
                                                                +-- minimal events
                                                                +-- core adapter
                                                                      |
                                                                  herdr-core
```

The shared handlers can live in a modular-monolith Canban process. A separate Canban daemon is not required by CLI, RPC, or MCP; decide process lifetime from actual closed-window/quit behavior. The UI need not serialize an RPC call merely to reach its own Rust application service. Main product surfaces are native Rust/NSH; WebView is an optional embed where useful.

CLI/RPC/MCP adapters must preserve the same execution association, report semantics, errors, and normal replay behavior. They must not become separate schedulers or create Session types.

## What Herdr actually does

**Verified:** Herdr already combines CLI and RPC.

| Surface | Evidence and behavior | Lesson for Canban |
| --- | --- | --- |
| hc CLI | The command table maps run -> terminal.run, send -> terminal.send, events -> events.subscribe, and schema -> rpc.discover. Ordinary calls connect to core and invoke the named method. | CLI is an ergonomic adapter, not a second execution engine. |
| JSON-RPC API | Unix socket/Windows named pipe; plain newline-delimited JSON-RPC, with optional framing for attach streams. hc connect carries the protocol over stdin/stdout. | A client protocol supports applications independently of the CLI command syntax. |
| Single API/schema model | One method table creates dispatch, the hello method list, and rpc.discover; Rust parameter types are reused as CLI arguments where exposed. | Keep operations/types/schema coherent instead of hand-maintaining unrelated interfaces. |
| Discovery/manual | hc schema / rpc.discover give OpenRPC; hc --skill prints the agent manual, with --help for exact arguments. | Machine-readable schema and concise agent instructions both matter. |
| Agent coordination | The manual tells agents to call hc run/read/send/wait and to send a result or blocker to a parent terminal. | A shell-callable command is a real, documented agent interface. |
| Herdr app | Its client launches hc connect and negotiates framed mode; herdr-app --skill separately describes commands that control the visible window. | UI organization and runtime execution remain different boundaries. |

Primary sources: [CLI method mapping](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/cli.rs#L850-L884), [CLI RPC call](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/cli.rs#L226-L228), [method/schema generation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/rpc.rs#L166-L195), [API connection guide](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md#L1-L43), [agent manual](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/SKILL.md#L8-L37), [parent reporting instructions](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/SKILL.md#L105-L119), [app's core connection](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/hc.rs#L277-L290), [app window manual](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/SKILL.md#L1-L18).

Important nuance: not every RPC needs a CLI verb. Core's opaque doc.get/doc.set are deliberately API-only. Canban can expose the useful agent subset without requiring an exhaustive command for every internal operation. [API-only methods](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/cli.rs#L883-L884).

### Is Herdr's interface already MCP?

**No built-in MCP server was found in the inspected core/app implementation.** The current core method table exposes hello, runtime operations, and rpc.discover, not MCP initialize/tools/list/tools/call. Searches of core/app source and manifests found no MCP handler/server implementation. This is a bounded source finding, not a claim about external plugins or every historical/future Herdr release.

Core's rpc.rs contains a comment comparing readable per-method schemas to MCP tools. Its actual document format is OpenRPC, and the comment does not implement MCP. [Actual API table](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api.rs#L56-L152), [OpenRPC generation/comment](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/rpc.rs#L89-L93), [format](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/rpc.rs#L131-L143).

### Why hc send is not Canban's completion API

hc send delivers text to another terminal and participates in Herdr's agent-to-agent coordination. It does not know Canban's card, logical request/attempt, completed callback, or notification contract.

Canban can copy the interaction pattern—teach an agent a small reporting command—while implementing execution.report in Canban. Do not require a fake supervisor agent/terminal just to receive hc send and parse prose into business state. Core's status observations can still feed the UI and attention handling.

## What MCP adds

MCP uses JSON-RPC with its own connection lifecycle and capabilities. It adds tool discovery through tools/list and invocation through tools/call, with input schemas and tool results. An arbitrary JSON-RPC service is therefore not automatically an MCP server. [Official lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle), [tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

The specification defines stdio and Streamable HTTP transports. In stdio, the host launches the server subprocess; stdout is protocol-only and logs go to stderr. An eventual local Canban MCP adapter could connect onward to the running application's service. MCP's protocol connection/session is distinct from a Canban execution/conversation and from HC_SESSION. [Official transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

These are MCP protocol facts, not proof that every installed agent/host supports the same registration, tool-loading, permission, or lifecycle behavior.

## Practical comparison for this MVP

The judgments below are engineering recommendations based on the current local, shell-capable-agent workflow, not measured performance or compatibility results.

| Concern | Thin CLI | Agent directly using RPC | MCP adapter |
| --- | --- | --- | --- |
| Agent entry | Run a short command through its existing shell/tool capability | Agent writes/uses an RPC client or host exposes one | Host exposes registered Canban tools |
| Installation/setup | Ship binary/path and short usage instructions | Supply protocol/endpoint and a usable client | Register server in each target host's supported configuration |
| Discovery | --help, a short --skill/manual, optional schema command | Versioned schema/rpc.discover-style discovery | Tool listing and schemas through the host |
| Argument burden | Flags/subcommands; execution binding can be injected | Must construct envelope, endpoint connection and parameters | Model supplies tool arguments; host/adapter handles protocol |
| Current execution | CLI inherits endpoint/request/attempt context at launch | Caller carries that context explicitly | Adapter must be explicitly bound to a Run or accept an explicit execution reference |
| Process/lifecycle cost | A small CLI process per invocation plus app service | Can be a reusable client connection | Standard connection initialization and host-managed server/connection lifecycle |
| Result/error presentation | Structured JSON plus exit code; do not rely on prose | Native API result/error, convenient for programmatic clients | Schema/tool result mapping and host display |
| Permission behavior | Uses existing shell access and host rules | Depends on client/host and endpoint access | Depends on host tool/server rules; MCP itself is not an approval bypass |
| Retry semantics | Adapter preserves report key and accepted/duplicate receipt | Client must preserve the same business key | Tool adapter must preserve the same business key; JSON-RPC ID is not that key |
| MVP fit | Strong: reuses current agent command capability and Herdr precedent | Strong internal/programmatic interface; weaker default instruction for a model to hand-code sockets | Useful optional tool experience, with extra host integration to qualify |
| Testing | Fixture invokes CLI, validates receipt, then card/timeline effect | Handler/protocol tests and adapter tests | All shared tests plus initialization/tool-schema/result/host-registration checks |

Context/token overhead depends on what the host loads and when. CLI can reveal a short manual/help on demand; MCP can expose structured tools through a host. Neither guarantees lower token use, better reliability, or faster execution without measurement.

Shell argument quoting and long reports need care: keep a file/body or stdin path so the agent does not embed large Markdown/JSON into a fragile shell command. This is a CLI ergonomics issue, not a reason for a second business API.

## Execution binding is the key MCP design issue

For the CLI proposal, Canban can pass its endpoint, request ID, attempt number, and CLI path through core's supported env field. The launched agent's CLI invocation inherits that execution context. [Run env/labels](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L225-L241).

A host may start its MCP server once, outside the subsequently launched agent's shell environment. A shared long-lived MCP server cannot safely infer the current Canban execution from whichever terminal happens to be focused or from a later shell export.

If MCP is added, choose an explicit association: a per-execution adapter process/configuration with fixed context, or an explicit execution reference resolved by shared handlers. Do not identify the Canban Run by an MCP protocol session ID. This is an adapter-context concern, not a reason to create question/stage Session types or a new Session UUID.

The injected IDs establish logical association. They do not independently create a per-agent security boundary. Keep the existing local-user trust boundary for MVP and avoid exposing a raw core command API merely to implement a reporting tool.

## Recommended delivery sequence

1. Define the small shared operation/result/error types for execution.report and core card/comment/context operations. Reuse them in native UI handlers and wire adapters.
2. Implement a thin CLI with injected execution binding, --help, machine-readable output, and a concise --skill/manual pattern inspired by Herdr. Keep the completion declaration as the agent's decision.
3. Use local RPC as the CLI/app boundary and as an explicit programmatic integration surface. It does not require a separate daemon or a wire call from the native UI.
4. Validate a fixture and the declared existing runnable launch configuration: completed, blocked, long report, lost acknowledgment/repeated report, and equivalent UI/API history.
5. Add a small MCP adapter only when a concrete host or tool-only agent benefits enough to justify registration/lifecycle support. Start with report, context/read, and comment operations; avoid exposing the entire core or application API by default.

An MCP-first choice can be reasonable if the actual first-release host already requires MCP and cannot execute the CLI, or the user explicitly prioritizes native tool discovery. That requirement was not established by current Herdr's design or the present shell-capable execution workflow.

## Decisions and validation still needed

The recommendation does not change product scope: no independent output verification, no process-exit gate, no old-execution coordination, ordinary review reports, JSON templates, and minimal event channels/topics only.

Select the primary agent adapter after reviewing this evidence. The current [MVP interface contract](../interfaces/mvp-contract.md) already models the recommended CLI/local-RPC path, but remains a proposal. The shared service operation is the stable seam regardless of adapter choice.

No installed-agent configuration, real-agent call, MCP host registration, runtime process, or native UI was exercised. Source paths, protocol documentation, and document structure were checked. Before claiming compatibility, test the selected adapter in the actual supported launch path; do not infer it from the protocol name alone.
