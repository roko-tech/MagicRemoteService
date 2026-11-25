//! One TV connection after the handshake: frames in, injected input and frames out, plus the
//! keep-alive and inactivity timers. Free of I/O and clocks so it can be tested exactly.

use crate::bindings::{Compiled, InputEvent, Plan};
use crate::log;
use crate::protocol::{self, ClientMessage};
use crate::ws;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const PING_EVERY: Duration = Duration::from_secs(30);
pub const PONG_TIMEOUT: Duration = Duration::from_secs(5);
pub const INPUT_POLL: Duration = Duration::from_millis(500);
const EXCLUDED_SCROLL_GAP: Duration = Duration::from_millis(300);
const VK_VOLUME_DOWN: u16 = 0xAE;
const VK_VOLUME_UP: u16 = 0xAF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownKind {
    Now,
    /// Scheduled shutdown the user can still cancel by touching the PC.
    InFiveMinutes,
    Abort,
}

/// Everything a session does to the PC, so tests can record it instead.
pub trait Effects {
    fn send_input(&self, events: &[InputEvent]);
    /// Normalized coordinates for a point on the TV's 1920x1080 grid, on `display` or the primary one.
    fn absolute(&self, display: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)>;
    /// The foreground app is one whose scroll wheel must step (volume in media players).
    fn scroll_excluded(&self) -> bool;
    /// Windows plays sound on something other than the TV (headphones, speakers).
    fn audio_is_local(&self) -> bool;
    fn pointer_shown(&self, shown: bool);
    fn run_command(&self, command: &str);
    fn shutdown(&self, kind: ShutdownKind);
    /// Milliseconds since the last keyboard or mouse input in this session.
    fn idle_ms(&self) -> Option<u32>;
}

#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// Ask the TV whether anyone is watching after this long without remote input.
    pub inactivity: Option<Duration>,
    /// Ask the same this long after the PC's monitor turned off.
    pub video_input: Option<Duration>,
    /// Display the TV's absolute pointer maps to (registry target id); `None` = primary.
    pub display: Option<u32>,
    /// Prefix for log lines.
    pub name: String,
}

/// Timeout settings in milliseconds -> the delay before asking the TV, which allows five
/// minutes for the scheduled shutdown itself. Same arithmetic as the C# service.
pub fn warning_delay(timeout_ms: u32) -> Duration {
    Duration::from_millis(timeout_ms.saturating_sub(300_000).max(1000) as u64)
}

pub struct Session<E: Effects> {
    fx: E,
    cfg: SessionConfig,
    /// Encoded frames waiting to be written.
    pub out: Vec<u8>,
    /// Set once the connection must end, with the reason.
    pub closed: Option<&'static str>,
    /// Remote keys held down, with the release sequence they were pressed with.
    held: HashMap<u16, Vec<InputEvent>>,
    pointer_shown: bool,
    ping_at: Option<Instant>,
    pong_by: Option<Instant>,
    inactivity_at: Option<Instant>,
    video_at: Option<Instant>,
    presence_pong_by: Option<Instant>,
    input_watch_at: Option<Instant>,
    last_excluded_scroll: Option<(Instant, i32)>,
}

impl<E: Effects> Session<E> {
    pub fn new(fx: E, cfg: SessionConfig, now: Instant) -> Self {
        let inactivity_at = cfg.inactivity.map(|d| now + d);
        Session {
            fx,
            cfg,
            out: Vec::new(),
            closed: None,
            held: HashMap::new(),
            pointer_shown: false,
            ping_at: Some(now + PING_EVERY),
            pong_by: None,
            inactivity_at,
            video_at: None,
            presence_pong_by: None,
            input_watch_at: None,
            last_excluded_scroll: None,
        }
    }

    pub fn effects(&self) -> &E {
        &self.fx
    }

    fn send(&mut self, opcode: u8, payload: &[u8]) {
        self.out.extend_from_slice(&ws::encode(opcode, payload));
    }

    fn close(&mut self, reason: &'static str) {
        if self.closed.is_none() {
            self.send(ws::OP_CLOSE, &[]);
            self.closed = Some(reason);
        }
    }

