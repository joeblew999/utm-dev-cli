/// WinRM SOAP client — no Python/pywinrm dependency.
/// Ported from _winrm.ts.  Uses HTTP Basic auth over plain HTTP (port 5985).
use anyhow::{Context, Result, bail};
use base64::Engine;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RESOURCE_CMD: &str = "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/cmd";

pub struct CmdResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct WinRM {
    url: String,
    auth: String,
    agent: ureq::Agent,
}

impl WinRM {
    pub fn new(host: &str, port: u16, user: &str, pass: &str) -> Result<Self> {
        let url = format!("http://{host}:{port}/wsman");
        let creds = format!("{user}:{pass}");
        let auth = base64::engine::general_purpose::STANDARD.encode(creds.as_bytes());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        Ok(Self { url, auth, agent })
    }

    // ── SOAP envelope ────────────────────────────────────────────────────────

    fn envelope(&self, action: &str, body: &str, shell_id: Option<&str>) -> String {
        let selectors = match shell_id {
            Some(id) => format!(
                r#"<wsman:SelectorSet><wsman:Selector Name="ShellId">{id}</wsman:Selector></wsman:SelectorSet>"#
            ),
            None => String::new(),
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing"
            xmlns:wsman="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd"
            xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell">
  <s:Header>
    <wsa:To>{url}</wsa:To>
    <wsman:ResourceURI s:mustUnderstand="true">{RESOURCE_CMD}</wsman:ResourceURI>
    <wsa:ReplyTo><wsa:Address s:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</wsa:Address></wsa:ReplyTo>
    <wsa:Action s:mustUnderstand="true">{action}</wsa:Action>
    <wsman:MaxEnvelopeSize s:mustUnderstand="true">512000</wsman:MaxEnvelopeSize>
    <wsa:MessageID>uuid:{msg_id}</wsa:MessageID>
    <wsman:OperationTimeout>PT600S</wsman:OperationTimeout>
    {selectors}
  </s:Header>
  <s:Body>{body}</s:Body>
</s:Envelope>"#,
            url = self.url,
            msg_id = new_message_id(),
        )
    }

