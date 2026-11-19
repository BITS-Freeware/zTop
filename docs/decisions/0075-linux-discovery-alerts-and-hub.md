# Decision 0075: discovery, alerts and the hub in the Linux app

Date: 2026-11-19
Status: accepted

## Context

L4 of zTop for Linux has three parts: Bonjour browsing in the add-host
form, alert rules with desktop notifications, and a fleet read through
one hub. Its done-when is three agents read back live through a hub in
a recorded run. The iPad and Mac app already do all three, so the Linux
app follows their rules rather than inventing new ones.

## Decision

- **Hub.** A host record can be a hub (`"hub": true` in hosts.json,
  written only when true). Its card reads `GET /v1/hosts` and says how
  many machines it watches and how many are live. Each machine gets a
  card of its own, placed after the hub's, read through the hub with
  `?host=<name>` (new `RemoteCollector::through_hub` and `hub_hosts` in
  `ztop-core`) and with the hub's token. Machine records are never
  saved: they come and go with the hub's own list. Their detail pages
  read history through the hub. Their process menus stay empty,
  because a hub does not forward control.
- **Bonjour.** The add-host dialog browses `_ztop._tcp` while it is
  open, with `mdns-sd` 0.21.7, the crate and version the agent already
  advertises with. That is this decision's dependency addition, and
  `cargo deny` passes with it. Choosing an agent fills in the name,
  address and port, never the fingerprint, because an advertisement
  proves nothing about who sent it. An agent on this machine is saved
  as `127.0.0.1`, because the default agent binds loopback only. Any
  other agent is saved by its IPv4 address, because a `.local` name
  needs nss-mdns to resolve on Linux. An agent whose TXT record names
  another protocol version is listed but cannot be chosen.
- **Alerts.** The rule kinds (CPU, memory, load, disk, unreachable)
  and their semantics are the Mac app's (that repository's decision
  0050). A rule fires once its condition has held for its duration,
  fires once, and clears when the condition stops. It watches one host
  or all of them. Rules live in `alerts.json` (0600), and the timers
  live in memory only. A fired alert becomes a `GNotification` (high
  priority, withdrawn when it clears) and a toast. The engine sits in
  `ztop-desktop-core::alerts` and is tested with fixed clocks.

## Consequences

- `check.sh` gains `recorded-hub` (three plain agents behind one TLS
  hub, with the app paired only with the hub; it passes when the hub
  and all three machines are live) and `recorded-alerts` (an
  unreachable rule fires when an agent is stopped). The live test
  `three_agents_read_back_live_through_one_hub` also reads each
  machine's snapshot and history through the hub.
- The Bonjour round trip, `an_advertised_agent_is_found_with_its_port_on_loopback`,
  runs only with `ZTOP_TEST_MULTICAST=1`, because a CI container may
  not route multicast. It passed on the development machine.
- The recorded runs check that alerts fire but not that the desktop
  shows them: the test session has no notification server. A run on a
  real desktop session should confirm the notification.
