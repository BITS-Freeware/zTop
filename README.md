# zTop

Resource monitoring for a mixed fleet of Macs and Linux machines.

- **`ztop`**, a terminal utility for macOS (Homebrew) and Linux (apt). Run it on its own and it is a full interactive monitor, at least the equal of `htop` for processes and of `asitop` for Apple Silicon power, frequency and GPU data. Run `ztop agent` and it serves those metrics over pinned TLS; run `ztop hub` and one machine collects from every agent.
- **zTop for Linux** (`desktop/`), a GTK4 desktop app that watches every machine at once through the agent and hub. See [desktop/README.md](desktop/README.md).
- **zTop**, an app for iPad and Mac that does the same. The app is developed separately.

Status: 0.1.0, an early release. See [ROADMAP.md](ROADMAP.md) for the plan, [DESIGN.md](DESIGN.md) for how it works, and [STATUS.md](STATUS.md) for where work stands.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any contribution you submit for inclusion in this work, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or conditions.

## History

zTop was first built in a personal working repository and moved here for its 0.1.0 release. Earlier, that repository held an end-to-end encrypted messenger, now [Thrum](https://github.com/BITS-Freeware/Thrum).
