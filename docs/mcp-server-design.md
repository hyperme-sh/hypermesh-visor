# Visor as an MCP server

Design for [#169](https://github.com/hyperme-sh/hypermesh-visor/issues/169).
This note does not authorize implementation or enable a listener. The HTTP MCP
paths below are proposed; today's optional daemon HTTP API is not an MCP endpoint.

## Transport and configuration

Reuse the inverse adapter's capability discovery, verbs, resources, task controls
and structured errors. Local stdio retains the protected Unix socket and assigned
session boundary. It opens no network listener. A native Windows stdio client
currently needs an explicitly configured remote daemon; it is not a local daemon.
See [the portable client](../inverse-client/README.md).

Proposed Streamable HTTP listens at `/mcp`; optional legacy SSE listens at `/sse`
with a same-origin message endpoint. Both share one dispatcher and authorization
path. Default binding is loopback. Advertise only explicitly enabled transports
and reachable operator-configured URLs, never wildcard bind addresses or secrets.
Remote access requires an explicit bind, scoped authentication and HTTPS or an
authenticated private tunnel. Restrict Host/Origin values, reject cross-origin
browser requests, bind protocol session IDs to credentials, and recheck revoked
credentials on every request, including SSE reconnect and message submission.

AnythingLLM accepts the following configuration shape in the storage `plugins`
directory's `anythingllm_mcp_servers.json`, or through its MCP management UI.
`autoStart: false` makes startup on demand. HTTP transports use `url`, not
`command`; always specify `type` because omitted HTTP types default to SSE.
See [the transport guide](https://docs.anythingllm.com/mcp-compatibility/overview).

Desktop stdio template (replace the path and assigned session):

```json
{
  "mcpServers": {
    "hypermesh-visor": {
      "command": "/absolute/path/to/hypermesh-desktop-mcp",
      "env": { "HYPERMESH_SESSION_ID": "<assigned-session-id>" },
      "anythingllm": { "autoStart": false }
    }
  }
}
```

Desktop HTTP template for a future authorized implementation (replace port and
token in private client settings; this is not usable against today's daemon):

```json
{
  "mcpServers": {
    "hypermesh-visor": {
      "type": "streamable",
      "url": "http://127.0.0.1:<visor-mcp-port>/mcp",
      "headers": { "X-API-KEY": "<scoped-token>" },
      "anythingllm": { "autoStart": false, "forwardIdentity": false }
    }
  }
}
```

Docker template for the same future endpoint on a restricted host bridge:

```json
{
  "mcpServers": {
    "hypermesh-visor": {
      "type": "streamable",
      "url": "http://host.docker.internal:<visor-mcp-port>/mcp",
      "headers": { "X-API-KEY": "<scoped-token>" },
      "anythingllm": { "autoStart": false, "forwardIdentity": false }
    }
  }
}
```

Container localhost refers to the container. AnythingLLM's Linux guide uses
`172.17.0.1` instead of `host.docker.internal`; verify the actual bridge gateway
on custom networks. A loopback-only listener is not reachable through that bridge.
Choose the specific bridge bind and firewall access for this client, retaining
authentication; use HTTPS/tunneling when the bridge is not trusted. Do not solve
reachability by exposing an unauthenticated wildcard listener. See
[Docker addressing](https://docs.anythingllm.com/installation-docker/localhost).
For SSE, change `type` to `sse` and the proposed path to `/sse` only when that
transport is enabled. No template contains a real credential.

## Authentication and dispatch

Require a Visor-issued scoped token or shared secret for every HTTP endpoint,
including initialization and discovery. Missing or invalid credentials fail even
when identity headers are present. A shared credential identifies the client
instance, not each human using it. Use separate grants where users need different
authority; never infer those grants from a forwarded username or role.

AnythingLLM can forward user/workspace fields in `X-AnythingLLM-*` and `_meta`.
These are untrusted hints for bounded display or narrowing already authorized
scope. They cannot authenticate, select another owner's session, or widen RBAC.
Keep forwarding disabled unless needed; avoid retaining personal fields by
default. See [identity forwarding](https://docs.anythingllm.com/mcp-compatibility/forward-identity).

The proposed dispatch sequence is credential validation, scoped capability
lookup, argument validation, AIuditor decision, and the existing Visor session,
RBAC, approval, pause and revocation checks immediately before execution. Denial
prevents dispatch; replacement arguments require validation and authorization of
the replacement operation. Filter discovery by scope and platform support, but
never treat discovery as permission to execute. Cancellation and queued bot tasks
must enforce the same owner/session boundary. Reconnection never replays an
uncertain mutating call automatically.

The remote AIuditor decision adapter is an implementation prerequisite, not a
claim that every current desktop route already provides this contract. Missing,
expired or unavailable policy decisions fail closed. The new server cannot be
enabled until that adapter and revocation checks are implemented. Forum bot
reports are not human approval authority; an independently authenticated operator
decision and current desktop consent remain necessary.

## Existing ownership and implementation checks

| Work | Relationship |
| --- | --- |
| #28 | Reuse the desktop capability/verb contract; no parallel tool catalog. |
| #29 | Discovers other applications; this server exposes Visor in the opposite direction. |
| #32 | Reuse the implemented inverse stdio adapter and its protected local transport. |
| #99 | Headless hosts advertise only supported, opted-in capabilities through the same dispatcher. |
| [Conductor #52](https://github.com/hyperme-sh/hypermesh-conductor/issues/52) | AnythingLLM Generic OpenAI chat routing remains separate from MCP tools. |

A later implementation must add unit/contract coverage for invalid credentials,
forged hints, cross-user protocol sessions, unsupported capabilities, policy
deny/replace, revoked grants, pause, cancelled tasks and uncertain reconnects.
It must document concrete endpoint and credential management commands before
presenting these templates as operational setup. AnythingLLM bundling, Community
Hub publication and changes to third-party discovery are outside this design.
