# Security policy

## Supported versions

Kynex is pre-1.0; only the [latest release](https://github.com/mrbergeron83/kynex/releases/latest)
gets security fixes. Kynex updates itself, and the install scripts always fetch the latest release.

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through GitHub:
[Security → Report a vulnerability](https://github.com/mrbergeron83/kynex/security/advisories/new).

Include what an attacker could do, the steps or a proof of concept, and your OS and Kynex version.
You'll get an answer within a week. Once a fix is released, the advisory is published with credit
to you unless you'd rather stay anonymous.

## Scope

In scope: Kynex itself, its install scripts (`install.ps1`, `install.sh`), its self-updater, and
the release pipeline. Problems in Claude Code itself go to
[Anthropic](https://www.anthropic.com/responsible-disclosure-policy); problems in a dependency go
to that project (tell us too, so we can update).

## Verifying downloads

Every release has a `SHA256SUMS` file, which the install scripts and the updater check, and a
build provenance attestation for each archive:

```sh
gh attestation verify kynex-x86_64-pc-windows-msvc.zip --repo mrbergeron83/kynex
```
