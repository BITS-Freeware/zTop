# Decision 0066: the Linux app's host detail page

Date: 2026-11-19
Status: accepted

## Context

L2 of zTop for Linux (decision 0065) is the host detail page: history
charts for CPU, memory, load, disks, network, temperatures and fans,
plus a bar per core. The agent already keeps an hour of samples and
serves them at `/v1/history`, but `ztop-core`'s client could only read
a snapshot.

## Decision

- `ztop-core` gains `RemoteCollector::history(seconds)`, sharing the
  snapshot read's TLS, token and protocol-version checks, so the
  desktop app never speaks HTTP itself.
- The page opens with the last 15 minutes from `/v1/history` and then
  appends each sample the fleet poll already reads (every 2 seconds).
  It never asks for the whole window again, so an open page costs the
  agent nothing beyond the poll.
- `History` in `ztop-desktop-core` holds the samples in time order
  without their process lists, skips repeated timestamps (the history
  and the poll overlap) and derives every series, so the chart logic is
  tested without a display. Rates are the agent's own per-sample
  rates; loopback is left out of the network total.
- Charts are cairo drawings on `gtk::DrawingArea`, with no new
  dependency. A chart's scale rounds up to a 1, 2 or 5 step (percent
  charts stay at 100), and its time span grows with the held history,
  from one minute up to the full 15, so a young agent's chart is not a
  sliver at the right edge. A section the host does not report stays
  hidden.
- Recorded runs gain `ZTOP_DESKTOP_DETAIL=<name>` (open that host's
  page once it is live) and `ZTOP_DESKTOP_SIZE`, and `check.sh` gains
  a `recorded-detail` stage.

## Consequences

- The recorded run shows CPU, memory, load, disks, network and the
  per-core bars against a live agent. The machine it ran on has no
  temperature sensors or fans, so those two sections are covered by
  unit tests only, and the parity item stays open until a run on a
  machine with sensors.
- The published screenshots are cropped to leave out the recording
  machine's mount list.
