# 0064 — Serving survives the network

## Context

The agent and the hub served TLS through their own accept loop and
plain HTTP through `axum::serve`. The TLS loop had four gaps:

- `listener.accept().await?` ended the whole server on the first
  failed accept. Running out of file descriptors, or a connection
  reset before it was taken, stopped the agent until someone
  restarted it.
- The TLS handshake had no deadline. A peer that opened a TCP
  connection and never sent a ClientHello held a task and a file
  descriptor forever.
- hyper's header-read timeout needs a timer, and none was set, so it
  was off. The HTTP/1-or-2 sniff in hyper-util's `auto` builder also
  read from the connection with no deadline. A peer that sent half a
  request head (or nothing) after the handshake was never dropped.
- Nothing capped the number of connections served at once.

The plain path through `axum::serve` retries failed accepts, but it
also has no header deadline and no cap.

## Decision

Both servers, plain and TLS, go through one accept loop,
`ztop_agent::serving::serve`, under `serving::Limits::DEFAULT`:

- A failed accept is logged once, retried after 50 ms, and the wait
  doubles on each failure in a row up to one second. The first
  accept that works logs that serving resumed.
- The TLS handshake must finish within 10 seconds.
- A request head must arrive in full within 10 seconds (hyper's
  `header_read_timeout`, with `TokioTimer` set). The clock starts
  when the server waits for a head, so an idle keep-alive
  connection closes at the same deadline. zTop clients send one
  request per connection with `Connection: close`.
- A request must be answered within 30 seconds, or it is answered
  408. The body is read inside the handler, so this stops a peer
  that promises a body and drips it. A WebSocket answers at once
  with the upgrade, so `/v1/stream` is not cut by it.
- At most 256 connections are served at once. A slot is taken
  before each accept, so at the cap the loop stops accepting and
  new connections wait in the kernel's backlog until one ends.

Only HTTP/1.1 is served. Every zTop client sends HTTP/1.1, and the
TLS configuration offers no ALPN, so HTTP/2 was reachable only as
prior-knowledge h2c. Dropping it removes the unbounded protocol sniff
and the `h2` crate. hyper becomes a direct dependency (features
`server` and `http1`); hyper-util keeps `tokio` and `service` and
drops `server-auto`.

`tls::serve_router_on_tls` is removed; the hub calls `serving::serve`
for both transports. `tls::serve_on_tls` and `serve_on` stay as the
agent's seams and call it too.

## Consequences

- Regression tests in `crates/agent/src/serving.rs` drive the loop
  with in-memory streams, under short limits: an accept error
  followed by a served request, a silent client, a half-sent head,
  a stalled body answered 408, a stalled TLS handshake, and a second
  connection that waits at a cap of one until the first closes. All
  six fail on the old loop.
- A WebSocket stream holds no slot after its upgrade. Streams need a
  token, so this is bounded by who holds tokens, not by the network.
- A peer can still fill the 256 slots for up to the deadlines. The
  cap bounds memory and file descriptors; it is not a defence
  against a flood, which the threat model leaves to the network.
