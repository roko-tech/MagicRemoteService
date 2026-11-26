//! SSH to a TV in Developer Mode, built into the exe (russh), so installing the TV app needs
//! nothing from Windows. The TV runs an OpenSSH from 2012: its host key and our signature are
//! plain `ssh-rsa`, and the key exchange is diffie-hellman-group-exchange-sha256.

use russh::client::{self, Handle};
use russh::keys::{decode_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// After a command was told to stop (stdin closed), how long it gets to finish.
const GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    /// The passphrase doesn't unlock the key.
    WrongPassphrase,
    /// The key isn't one this can read.
    BadKey,
    /// No SSH answer: the TV is off, Developer Mode is off, or the address is wrong.
    Unreachable,
    /// The TV isn't the one first seen at this address (a different host key).
    HostKeyChanged,
    /// The TV rejected the key.
    KeyRefused,
    TimedOut,
    Other(String),
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::Other(m) => f.write_str(m),
            e => write!(f, "{e:?}"),
        }
    }
}

/// Checks the TV's host key against the one remembered for it, and notes the one it showed.
struct Client {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

/// `ssh-rsa AAAA…`, the form `known_hosts` lines use.
fn host_key_text(key: &PublicKeyOrCertificate) -> Option<String> {
    let PublicKeyOrCertificate::PublicKey { key, .. } = key else { return None };
    let text = key.to_openssh().ok()?;
    // Algorithm and key only, without a comment
    Some(text.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let Some(text) = host_key_text(key) else { return Ok(false) };
        let ok = self.expected.as_ref().is_none_or(|e| *e == text);
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
        Ok(ok)
    }
}

/// What a command printed, and how it ended.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<u32>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.exit == Some(0)
    }
}

/// A signed-in SSH session to a TV.
pub struct Session {
    rt: tokio::runtime::Runtime,
    handle: Handle<Client>,
    /// The TV's host key, `ssh-rsa AAAA…`, to remember for next time.
    pub host_key: String,
}

/// Unlocks `key_pem` with `passphrase` and signs in to the TV. With `expected_host_key`, a TV
/// that shows another host key is refused.
pub fn connect(host: Ipv4Addr, port: u16, user: &str, key_pem: &str, passphrase: &str, expected_host_key: Option<&str>) -> Result<Session, SshError> {
    let key = match decode_secret_key(key_pem, Some(passphrase)) {
        Ok(key) => key,
        Err(_) if key_pem.contains("ENCRYPTED") => return Err(SshError::WrongPassphrase),
        Err(_) => return Err(SshError::BadKey),
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| SshError::Other(e.to_string()))?;
    let seen = Arc::new(Mutex::new(None));
    let client = Client { expected: expected_host_key.map(str::to_string), seen: seen.clone() };
    // No inactivity limit: each command has its own timeout, and installing can be quiet for a while
    let config = Arc::new(client::Config::default());
    let handle = rt.block_on(async {
        let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, (IpAddr::V4(host), port), client)).await {
            Err(_) => return Err(SshError::Unreachable),
            Ok(Err(russh::Error::UnknownKey)) => return Err(SshError::HostKeyChanged),
            Ok(Err(russh::Error::IO(_))) => return Err(SshError::Unreachable),
            Ok(Err(e)) => return Err(SshError::Other(format!("SSH: {e}"))),
            Ok(Ok(h)) => h,
        };
        // The TV's OpenSSH only knows SHA-1 RSA signatures (`ssh-rsa`)
        let auth =
            handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None)).await.map_err(|e| SshError::Other(format!("SSH: {e}")))?;
        if !auth.success() {
            return Err(SshError::KeyRefused);
        }
        Ok(handle)
    })?;
    let host_key = seen.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    Ok(Session { rt, handle, host_key })
}

impl Session {
    /// Runs `command` with `input` on its stdin, handing each output line to `stop`. Without
    /// `hold_stdin`, stdin closes after a non-empty input; with it, stdin closes once `stop` says
    /// so (which ends `luna-send -i`), and the command gets a few seconds to finish.
    pub fn exec(&self, command: &str, input: &[u8], hold_stdin: bool, timeout: Duration, mut stop: impl FnMut(&str) -> bool) -> Result<Output, SshError> {
        let other = |e: russh::Error| SshError::Other(format!("SSH: {e}"));
        self.rt.block_on(async {
            let deadline = Instant::now() + timeout;
            let mut channel = self.handle.channel_open_session().await.map_err(other)?;
            channel.exec(true, command).await.map_err(other)?;
            // stdin closes after the input. A command given none keeps it open until it ends:
            // `luna-send` quits on end of input, before printing its answer.
            if !input.is_empty() {
                channel.data(input).await.map_err(other)?;
                if !hold_stdin {
                    channel.eof().await.map_err(other)?;
                }
            }
            let mut out = Output::default();
            let mut pending = String::new();
            let mut stopped_at: Option<Instant> = None;
            loop {
                let limit = stopped_at.map_or(deadline, |t| (t + GRACE).min(deadline));
                let msg = match tokio::time::timeout(limit.saturating_duration_since(Instant::now()), channel.wait()).await {
                    Ok(Some(msg)) => msg,
                    Ok(None) => break,
                    Err(_) if stopped_at.is_some() => break,
                    Err(_) => return Err(SshError::TimedOut),
                };
                match msg {
                    ChannelMsg::Data { data } => {
                        pending.push_str(&String::from_utf8_lossy(&data));
                        while let Some(i) = pending.find('\n') {
                            let line: String = pending.drain(..=i).collect();
                            let line = line.trim_end_matches(['\r', '\n']);
                            out.stdout.push_str(line);
                            out.stdout.push('\n');
                            if stopped_at.is_none() && stop(line) {
                                stopped_at = Some(Instant::now());
                                if hold_stdin {
                                    let _ = channel.eof().await;
                                }
                            }
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => out.stderr.push_str(&String::from_utf8_lossy(&data)),
                    ChannelMsg::ExitStatus { exit_status } => out.exit = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if !pending.is_empty() {
                out.stdout.push_str(&pending);
                let _ = stop(pending.trim_end());
            }
            let _ = channel.close().await;
            Ok(out)
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.rt.block_on(self.handle.disconnect(russh::Disconnect::ByApplication, "", "en"));
    }
}
