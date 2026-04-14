# Bash CLI Project — Specs Overview

Archived reference: superseded by [17. riptsk Codebase Structure](17-codebase-structure.md).

## Directory Structure

```
my-cli/
├── bin/
│   └── my-cli              # entry point (thin shim, sources lib)
├── lib/
│   ├── core.sh             # core logic
│   ├── utils.sh            # shared helpers (logging, error handling)
│   ├── commands/
│   │   ├── cmd_foo.sh
│   │   └── cmd_bar.sh
│   └── completions/
│       └── my-cli.bash     # bash completion script
├── man/
│   └── my-cli.1            # man page (optional but clean)
├── tests/
│   └── test_core.bats      # using bats-core
├── install.sh
├── uninstall.sh
├── justfile
└── README.md
```

---

## Entry Point Pattern (`bin/my-cli`)

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="${SCRIPT_DIR}/../lib"

source "${LIB_DIR}/utils.sh"
source "${LIB_DIR}/core.sh"

main "$@"
```

Keep `bin/` as a thin dispatcher — no logic lives there.

---

## Key Bash Flags (every file)

```bash
set -euo pipefail
IFS=$'\n\t'
```

---

## Architecture Patterns

- **Subcommand dispatch** via `case "$1"` in `core.sh`, sourcing `commands/cmd_*.sh` lazily
- **Namespaced functions**: `mycli::utils::log`, `mycli::cmd::foo` to avoid collisions
- **`readonly` globals** for constants; `local` for everything inside functions
- **Centralized error handler** via `trap 'error_handler $? $LINENO' ERR`

---

## Install / Uninstall

`install.sh` should:
- Support `PREFIX` override (default `/usr/local`)
- Copy `bin/my-cli` → `$PREFIX/bin/`
- Copy `lib/` → `$PREFIX/lib/my-cli/` (or embed inline for single-file distro)
- Install completion → `/etc/bash_completion.d/` or `$PREFIX/share/bash-completion/completions/`
- Install man page → `$PREFIX/share/man/man1/`
- Optionally: detect if running as root vs user-local (`~/.local`)

`uninstall.sh` is the exact inverse — track installed paths explicitly (a manifest file or hardcoded list).

`justfile` wraps `install`/`uninstall`/`test`/`lint` targets.

---

## Testing

Use **[bats-core](https://github.com/bats-core/bats-core)** — de facto standard for bash unit/integration tests.

```bash
@test "foo command outputs expected result" {
  run my-cli foo --bar
  assert_output "expected"
  assert_success
}
```

---

## Linting / Static Analysis

- **[shellcheck](https://www.shellcheck.net/)** — mandatory, run in CI
- **[shfmt](https://github.com/mvdan/sh)** — formatting, treat as `gofmt`

---

## Distribution Options

| Approach | Use case |
|---|---|
| Multi-file + installer | Full projects, teams |
| Single bundled file (`shc` or manual concat) | Easy curl-pipe installs |
| Nix/AUR/deb package | If targeting distro packaging |

---

## Summary of Non-Negotiables

1. `set -euo pipefail` everywhere
2. `shellcheck` clean
3. Namespaced functions
4. `bats-core` tests
5. `PREFIX`-aware installer

Want me to scaffold the actual boilerplate?
