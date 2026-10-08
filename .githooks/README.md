# Git hooks

Checks that run on this machine instead of in GitHub Actions. Turn them on once per clone:

```
git config core.hooksPath .githooks
```

| Hook | When | Runs |
|---|---|---|
| `pre-commit` | each commit, for the parts the commit touches | `cargo fmt --check` and `cargo clippy` for Rust; `make -C esphome lint check test` for the firmware (needs `poetry install`) |
| `pre-push` | each push | `cargo test` |

Skip one with `--no-verify`. The firmware's compile (`make -C esphome compile`, a few minutes and a toolchain download)
is not in a hook: run it when the YAML or the device headers change.
