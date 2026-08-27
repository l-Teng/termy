use std::borrow::Cow;

use super::{KittyClipboardOsc, KittyClipboardOscTerminator};

const MAX_OSC_BODY_BYTES: usize = 64 * 1024;
const MAX_CONTROL_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyClipboardControl {
    Set(bool),
    Query,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyClipboardInput {
    Packet(KittyClipboardOsc),
    Control(KittyClipboardControl),
}

#[derive(Debug, Default)]
pub struct KittyClipboardInterceptor {
    pending: Vec<u8>,
    osc_body: Option<Vec<u8>>,
    osc_escape: bool,
    osc_overflowed: bool,
}

impl KittyClipboardInterceptor {
    pub fn process<'a>(&mut self, input: &'a [u8]) -> (Cow<'a, [u8]>, Vec<KittyClipboardInput>) {
        if self.pending.is_empty()
            && self.osc_body.is_none()
            && !input.iter().any(|byte| matches!(byte, 0x1b | 0x9b | 0x9d))
        {
            return (Cow::Borrowed(input), Vec::new());
        }

        let mut output = Vec::with_capacity(input.len());
        let mut events = Vec::new();
        for &byte in input {
            self.process_byte(byte, &mut output, &mut events);
        }
        (Cow::Owned(output), events)
    }

    fn process_byte(
        &mut self,
        byte: u8,
        output: &mut Vec<u8>,
        events: &mut Vec<KittyClipboardInput>,
    ) {
        if self.osc_body.is_some() {
            self.process_osc_byte(byte, events);
            return;
        }

        if self.pending.is_empty() {
            if matches!(byte, 0x1b | 0x9b | 0x9d) {
                self.pending.push(byte);
            } else {
                output.push(byte);
            }
            return;
        }

        self.pending.push(byte);
        if is_osc_start(&self.pending) {
            self.pending.clear();
            self.osc_body = Some(Vec::new());
            self.osc_escape = false;
            self.osc_overflowed = false;
            return;
        }
        if let Some((event, pass_through)) = exact_control_match(&self.pending) {
            if pass_through {
                output.extend_from_slice(&self.pending);
            }
            events.push(KittyClipboardInput::Control(event));
            self.pending.clear();
            return;
        }
        if is_prefix(&self.pending) {
            return;
        }

        let rejected = std::mem::take(&mut self.pending);
        output.push(rejected[0]);
        for byte in rejected.into_iter().skip(1) {
            self.process_byte(byte, output, events);
        }
    }

    fn process_osc_byte(&mut self, byte: u8, events: &mut Vec<KittyClipboardInput>) {
        if self.osc_escape {
            self.osc_escape = false;
            if byte == b'\\' {
                self.finish_osc(KittyClipboardOscTerminator::StringTerminator, events);
                return;
            }
            self.push_osc_body(0x1b);
            self.push_osc_body(byte);
            return;
        }

        match byte {
            0x07 => self.finish_osc(KittyClipboardOscTerminator::Bell, events),
            0x9c => self.finish_osc(KittyClipboardOscTerminator::StringTerminator, events),
            0x1b => self.osc_escape = true,
            _ => self.push_osc_body(byte),
        }
    }

    fn push_osc_body(&mut self, byte: u8) {
        if self.osc_overflowed {
            return;
        }
        let body = self.osc_body.as_mut().expect("OSC capture is active");
        if body.len() < MAX_OSC_BODY_BYTES {
            body.push(byte);
        } else {
            body.clear();
            self.osc_overflowed = true;
        }
    }

    fn finish_osc(
        &mut self,
        terminator: KittyClipboardOscTerminator,
        events: &mut Vec<KittyClipboardInput>,
    ) {
        let body = self.osc_body.take().expect("OSC capture is active");
        if !self.osc_overflowed {
            events.push(KittyClipboardInput::Packet(KittyClipboardOsc::from_body(
                &body, terminator,
            )));
        }
        self.osc_escape = false;
        self.osc_overflowed = false;
    }
}

const CONTROL_PATTERNS: &[(&[u8], KittyClipboardControl, bool)] = &[
    (b"\x1b[?5522$p", KittyClipboardControl::Query, false),
    (b"\x9b?5522$p", KittyClipboardControl::Query, false),
    (b"\x1bc", KittyClipboardControl::Reset, true),
];

const OSC_START_PATTERNS: &[&[u8]] = &[b"\x1b]5522;", b"\x9d5522;"];

fn exact_control_match(bytes: &[u8]) -> Option<(KittyClipboardControl, bool)> {
    if let Some(result) = private_mode_set(bytes) {
        return Some(result);
    }
    CONTROL_PATTERNS
        .iter()
        .find(|(pattern, _, _)| *pattern == bytes)
        .map(|(_, event, pass_through)| (*event, *pass_through))
}