    fn request(&self, xml: &str) -> Result<String> {
        let resp = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/soap+xml;charset=UTF-8")
            .header("Authorization", &format!("Basic {}", self.auth))
            .send(xml)
            .context("WinRM HTTP request")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.into_body().read_to_string().unwrap_or_default();
            let snippet = &text[..text.len().min(400)];
            bail!("WinRM HTTP {status}: {snippet}");
        }
        resp.into_body()
            .read_to_string()
            .context("reading WinRM response")
    }

    // ── Shell lifecycle ──────────────────────────────────────────────────────

    fn create_shell(&self) -> Result<String> {
        let body = r#"<rsp:Shell xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell">
      <rsp:InputStreams>stdin</rsp:InputStreams>
      <rsp:OutputStreams>stdout stderr</rsp:OutputStreams>
    </rsp:Shell>"#;
        let resp = self.request(&self.envelope(
            "http://schemas.xmlsoap.org/ws/2004/09/transfer/Create",
            body,
            None,
        ))?;
        let id = extract_tag(&resp, "ShellId");
        if id.is_empty() {
            bail!("WinRM: failed to create shell (ShellId missing in response)");
        }
        Ok(id)
    }

    fn delete_shell(&self, shell_id: &str) {
        let _ = self.request(&self.envelope(
            "http://schemas.xmlsoap.org/ws/2004/09/transfer/Delete",
            "",
            Some(shell_id),
        ));
    }

    // ── Command execution ────────────────────────────────────────────────────

    fn exec_command(&self, shell_id: &str, command: &str, args: &[&str]) -> Result<CmdResult> {
        let args_xml: String = args
            .iter()
            .map(|a| format!("<rsp:Arguments>{}</rsp:Arguments>", escape_xml(a)))
            .collect();
        let body = format!(
            r#"<rsp:CommandLine xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell">
      <rsp:Command>{cmd}</rsp:Command>{args_xml}
    </rsp:CommandLine>"#,
            cmd = escape_xml(command),
        );
        let resp = self.request(&self.envelope(
            "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/Command",
            &body,
            Some(shell_id),
        ))?;
        let command_id = extract_tag(&resp, "CommandId");

        let recv_body = format!(
            r#"<rsp:Receive xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell" SequenceId="0">
      <rsp:DesiredStream CommandId="{command_id}">stdout stderr</rsp:DesiredStream>
    </rsp:Receive>"#
        );

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = -1i32;

        loop {
            let recv = self.request(&self.envelope(
                "http://schemas.microsoft.com/wbem/wsman/1/windows/shell/Receive",
                &recv_body,
                Some(shell_id),
            ))?;
            let streams = extract_streams(&recv);
            stdout.push_str(&streams.0);
            stderr.push_str(&streams.1);

            if let Some(pos) = recv.find("ExitCode>") {
                let after = &recv[pos + "ExitCode>".len()..];
                if let Some(end) = after.find('<') {
                    exit_code = after[..end].trim().parse().unwrap_or(-1);
                    break;
                }
            }
            if recv.contains("CommandState") && recv.contains("Done") {
                break;
            }
        }

        Ok(CmdResult {
            stdout: stdout.trim().to_string(),
            stderr: stderr.trim().to_string(),
            exit_code,
        })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Run a PowerShell script (encoded as UTF-16LE to handle special chars).
    pub fn run_ps(&self, script: &str) -> Result<CmdResult> {
        let utf16: Vec<u8> = script
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
        let shell_id = self.create_shell()?;
        let result = self.exec_command(
            &shell_id,
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded],
        );
        self.delete_shell(&shell_id);
        result
    }

    /// Run PowerShell as SYSTEM via a scheduled task (bypasses UAC token filtering).
    /// Returns true if the task completed within `timeout_secs`.
    ///
    /// Completion detection: the user's script is wrapped in a try/finally
    /// that ALWAYS writes a sentinel file (`C:\bootstrap-step-done.txt`) at
    /// the end. The poll loop watches for that sentinel via `Test-Path` —
    /// far more reliable than `(Get-ScheduledTask).State`, which can stay
    /// "Running" minutes after the action's actual process exits (observed
    /// repeatedly during the VS Build Tools install on ARM64). If sentinel
    /// detection fails (WinRM dropping during heavy I/O), we fall back to
    /// the scheduled-task state check; if both fail, we keep polling until
    /// timeout.
    pub fn run_elevated(&self, ps_code: &str, timeout_secs: u64) -> Result<bool> {
        // Wrap user code so the sentinel is written even if the user code
        // throws. $LASTEXITCODE survives across PowerShell try/finally for
        // native processes. We don't propagate the exit code anywhere yet,
        // but it's there if a future caller wants it.
        let wrapped = format!(
            "$ErrorActionPreference = 'Continue'\n\
             try {{\n{ps_code}\n}} finally {{\n  \
                Set-Content 'C:\\bootstrap-step-done.txt' \"$LASTEXITCODE\" -Force\n\
             }}"
        );

        // Write script to disk first (avoids command-line quoting issues).
        // Pre-clear the sentinel from any prior run so its presence below is
        // unambiguous.
        let write_ps = format!(
            "Remove-Item 'C:\\bootstrap-step-done.txt' -Force -ErrorAction SilentlyContinue\n\
             @'\n{wrapped}\n'@ | Set-Content 'C:\\bootstrap-step.ps1' -Force"
        );
        let w = self.run_ps(&write_ps)?;
        if w.exit_code != 0 {
            bail!(
                "run_elevated: failed to write bootstrap script: {}",
                w.stderr
            );
        }

        let register = r"
$action    = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\bootstrap-step.ps1'
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -RunLevel Highest
Register-ScheduledTask -TaskName 'BootstrapStep' -Action $action -Principal $principal -Force | Out-Null
Start-ScheduledTask -TaskName 'BootstrapStep'
";
        self.run_ps(register)?;

        let started = SystemTime::now();
        let deadline = started + Duration::from_secs(timeout_secs);
        while SystemTime::now() < deadline {
            std::thread::sleep(Duration::from_secs(5));
            let elapsed = SystemTime::now()
                .duration_since(started)
                .unwrap_or_default()
                .as_secs();
            print!(
                "\r    ... [{:>4}s / {}s] still running",
                elapsed, timeout_secs
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());

            // Primary: sentinel file. Secondary: scheduled-task state.
            // Fold both into one PS call so we only pay for one WinRM round-trip.
            let probe = self.run_ps(
                "if (Test-Path 'C:\\bootstrap-step-done.txt') { 'DONE' } \
                 else { (Get-ScheduledTask -TaskName 'BootstrapStep' -ErrorAction SilentlyContinue).State }",
            );
            // Err is ignored — WinRM may drop during heavy I/O; keep polling.
            if let Ok(r) = probe {
                let s = r.stdout.trim();
                if s == "DONE" || (!s.is_empty() && s != "Running") {
                    println!(); // newline after carriage-return progress
                    break;
                }
            }
        }

        // Best-effort cleanup
        let _ = self.run_ps(
            "Unregister-ScheduledTask -TaskName 'BootstrapStep' -Confirm:$false -ErrorAction SilentlyContinue",
        );
        let _ = self.run_ps(
            "Remove-Item 'C:\\bootstrap-step.ps1' -Force -ErrorAction SilentlyContinue;\
             Remove-Item 'C:\\bootstrap-step-done.txt' -Force -ErrorAction SilentlyContinue",
        );
        Ok(true)
    }

    /// Cheap reachability probe — tries a GET (any HTTP response = WinRM is up).
    pub fn ping(&self) -> bool {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .build()
            .into();
        agent.get(&self.url).call().is_ok()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn new_message_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let c = c as u128;
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (t >> 32) as u32,
        (t >> 16) as u16 ^ c as u16,
        t as u16 & 0xfff,
        0x8000u16 | ((t ^ c) as u16 & 0x3fff),
        (t ^ (c << 32)) & 0xffff_ffffffff,
    )
}

fn extract_tag(xml: &str, tag: &str) -> String {
    for prefix in &["rsp:", "wsman:", ""] {
        let open = format!("<{prefix}{tag}>");
        if let Some(start) = xml.find(&open) {
            let after = &xml[start + open.len()..];
            if let Some(end) = after.find('<') {
                return after[..end].to_string();
            }
        }
    }
    String::new()
}

fn extract_streams(xml: &str) -> (String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let mut pos = 0;
    while let Some(rel) = xml[pos..].find("<rsp:Stream") {
        let abs = pos + rel;
        let tag_end = match xml[abs..].find('>') {
            Some(e) => abs + e + 1,
            None => break,
        };
        let tag = &xml[abs..tag_end];
        let close = match xml[tag_end..].find("</") {
            Some(e) => tag_end + e,
            None => break,
        };
        let data = xml[tag_end..close].trim();
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
            if tag.contains(r#"Name="stdout""#) {
                stdout.extend_from_slice(&decoded);
            } else if tag.contains(r#"Name="stderr""#) {
                stderr.extend_from_slice(&decoded);
            }
        }
        pos = close;
    }

    (
        String::from_utf8_lossy(&stdout).trim().to_string(),
        String::from_utf8_lossy(&stderr).trim().to_string(),
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
