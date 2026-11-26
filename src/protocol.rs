//! Binary messages between the TV app (`main.js`) and the PC. Integers are little-endian.

/// A message the TV sends in a binary WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessage {
    /// 0x00: move the pointer by a delta.
    PositionRelative { dx: i16, dy: i16 },
    /// 0x01: move the pointer to a point on a 1920x1080 grid.
    PositionAbsolute { x: u16, y: u16 },
    /// 0x02: scroll wheel delta as the TV reports it.
    Wheel { delta: i16 },
    /// 0x03: the TV shows (true) or hides (false) its pointer.
    Visible(bool),
    /// 0x04: a remote key went down or up.
    Key { code: u16, down: bool },
    /// 0x05: one UTF-16 code unit typed on the TV keyboard.
    Unicode(u16),
    /// 0x06: the user confirmed the shutdown dialog.
    Shutdown,
    /// 0x07: the TV volume moved by this many steps.
    Volume(i8),
    /// 0x08: sent by TV apps that understand more than the original protocol, right after
    /// connecting. Bit 0x01: it applies live settings ([`settings_payload`]).
    Hello { capabilities: u8 },
}

/// `Hello` capability: the TV app applies settings the PC sends.
pub const CAN_LIVE_SETTINGS: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    TooShort(u8),
    Unknown(u8),
}

fn i16_at(p: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([p[i], p[i + 1]])
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

/// Parses one binary frame payload.
pub fn parse(p: &[u8]) -> Result<ClientMessage, ParseError> {
    let kind = *p.first().ok_or(ParseError::Empty)?;
    let need = match kind {
        0x00 | 0x01 => 5,
        0x02 | 0x05 => 3,
        0x03 | 0x07 | 0x08 => 2,
        0x04 => 4,
        0x06 => 1,
        other => return Err(ParseError::Unknown(other)),
    };
    if p.len() < need {
        return Err(ParseError::TooShort(kind));
    }
    Ok(match kind {
        0x00 => ClientMessage::PositionRelative { dx: i16_at(p, 1), dy: i16_at(p, 3) },
        0x01 => ClientMessage::PositionAbsolute { x: u16_at(p, 1), y: u16_at(p, 3) },
        0x02 => ClientMessage::Wheel { delta: i16_at(p, 1) },
        0x03 => ClientMessage::Visible(p[1] != 0),
        0x04 => ClientMessage::Key { code: u16_at(p, 1), down: p[3] & 0x01 == 0x01 },
        0x05 => ClientMessage::Unicode(u16_at(p, 1)),
        0x06 => ClientMessage::Shutdown,
        0x07 => ClientMessage::Volume(p[1] as i8),
        _ => ClientMessage::Hello { capabilities: p[1] },
    })
}

/// Payload that asks the TV to run an action (0x01 shutdown dialog, 0x02 keyboard).
pub fn action_payload(action: u8) -> [u8; 1] {
    [action]
}

/// Payload that tells the TV to undo a volume step the PC took over.
pub fn restore_volume_payload(step: i8) -> [u8; 2] {
    [0x03, step as u8]
}

/// Payload carrying live TV settings as JSON (0x04), only for TV apps that said they apply them:
/// older apps show an error for message types they don't know.
pub fn settings_payload(json: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(json.len() + 1);
    p.push(0x04);
    p.extend_from_slice(json.as_bytes());
    p
}

/// Ping payload asking the TV whether someone is still watching; it answers with pong `01`.
pub const PING_USER_INPUT: [u8; 1] = [0x01];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_message_main_js_sends() {
        assert_eq!(parse(&[0x00, 0xFE, 0xFF, 0x03, 0x00]), Ok(ClientMessage::PositionRelative { dx: -2, dy: 3 }));
        assert_eq!(parse(&[0x01, 0xC0, 0x03, 0x1C, 0x02]), Ok(ClientMessage::PositionAbsolute { x: 960, y: 540 }));
        assert_eq!(parse(&[0x02, 0x9C, 0xFF]), Ok(ClientMessage::Wheel { delta: -100 }));
        assert_eq!(parse(&[0x03, 0x01]), Ok(ClientMessage::Visible(true)));
        assert_eq!(parse(&[0x03, 0x00]), Ok(ClientMessage::Visible(false)));
        assert_eq!(parse(&[0x04, 0x0D, 0x00, 0x01]), Ok(ClientMessage::Key { code: 0x0D, down: true }));
        assert_eq!(parse(&[0x04, 0xCD, 0x01, 0x00]), Ok(ClientMessage::Key { code: 0x01CD, down: false }));
        assert_eq!(parse(&[0x05, 0xE9, 0x00]), Ok(ClientMessage::Unicode(0x00E9)));
        assert_eq!(parse(&[0x06]), Ok(ClientMessage::Shutdown));
        assert_eq!(parse(&[0x07, 0xFF]), Ok(ClientMessage::Volume(-1)));
        assert_eq!(parse(&[0x08, 0x01]), Ok(ClientMessage::Hello { capabilities: CAN_LIVE_SETTINGS }));
    }

    #[test]
    fn rejects_short_empty_and_unknown_payloads() {
        assert_eq!(parse(&[]), Err(ParseError::Empty));
        assert_eq!(parse(&[0x00, 1, 2, 3]), Err(ParseError::TooShort(0x00)));
        assert_eq!(parse(&[0x04, 1, 2]), Err(ParseError::TooShort(0x04)));
        assert_eq!(parse(&[0x07]), Err(ParseError::TooShort(0x07)));
        assert_eq!(parse(&[0x08]), Err(ParseError::TooShort(0x08)));
        assert_eq!(parse(&[0x09]), Err(ParseError::Unknown(0x09)));
    }

    #[test]
    fn server_payloads_match_what_main_js_reads() {
        assert_eq!(action_payload(0x02), [0x02]);
        assert_eq!(restore_volume_payload(-3), [0x03, 0xFD]);
        assert_eq!(settings_payload("{}"), b"\x04{}");
    }
}