    /// The service is stopping: say goodbye to the TV.
    pub fn stop(&mut self) {
        self.close("service stopping");
    }

    pub fn on_frame(&mut self, fin: bool, opcode: u8, payload: &[u8], binds: &Compiled, now: Instant) {
        if !fin || opcode == ws::OP_CONTINUATION {
            log::warn(format!("{}: ignored a fragmented message", self.cfg.name));
            return;
        }
        match opcode {
            ws::OP_BINARY => {
                self.ping_at = Some(now + PING_EVERY);
                if let Some(d) = self.cfg.inactivity {
                    self.inactivity_at = Some(now + d);
                }
                if !payload.is_empty() {
                    self.on_message(payload, binds, now);
                }
            }
            ws::OP_TEXT => {
                if !payload.is_empty() {
                    let text = String::from_utf8_lossy(&payload[..payload.len().min(200)]).into_owned();
                    log::warn(format!("{}: unprocessed text message [{text}]", self.cfg.name));
                }
            }
            ws::OP_CLOSE => {
                // Echo the close, as RFC 6455 asks
                self.send(ws::OP_CLOSE, payload);
                self.closed = Some("TV disconnected");
            }
            ws::OP_PING => self.send(ws::OP_PONG, payload),
            ws::OP_PONG => match payload {
                [] => self.pong_by = None,
                [0x01] => {
                    // The TV says nobody is using it: schedule a shutdown the user can cancel by touching the PC
                    self.presence_pong_by = None;
                    self.fx.shutdown(ShutdownKind::InFiveMinutes);
                    self.input_watch_at = Some(now + INPUT_POLL);
                    log::info(format!("{}: TV reports no viewer, shutdown scheduled in 5 minutes", self.cfg.name));
                }
                other => log::warn(format!("{}: unprocessed pong {other:02X?}", self.cfg.name)),
            },
            other => log::warn(format!("{}: unprocessed opcode {other:#X}", self.cfg.name)),
        }
    }

    fn on_message(&mut self, payload: &[u8], binds: &Compiled, now: Instant) {
        let msg = match protocol::parse(payload) {
            Ok(m) => m,
            Err(protocol::ParseError::TooShort(_)) => return,
            Err(e) => {
                log::warn(format!("{}: unprocessed binary message {e:?} {:02X?}", self.cfg.name, &payload[..payload.len().min(16)]));
                return;
            }
        };
        match msg {
            ClientMessage::PositionRelative { dx, dy } => self.fx.send_input(&[InputEvent::MoveRelative { dx: dx as i32, dy: dy as i32 }]),
            ClientMessage::PositionAbsolute { x, y } => {
                if let Some((x, y)) = self.fx.absolute(self.cfg.display, x, y) {
                    self.fx.send_input(&[InputEvent::MoveAbsolute { x, y }]);
                }
            }
            ClientMessage::Wheel { delta } => self.on_wheel(delta, now),
            ClientMessage::Visible(shown) => {
                if shown != self.pointer_shown {
                    self.pointer_shown = shown;
                    self.fx.pointer_shown(shown);
                }
            }
            ClientMessage::Key { code, down: true } => self.on_key_down(code, binds),
            ClientMessage::Key { code, down: false } => {
                if let Some(up) = self.held.remove(&code) {
                    self.fx.send_input(&up);
                } else if let Some(Plan::Inputs { up, .. }) = binds.plans.get(&code) {
                    self.fx.send_input(up);
                }
            }
            ClientMessage::Unicode(unit) => self.fx.send_input(&[InputEvent::Unicode { unit, down: true }, InputEvent::Unicode { unit, down: false }]),
            ClientMessage::Shutdown => {
                log::info(format!("{}: shutdown requested from the TV", self.cfg.name));
                self.fx.shutdown(ShutdownKind::Now);
            }
            ClientMessage::Volume(step) => {
                // A TV volume step: when Windows plays on the TV it stays there,
                // otherwise it moves the PC volume and the TV is told to undo it.
                if step != 0 && self.fx.audio_is_local() {
                    let vk = if step > 0 { VK_VOLUME_UP } else { VK_VOLUME_DOWN };
                    for _ in 0..step.unsigned_abs() {
                        self.fx.send_input(&[
                            InputEvent::Key { vk, scan: 0, extended: true, down: true },
                            InputEvent::Key { vk, scan: 0, extended: true, down: false },
                        ]);
                    }
                    self.send(ws::OP_BINARY, &protocol::restore_volume_payload(step));
                }
            }
        }
    }

