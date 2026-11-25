//! The real `Effects` and `Platform`: what a TV session does to this PC.

use super::{audio, cursor, input, proc, screens};
use crate::bindings::InputEvent;
use crate::log;
use crate::server::Platform;
use crate::session::{Effects, ShutdownKind};
use crate::settings::Scope;
use crate::state::Shared;
use std::net::IpAddr;
use std::sync::Arc;

pub struct RealEffects {
    shared: Arc<Shared>,
}

impl Effects for RealEffects {
    fn send_input(&self, events: &[InputEvent]) {
        input::send(events);
    }

    fn absolute(&self, display: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
        screens::absolute(display, x, y)
    }

    fn scroll_excluded(&self) -> bool {
        proc::foreground_process_name().is_some_and(|name| self.shared.is_scroll_excluded(&name))
    }

    fn audio_is_local(&self) -> bool {
        audio::default_is_local()
    }

    fn pointer_shown(&self, shown: bool) {
        cursor::pointer_shown(shown);
    }

    fn run_command(&self, command: &str) {
        // As the signed-in user: a binding saved through the web UI must not run with SYSTEM rights
        if let Err(e) = proc::spawn_as_user(&proc::system_dir().join("cmd.exe"), &format!("/c {command}"), false) {
            log::warn(format!("Command binding \"{command}\" not run: {e}"));
        }
    }

    fn shutdown(&self, kind: ShutdownKind) {
        let args = match kind {
            ShutdownKind::Now => "/s /t 0",
            ShutdownKind::InFiveMinutes => "/s /t 300",
            ShutdownKind::Abort => "/a",
        };
        if let Err(e) = proc::spawn_hidden(&proc::system_dir().join("shutdown.exe").to_string_lossy(), args) {
            log::error(format!("shutdown {args} failed: {e}"));
        }
    }

    fn idle_ms(&self) -> Option<u32> {
        input::idle_ms()
    }
}

pub struct RealPlatform {
    pub shared: Arc<Shared>,
    pub scope: Scope,
}

impl Platform for RealPlatform {
    fn effects(&self) -> Box<dyn Effects + Send> {
        Box::new(RealEffects { shared: self.shared.clone() })
    }

    fn display_for(&self, ip: IpAddr) -> Option<u32> {
        let mappings = self.scope.displays();
        // Only ask ares which TV this is (it starts Node) when some TV maps to a display
        if mappings.iter().all(|(_, d)| *d == 0) {
            return None;
        }
        let IpAddr::V4(ip) = ip else { return None };
        let device = crate::tv::Ares::new().ok()?.device_at(ip)?;
        mappings.into_iter().find(|(name, _)| *name == device.name).map(|(_, d)| d)
    }
}
