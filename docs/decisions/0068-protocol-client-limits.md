# 0068 — The protocol client reads bytes, within limits

## Context

The audit of 0.1.0 (finding M1) showed that the protocol client in
`ztop_core::client`, used by `ztop connect` and by every hub poll,
trusted the server it talked to:

- `dechunk` and the `Content-Length` branch cut a `&str` at a byte
  count the server announced. A count that lands inside a multi-byte
  character panicked ("end byte index 1 is not a char boundary"). The
  TUI installed no panic hook, so `ztop connect` crashed and left the
  terminal in raw mode.
- The response was read with `read_to_end` and no cap, so a server
  that never stops sending grew the client without bound.
- The socket had per-read timeouts only. A server that sends one byte
  inside each timeout kept a fetch open forever.
- `TcpStream::connect` had no timeout of its own.

## Decision

- `parse_response` works on bytes. The body is cut as bytes and only
  then checked as UTF-8; every malformed shape is an error. It is
  public so that a new fuzz target, `parse_response`, can hold it to
  that.
- A response is at most `MAX_RESPONSE_BYTES` (32 MiB, headers
  included). The largest real answer, an hour of history without
  process lists (decision 0062), is far below it.
- One deadline covers the whole fetch. `Deadlined` wraps the socket
  and gives each read and write only the time left, and the connect
  uses `connect_timeout` with the same budget, address by address.
  The budget stays five seconds for a fetch and three for pairing.
- The TUI installs a panic hook, once per process, that restores the
  terminal before the previous hook prints.

## Consequences

- A hostile or broken server can make a fetch fail, but not panic the
  client, hold it past its timeout, or grow its memory past the cap.
- Name resolution is still outside the deadline; `ToSocketAddrs` has
  no timeout, and zTop targets are usually addresses or local names.
- The fuzz smoke runs eighteen targets.
