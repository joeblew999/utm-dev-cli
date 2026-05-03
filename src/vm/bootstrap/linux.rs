//! Linux bootstrap — apt-driven, ssh transport.

use anyhow::Result;

use crate::vm::profiles::{BootstrapMode, VmProfile};
use crate::vm::ssh;

pub(super) fn run(session: &ssh::Session, profile: &VmProfile) -> Result<()> {
    println!(
        "→ Bootstrapping Linux VM (mode: {:?})...",
        profile.bootstrap
    );

    // Install host's public key so the user can `code --remote ssh-remote+...`
    // (and re-runs of `vm exec`) without password prompts. Idempotent.
    install_host_pubkey(session)?;

    if profile.bootstrap == BootstrapMode::SshOnly {
        let out = ssh::exec(session, "echo ok")?;
        if out.contains("ok") {
            println!("✓ SSH verified");
        }
        return Ok(());
    }

    // Full bootstrap — check before each step (idempotent)

    // Step 1: build-essential + curl + git
    let installed = ssh::exec(
        session,
        "dpkg -s build-essential 2>/dev/null | grep -c 'ok installed'",
    )
    .unwrap_or_default();
    if installed.trim() != "1" {
        run_step(
            session,
            "update packages",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq",
        )?;
        run_step(
            session,
            "install build deps",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
             build-essential curl git pkg-config",
        )?;
    } else {
        println!("  ✓ build-essential already installed");
    }

    // Step 2: Tauri Linux dependencies + observability tools.
    //
    // We check Xvfb specifically (not libwebkit2gtk) because Xvfb is the
    // newest addition — older bootstrapped VMs WILL have libwebkit2gtk
    // already and would skip the whole apt-install if we keyed on that,
    // missing xvfb / scrot. apt-get install is idempotent — passing tools
    // that are already installed is a no-op, fast (~1s).
    let xvfb_present = ssh::exec(
        session,
        "command -v Xvfb >/dev/null 2>&1 && echo present || echo missing",
    )
    .unwrap_or_default();
    if !xvfb_present.contains("present") {
        // libwebkit2gtk + GTK family — Tauri build deps.
        // xvfb: virtual framebuffer X server for headless GUI launches
        //       (`vm run` uses Xvfb on :99 so apps boot without a display).
        // scrot: tiny screenshot tool — `vm screenshot` captures the
        //        xvfb display and scp's the png back.
        // xdg-utils: xdg-open, required by Tauri's AppImage bundler.
        // openbox: tiny window manager (~1 MB). Without a WM, GTK windows
        // open but don't get mapped/composited on bare Xvfb, so vm screenshot
        // returns a black png. With openbox running on :99, windows appear.
        run_step(
            session,
            "install Tauri Linux deps + xvfb + scrot + openbox",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
             libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
             librsvg2-dev libssl-dev libxdo-dev patchelf wget file \
             libsoup-3.0-dev libjavascriptcoregtk-4.1-dev xvfb xdg-utils \
             scrot openbox",
        )?;
    } else {
        println!("  ✓ Tauri deps + xvfb + scrot already installed");
    }

    // Step 3: mise
    let mise = ssh::exec(
        session,
        "~/.local/bin/mise --version 2>/dev/null || mise --version 2>/dev/null || echo missing",
    )
    .unwrap_or_default();
    if mise.contains("missing") || mise.is_empty() {
        run_step(session, "install mise", "curl https://mise.run | sh")?;
    } else {
        println!("  ✓ mise already installed ({})", mise.trim());
    }
    run_step(
        session,
        "activate mise in .bashrc",
        r#"grep -q 'mise activate' ~/.bashrc || echo 'eval "$(~/.local/bin/mise activate bash)"' >> ~/.bashrc"#,
    )?;

    // (Rust is installed by the project's mise.toml at vm build time, not
    // here — see AGENTS.md "Source-of-truth invariant for VM bootstrap".)

    // Step 3b: cargo-binstall + mise's cargo_binstall setting.
    // mirrors what the Windows bootstrap (step 7a/7b) does — installs the
    // cargo-binstall binary directly (no compile) and persists the mise
    // setting so cargo: tools fetch prebuilt binaries from GitHub releases
    // instead of compiling from source.
    let binstall_present = ssh::exec(
        session,
        "[ -x \"$HOME/.cargo/bin/cargo-binstall\" ] && echo present || echo missing",
    )
    .unwrap_or_default();
    if !binstall_present.contains("present") {
        let arch = ssh::exec(session, "uname -m").unwrap_or_default();
        let target = if arch.trim() == "aarch64" {
            "aarch64-unknown-linux-musl"
        } else {
            "x86_64-unknown-linux-musl"
        };
        let url = format!(
            "https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-{target}.tgz"
        );
        run_step(
            session,
            "install cargo-binstall (binstall fast-path)",
            &format!(
                "mkdir -p ~/.cargo/bin && curl -sSfL {url} | tar -xz -C ~/.cargo/bin && chmod +x ~/.cargo/bin/cargo-binstall"
            ),
        )?;
    } else {
        println!("  ✓ cargo-binstall already installed");
    }
    // mise config: cargo_binstall = true (idempotent).
    run_step(
        session,
        "configure mise cargo_binstall = true",
        "mkdir -p ~/.config/mise && \
         touch ~/.config/mise/config.toml && \
         (grep -q 'cargo_binstall' ~/.config/mise/config.toml || \
          printf '\\n[settings]\\ncargo_binstall = true\\n' >> ~/.config/mise/config.toml)",
    )?;

    // Step 4: linux-dev extras (Debian 12 with GNOME).
    // Marker is fonts-noto-color-emoji because xdg-utils is already
    // installed for ALL Linux profiles by step 2.
    if profile.name == "linux-dev" {
        let emoji = ssh::exec(
            session,
            "dpkg -s fonts-noto-color-emoji 2>/dev/null | grep -c 'ok installed'",
        )
        .unwrap_or_default();
        if emoji.trim() != "1" {
            run_step(
                session,
                "install desktop extras (fonts-noto-color-emoji)",
                "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
                 fonts-noto-color-emoji",
            )?;
        } else {
            println!("  ✓ desktop extras already installed");
        }
    }

    println!("✓ Linux bootstrap complete");
    Ok(())
}