    fn on_wheel(&mut self, delta: i16, now: Instant) {
        if self.fx.scroll_excluded() {
            // One small step per gesture, so media players change volume by one notch
            let dir = (delta as i32).signum();
            let due = match self.last_excluded_scroll {
                Some((at, last_dir)) => now.duration_since(at) > EXCLUDED_SCROLL_GAP || dir != last_dir,
                None => true,
            };
            if dir != 0 && due {
                self.fx.send_input(&[InputEvent::Wheel { delta: -dir * 10 }]);
                self.last_excluded_scroll = Some((now, dir));
            }
        } else {
            self.fx.send_input(&[InputEvent::Wheel { delta: -(delta as i32) * 3 }]);
        }
    }

    fn on_key_down(&mut self, code: u16, binds: &Compiled) {
        match binds.plans.get(&code) {
            Some(Plan::Inputs { down, up }) => {
                self.fx.send_input(down);
                self.held.insert(code, up.clone());
            }
            Some(Plan::Actions(actions)) => {
                for a in actions {
                    self.send(ws::OP_BINARY, &protocol::action_payload(*a as u8));
                }
            }
            Some(Plan::Commands(commands)) => {
                for c in commands {
                    if !crate::bindings::command_is_safe(c) {
                        log::warn(format!("{}: blocked unsafe command binding: {c}", self.cfg.name));
                        break;
                    }
                    self.fx.run_command(c);
                }
            }
            None => log::debug(format!("{}: key {code:#06X} has no binding", self.cfg.name)),
        }
    }

    /// The PC's monitor turned on or off.
    pub fn on_monitor(&mut self, on: bool, now: Instant) {
        if let Some(d) = self.cfg.video_input {
            if on {
                self.video_at = None;
            } else if self.video_at.is_none() {
                self.video_at = Some(now + d);
            }
        }
    }

    fn ask_presence(&mut self, now: Instant) {
        self.send(ws::OP_PING, &protocol::PING_USER_INPUT);
        self.presence_pong_by.get_or_insert(now + PONG_TIMEOUT);
    }

