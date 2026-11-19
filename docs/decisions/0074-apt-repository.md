# 0074 — A signed apt repository and a one-line installer

## Context

0.1.0 shipped a `.deb` that only CI ever installed. It put the binary
in `/usr/local`, which belongs to the person and not to packages; it
was cross-built for arm64 and never installed there; it carried a
placeholder maintainer address; its postinst created a `ztop` system
user nothing used and enabled the agent's user unit as root, for
root's own session. Nothing published it, and nothing told an
installed copy where to find the next version.

## Decision

- The package installs into `/usr` (`/usr/bin/ztop`,
  `/usr/lib/systemd/user/ztop-agent.service`, a gzipped man page and a
  Debian copyright file), takes its version from the workspace
  `Cargo.toml` plus the commit count as the Debian revision, and gets
  its dependencies from `dpkg-shlibdeps`. The postinst only prints how
  to turn the agent on: it is a per-person service and stays off until
  that person enables it. No system user.
- Each architecture builds natively on Debian 12 (GitHub's amd64 and
  arm64 runners, in a `debian:bookworm` container), so the C library
  floor is bookworm's and both packages are install-tested.
- The package carries zTop's apt source
  (`https://downloads.bristleconeit.com/ztop/apt`, suite `stable`,
  component `main`) as a conffile, and the public key that signs it,
  pinned with `Signed-By`. The key is the shared "Bristlecone IT
  Services apt signing" key, so one key signs every Bristlecone Linux
  package.
- This repository builds and tests the packages and keeps them as
  the Packaging workflow's artifacts; it holds no signing key and no
  credentials for the downloads site. Publishing happens outside this
  repository: the maintainers' publishing job downloads a Packaging
  run's two packages, signs the repository with
  `packaging/build-apt-repo.sh` at that run's commit, and uploads the
  pool (never replaced), the by-hash indexes, then the signed
  `Release`, `Release.gpg` and `InRelease` last, then the latest links
  and the installer. The signing secret is the subkey alone.
- The one-line installer (`curl -fsSL
  https://downloads.bristleconeit.com/ztop/install.sh | sh`) runs as a
  function called on its last line, so a truncated download runs
  nothing; it checks the package's SHA-256 before `apt-get install`.
- `packaging/check-deb.sh` tests all of it in CI against a local copy of
  the downloads site and a repository signed with a throwaway key,
  including a bad checksum and a tampered `InRelease`, both refused.

## Consequences

- `sudo apt upgrade` updates ztop once a release is published, on
  amd64 and arm64.
- Upgrading from the 0.1.0 `.deb` moves the files from `/usr/local` to
  `/usr`; the `ztop` user it created stays until removed by hand
  (`sudo userdel -r ztop`).
- The downloads site, its credentials and the signing subkey are
  outside this repository, and none of them is ever added to it: this
  repository is public. A key rotation means committing the new public
  key here before the next publish.
- This record was first numbered 0065, the number the Linux desktop
  app's first decision also took; it was renumbered 0074.
