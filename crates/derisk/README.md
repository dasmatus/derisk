# derisk

An adaptive, agent-first Wayland desktop shell built on mcsapi.

The shell and the `derisk` binary: `derisk session`, `display-manager`, `greeter`, `installer`, `setup`, `agent` and the headless commands. Build the compositor with `--features host`. `data/` holds its systemd user units and PAM services.

Part of [derisk](https://github.com/losos-project/derisk), where it is built as
a member of the derisk Cargo workspace; see the README there.

Licensed under GPL-3.0-only (see `LICENSE`).