    pub fn tick(&mut self, now: Instant) {
        let due = |t: Option<Instant>| t.is_some_and(|t| t <= now);
        if due(self.ping_at) {
            self.send(ws::OP_PING, &[]);
            self.pong_by.get_or_insert(now + PONG_TIMEOUT);
            self.ping_at = Some(now + PING_EVERY);
        }
        if due(self.pong_by) {
            log::warn(format!("{}: TV stopped answering pings", self.cfg.name));
            self.close("pong timeout");
        }
        if due(self.inactivity_at) {
            self.inactivity_at = None;
            log::info(format!("{}: no remote input for the inactivity timeout", self.cfg.name));
            self.ask_presence(now);
        }
        if due(self.video_at) {
            self.video_at = None;
            log::info(format!("{}: monitor off for the video input timeout", self.cfg.name));
            self.ask_presence(now);
        }
        if due(self.presence_pong_by) {
            log::warn(format!("{}: TV did not answer the presence check", self.cfg.name));
            self.close("presence pong timeout");
        }
        if due(self.input_watch_at) {
            if self.fx.idle_ms().is_some_and(|ms| ms < INPUT_POLL.as_millis() as u32) {
                self.fx.shutdown(ShutdownKind::Abort);
                self.input_watch_at = None;
                log::info(format!("{}: user input on the PC, shutdown cancelled", self.cfg.name));
            } else {
                self.input_watch_at = Some(now + INPUT_POLL);
            }
        }
    }

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        [self.ping_at, self.pong_by, self.inactivity_at, self.video_at, self.presence_pong_by, self.input_watch_at].into_iter().flatten().min()
    }

    /// The connection is gone: release what the remote left pressed and give the pointer back,
    /// so a dropped connection cannot leave Win, Ctrl or a mouse button stuck.
    pub fn finish(&mut self) {
        for (_, up) in self.held.drain() {
            self.fx.send_input(&up);
        }
        if self.pointer_shown {
            self.pointer_shown = false;
            self.fx.pointer_shown(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{defaults, Compiled, MouseButton};
    use std::cell::RefCell;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Input(Vec<InputEvent>),
        Pointer(bool),
        Command(String),
        Shutdown(ShutdownKind),
    }

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<Call>>,
        excluded: bool,
        local_audio: bool,
        idle: std::cell::Cell<u32>,
    }

    impl Effects for &Fake {
        fn send_input(&self, e: &[InputEvent]) {
            self.calls.borrow_mut().push(Call::Input(e.to_vec()));
        }
        fn absolute(&self, display: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
            assert_eq!(display, Some(7));
            Some((x as i32 * 2, y as i32 * 2))
        }
        fn scroll_excluded(&self) -> bool {
            self.excluded
        }
        fn audio_is_local(&self) -> bool {
            self.local_audio
        }
        fn pointer_shown(&self, shown: bool) {
            self.calls.borrow_mut().push(Call::Pointer(shown));
        }
        fn run_command(&self, c: &str) {
            self.calls.borrow_mut().push(Call::Command(c.into()));
        }
        fn shutdown(&self, k: ShutdownKind) {
            self.calls.borrow_mut().push(Call::Shutdown(k));
        }
        fn idle_ms(&self) -> Option<u32> {
            Some(self.idle.get())
        }
    }

    fn session(fake: &Fake, cfg: SessionConfig, now: Instant) -> Session<&Fake> {
        Session::new(fake, SessionConfig { display: Some(7), name: "test".into(), ..cfg }, now)
    }

    fn bin(s: &mut Session<&Fake>, payload: &[u8], binds: &Compiled, now: Instant) {
        s.on_frame(true, ws::OP_BINARY, payload, binds, now);
    }

    fn take(fake: &Fake) -> Vec<Call> {
        std::mem::take(&mut *fake.calls.borrow_mut())
    }

    #[test]
    fn pointer_wheel_and_text_input() {
        let (fake, t0, b) = (Fake::default(), Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        bin(&mut s, &[0x00, 0xFE, 0xFF, 0x03, 0x00], &b, t0);
        bin(&mut s, &[0x01, 0xC0, 0x03, 0x1C, 0x02], &b, t0);
        bin(&mut s, &[0x02, 0x02, 0x00], &b, t0);
        bin(&mut s, &[0x05, 0xE9, 0x00], &b, t0);
        assert_eq!(
            take(&fake),
            vec![
                Call::Input(vec![InputEvent::MoveRelative { dx: -2, dy: 3 }]),
                Call::Input(vec![InputEvent::MoveAbsolute { x: 1920, y: 1080 }]),
                Call::Input(vec![InputEvent::Wheel { delta: -6 }]),
                Call::Input(vec![InputEvent::Unicode { unit: 0xE9, down: true }, InputEvent::Unicode { unit: 0xE9, down: false }]),
            ]
        );
        assert!(s.out.is_empty());
    }

    #[test]
    fn excluded_apps_get_one_wheel_step_per_gesture() {
        let fake = Fake { excluded: true, ..Default::default() };
        let (t0, b) = (Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        bin(&mut s, &[0x02, 0x05, 0x00], &b, t0);
        bin(&mut s, &[0x02, 0x05, 0x00], &b, t0 + Duration::from_millis(100));
        bin(&mut s, &[0x02, 0xFB, 0xFF], &b, t0 + Duration::from_millis(150));
        bin(&mut s, &[0x02, 0xFB, 0xFF], &b, t0 + Duration::from_millis(500));
        bin(&mut s, &[0x02, 0x00, 0x00], &b, t0 + Duration::from_secs(5));
        let wheel = |d| Call::Input(vec![InputEvent::Wheel { delta: d }]);
        assert_eq!(take(&fake), vec![wheel(-10), wheel(10), wheel(10)]);
    }

    #[test]
    fn keys_press_and_release_and_dropped_connections_release_held_keys() {
        let (fake, t0, b) = (Fake::default(), Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        bin(&mut s, &[0x04, 0x21, 0x00, 0x01], &b, t0); // CH+ down -> Ctrl, C
        bin(&mut s, &[0x04, 0x21, 0x00, 0x00], &b, t0); // up -> C, Ctrl
        bin(&mut s, &[0x04, 0x01, 0x00, 0x01], &b, t0); // OK click down, never released
        let calls = take(&fake);
        assert_eq!(calls.len(), 3);
        let Call::Input(up) = &calls[1] else { panic!() };
        assert_eq!(up[0], InputEvent::Key { vk: 0x43, scan: 0x2E, extended: false, down: false });
        // Bindings reloaded while the button is held: the release still matches the press
        s.finish();
        assert_eq!(take(&fake), vec![Call::Input(vec![InputEvent::Button { button: MouseButton::Left, down: false }])]);
    }

    #[test]
    fn actions_go_to_the_tv_and_commands_run_unless_unsafe() {
        let (fake, t0) = (Fake::default(), Instant::now());
        let mut map = defaults();
        map.insert(0x0030, vec![crate::bindings::Bind::Command("calc.exe".into())]);
        map.insert(0x0031, vec![crate::bindings::Bind::Command("calc.exe & del x".into()), crate::bindings::Bind::Command("notepad".into())]);
        let b = Compiled::new(&map);
        let mut s = session(&fake, SessionConfig::default(), t0);
        bin(&mut s, &[0x04, 0x96, 0x01, 0x01], &b, t0); // Blue -> keyboard action
        bin(&mut s, &[0x04, 0x93, 0x01, 0x01], &b, t0); // Red -> shutdown dialog
        assert_eq!(s.out, [0x82, 0x01, 0x02, 0x82, 0x01, 0x01]);
        bin(&mut s, &[0x04, 0x30, 0x00, 0x01], &b, t0);
        bin(&mut s, &[0x04, 0x31, 0x00, 0x01], &b, t0);
        assert_eq!(take(&fake), vec![Call::Command("calc.exe".into())]);
    }

    #[test]
    fn volume_moves_the_pc_only_when_sound_is_local() {
        let (t0, b) = (Instant::now(), Compiled::new(&defaults()));
        let tv = Fake::default();
        let mut s = session(&tv, SessionConfig::default(), t0);
        bin(&mut s, &[0x07, 0x02], &b, t0);
        assert!(take(&tv).is_empty() && s.out.is_empty());

        let headphones = Fake { local_audio: true, ..Default::default() };
        let mut s = session(&headphones, SessionConfig::default(), t0);
        bin(&mut s, &[0x07, 0xFE], &b, t0);
        let calls = take(&headphones);
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            Call::Input(vec![
                InputEvent::Key { vk: VK_VOLUME_DOWN, scan: 0, extended: true, down: true },
                InputEvent::Key { vk: VK_VOLUME_DOWN, scan: 0, extended: true, down: false }
            ])
        );
        assert_eq!(s.out, [0x82, 0x02, 0x03, 0xFE]);
    }

    #[test]
    fn pointer_visibility_is_tracked_and_restored() {
        let (fake, t0, b) = (Fake::default(), Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        bin(&mut s, &[0x03, 0x01], &b, t0);
        bin(&mut s, &[0x03, 0x01], &b, t0);
        s.finish();
        assert_eq!(take(&fake), vec![Call::Pointer(true), Call::Pointer(false)]);
    }

    #[test]
    fn pings_every_30s_and_closes_when_the_tv_stops_answering() {
        let (fake, t0, b) = (Fake::default(), Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        assert_eq!(s.next_deadline(), Some(t0 + PING_EVERY));
        s.tick(t0 + Duration::from_secs(29));
        assert!(s.out.is_empty());
        // Remote input pushes the ping back
        bin(&mut s, &[0x00, 0, 0, 0, 0], &b, t0 + Duration::from_secs(20));
        s.tick(t0 + Duration::from_secs(30));
        assert!(s.out.is_empty());
        s.tick(t0 + Duration::from_secs(50));
        assert_eq!(std::mem::take(&mut s.out), [0x89, 0x00]);
        s.on_frame(true, ws::OP_PONG, &[], &b, t0 + Duration::from_secs(51));
        s.tick(t0 + Duration::from_secs(56));
        assert!(s.closed.is_none());
        s.tick(t0 + Duration::from_secs(80));
        s.tick(t0 + Duration::from_secs(85));
        assert_eq!(s.closed, Some("pong timeout"));
        assert_eq!(s.out, [0x89, 0x00, 0x88, 0x00]);
    }

    #[test]
    fn answers_pings_and_echoes_close() {
        let (fake, t0, b) = (Fake::default(), Instant::now(), Compiled::new(&defaults()));
        let mut s = session(&fake, SessionConfig::default(), t0);
        s.on_frame(true, ws::OP_PING, b"hi", &b, t0);
        assert_eq!(std::mem::take(&mut s.out), [0x8A, 0x02, b'h', b'i']);
        s.on_frame(false, ws::OP_BINARY, &[0x06], &b, t0);
        assert!(take(&fake).is_empty(), "fragments are ignored");
        s.on_frame(true, ws::OP_CLOSE, &[0x03, 0xE8], &b, t0);
        assert_eq!(s.out, [0x88, 0x02, 0x03, 0xE8]);
        assert_eq!(s.closed, Some("TV disconnected"));
    }

    #[test]
    fn inactivity_asks_the_tv_then_schedules_and_cancels_shutdown() {
        let fake = Fake::default();
        fake.idle.set(10_000);
        let (t0, b) = (Instant::now(), Compiled::new(&defaults()));
        let cfg = SessionConfig { inactivity: Some(warning_delay(7_200_000)), ..Default::default() };
        let mut s = session(&fake, cfg, t0);
        let ask = t0 + Duration::from_millis(6_900_000);
        s.ping_at = None; // keep the keep-alive out of this test
        s.tick(ask - Duration::from_millis(1));
        assert!(s.out.is_empty());
        s.tick(ask);
        assert_eq!(std::mem::take(&mut s.out), [0x89, 0x01, 0x01]);
        s.on_frame(true, ws::OP_PONG, &[0x01], &b, ask + Duration::from_secs(1));
        assert_eq!(take(&fake), vec![Call::Shutdown(ShutdownKind::InFiveMinutes)]);
        s.tick(ask + Duration::from_millis(1500));
        assert!(take(&fake).is_empty(), "still idle");
        fake.idle.set(100);
        s.tick(ask + Duration::from_millis(2000));
        assert_eq!(take(&fake), vec![Call::Shutdown(ShutdownKind::Abort)]);
        s.tick(ask + Duration::from_secs(60));
        assert!(take(&fake).is_empty() && s.closed.is_none());
    }

    #[test]
    fn presence_check_without_answer_closes() {
        let (fake, t0) = (Fake::default(), Instant::now());
        let cfg = SessionConfig { video_input: Some(warning_delay(900_000)), ..Default::default() };
        let mut s = session(&fake, cfg, t0);
        s.ping_at = None;
        s.on_monitor(false, t0);
        s.on_monitor(true, t0 + Duration::from_secs(10));
        s.tick(t0 + Duration::from_secs(700));
        assert!(s.out.is_empty(), "monitor came back on");
        s.on_monitor(false, t0 + Duration::from_secs(1000));
        s.tick(t0 + Duration::from_secs(1600));
        assert_eq!(std::mem::take(&mut s.out), [0x89, 0x01, 0x01]);
        s.tick(t0 + Duration::from_secs(1605));
        assert_eq!(s.closed, Some("presence pong timeout"));
    }

    #[test]
    fn warning_delay_matches_the_c_sharp_arithmetic() {
        assert_eq!(warning_delay(7_200_000), Duration::from_millis(6_900_000));
        assert_eq!(warning_delay(300_500), Duration::from_millis(1000));
        assert_eq!(warning_delay(0), Duration::from_millis(1000));
    }
}