fn is_osc_start(bytes: &[u8]) -> bool {
    OSC_START_PATTERNS.contains(&bytes)
}

fn is_prefix(bytes: &[u8]) -> bool {
    CONTROL_PATTERNS
        .iter()
        .any(|(pattern, _, _)| pattern.starts_with(bytes))
        || OSC_START_PATTERNS
            .iter()
            .any(|pattern| pattern.starts_with(bytes))
        || private_mode_prefix(bytes)
}

fn private_mode_set(bytes: &[u8]) -> Option<(KittyClipboardControl, bool)> {
    let parameters = bytes
        .strip_prefix(b"\x1b[?")
        .or_else(|| bytes.strip_prefix(b"\x9b?"))?;
    let (&final_byte, parameters) = parameters.split_last()?;
    if !matches!(final_byte, b'h' | b'l')
        || !parameters
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
    {
        return None;
    }
    let mut found = false;
    let mut pass_through = false;
    for parameter in parameters.split(|byte| *byte == b';') {
        if parameter == b"5522" {
            found = true;
        } else {
            pass_through = true;
        }
    }
    found.then_some((KittyClipboardControl::Set(final_byte == b'h'), pass_through))
}

fn private_mode_prefix(bytes: &[u8]) -> bool {
    if bytes.len() > MAX_CONTROL_BYTES {
        return false;
    }
    bytes
        .strip_prefix(b"\x1b[?")
        .or_else(|| bytes.strip_prefix(b"\x9b?"))
        .is_some_and(|parameters| {
            parameters
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b';')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_mode_controls_across_chunks() {
        let mut interceptor = KittyClipboardInterceptor::default();
        let (first, events) = interceptor.process(b"before\x1b[?55");
        assert_eq!(first.as_ref(), b"before");
        assert!(events.is_empty());

        let (second, events) = interceptor.process(b"22hafter");
        assert_eq!(second.as_ref(), b"after");
        assert_eq!(
            events,
            vec![KittyClipboardInput::Control(KittyClipboardControl::Set(
                true
            ))]
        );
    }

    #[test]
    fn recognizes_kitty_mode_in_multi_parameter_decset() {
        let mut interceptor = KittyClipboardInterceptor::default();
        let (output, events) = interceptor.process(b"\x1b[?2004;5522h");

        assert_eq!(output.as_ref(), b"\x1b[?2004;5522h");
        assert_eq!(
            events,
            vec![KittyClipboardInput::Control(KittyClipboardControl::Set(
                true
            ))]
        );
    }

    #[test]
    fn preserves_unrelated_control_sequences_and_ris() {
        let mut interceptor = KittyClipboardInterceptor::default();
        let (output, events) = interceptor.process(b"\x1b[31mred\x1bc");
        assert_eq!(output.as_ref(), b"\x1b[31mred\x1bc");
        assert_eq!(
            events,
            vec![KittyClipboardInput::Control(KittyClipboardControl::Reset)]
        );
    }

    #[test]
    fn recognizes_c1_queries_and_osc_packets() {
        let mut interceptor = KittyClipboardInterceptor::default();
        let (output, events) = interceptor.process(b"\x9b?5522$p\x9d5522;type=read:id=c1;Lg==\x9c");
        assert!(output.is_empty());
        assert!(matches!(
            events.as_slice(),
            [
                KittyClipboardInput::Control(KittyClipboardControl::Query),
                KittyClipboardInput::Packet(packet),
            ] if packet == &KittyClipboardOsc::from_body(
                b"type=read:id=c1;Lg==",
                KittyClipboardOscTerminator::StringTerminator,
            )
        ));
    }

    #[test]
    fn preserves_packet_and_control_wire_order_across_chunk_splits() {
        let input = b"\x1b[?5522h\x1b]5522;type=read:id=one;Lg==\x1b\\\x1b[?5522l";
        let mut one_shot = KittyClipboardInterceptor::default();
        let (expected_output, expected_events) = one_shot.process(input);

        for split in 0..=input.len() {
            let mut chunked = KittyClipboardInterceptor::default();
            let (first_output, mut events) = chunked.process(&input[..split]);
            let mut output = first_output.into_owned();
            let (second_output, second_events) = chunked.process(&input[split..]);
            output.extend_from_slice(&second_output);
            events.extend(second_events);

            assert_eq!(output, expected_output.as_ref(), "split at {split}");
            assert_eq!(events, expected_events, "split at {split}");
        }
    }
}
