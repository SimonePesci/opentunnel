# OpenTunnel

OpenTunnel is an open-source tunneling project written in Rust. The long-term
goal is to provide an ngrok-style workflow for exposing local services through a
public tunnel, while keeping the codebase small enough to learn from.

> Status: early restart. The server command can bind a local TCP listener, and
> the expose command can check whether a local service and an OpenTunnel server
> are reachable. Expose sends a small handshake that the server parses and
> acknowledges. The expose control connection stays open after registration,
> and the server tracks active expose sessions across connection threads. The
> server rejects a second expose for the same local port while one is already
> active. Tunneling behavior is not implemented yet.

## Goals

- Expose a local TCP service through a remote public endpoint.
- Keep the protocol and networking code readable.
- Prefer small, reviewable commits over large rewrites.

## Quick Start

Install Rust from <https://rustup.rs/>, then run:

```sh
cd opentunnel
cargo run
```

Expected output:

```text
OpenTunnel

Usage:
  opentunnel --help
  opentunnel --version
  opentunnel server --listen <port>
  opentunnel expose --local <port> --server <address>
```

You can also run:

```sh
cargo run -- --version
cargo run -- server --listen 8080
cargo run -- expose --local 3000 --server 127.0.0.1:8080
```

The server listens on `127.0.0.1` and runs until stopped with `Ctrl-C`.
The expose command expects a service to already be listening on the selected
local port and an OpenTunnel server address such as `127.0.0.1:8080`.
After connecting, expose sends `EXPOSE <local-port>` to the server and expects
`OK` back. After `OK`, expose keeps the control connection open until stopped.
The server registers active expose sessions and removes them when they
disconnect. If the same local port is already active, the server returns `ERR`.

## Architecture

```mermaid
graph TD
    subgraph "opentunnel binary"
        main[main.rs]
        cli[cli.rs]
        protocol[protocol.rs]
        expose[expose.rs]
        server[server.rs]
    end

    main --> cli
    cli --> expose
    cli --> server
    expose --> protocol
    server --> protocol
```

```mermaid
sequenceDiagram
    participant L as Local Service
    participant E as Expose Client
    participant S as Server

    E->>L: TCP connect (verify reachable)
    E->>S: TCP connect
    E->>S: EXPOSE <port>\n
    S->>S: Register session
    S->>E: OK\n
    Note over E,S: Control connection held open
    E--xS: disconnect
    S->>S: Unregister session
```

## Repository Layout

```text
opentunnel/
├── Cargo.toml
└── src/
    ├── cli.rs
    ├── expose.rs
    ├── main.rs
    ├── protocol.rs
    └── server.rs
```

## Roadmap

1. Project structure.
2. CLI shape.
3. Configuration parsing.
4. Local TCP listener.
5. Tunnel protocol.
6. Client/server connection flow.
7. Public tunnel routing.

## Development

This project intentionally moves in small steps. Early commits may skip tests
when the change is only structure or documentation. Once behavior appears, tests
should be added close to the code that introduces it.