fn run_step(session: &ssh::Session, label: &str, cmd: &str) -> Result<()> {
    print!("  {label}...");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let (out, code) = ssh::exec_with_exit(session, cmd)?;
    if code != 0 {
        println!(" ✗ (exit {code})");
        eprintln!("    {out}");
    } else {
        println!(" ✓");
    }
    Ok(())
}

fn install_host_pubkey(session: &ssh::Session) -> Result<()> {
    let pub_key = match super::find_public_key() {
        Ok(k) => k,
        Err(_) => {
            println!(
                "  ⚠ no SSH public key in ~/.ssh — VS Code Remote SSH will prompt for password"
            );
            return Ok(());
        }
    };
    // Quote-safe single-line shell pipeline. grep -qxF avoids partial-line matches.
    let cmd = format!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && \
         chmod 600 ~/.ssh/authorized_keys && \
         grep -qxF {key} ~/.ssh/authorized_keys || echo {key} >> ~/.ssh/authorized_keys",
        key = shell_quote(&pub_key)
    );
    let (out, code) = ssh::exec_with_exit(session, &cmd)?;
    if code != 0 {
        println!("  ⚠ failed to install host pubkey (exit {code}): {out}");
    } else {
        println!("  ✓ host SSH key authorised (passwordless `code --remote` ready)");
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    // Wrap in single quotes; escape any embedded single quotes by closing,
    // adding an escaped quote, and reopening.
    format!("'{}'", s.replace('\'', r"'\''"))
}
