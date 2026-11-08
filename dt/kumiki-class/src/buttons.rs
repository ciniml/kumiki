//! Button model: raw state bits from any source → debounced events.
//!
//! Like M5Unified's `btn_rawstate_bits`, every input source (GPIO, PMIC power
//! key, IO-expander pin, touch zone) only reports a raw pressed/released bit;
//! [`Buttons`] turns the bitmask into debounced press / release / click /
//! long-press events. Time is passed in by the caller (milliseconds from any
//! monotonic clock), so no time driver is required.

/// Logical button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonId {
    /// Main button (e.g. AtomS3 screen button, CoreS3 touch tap).
    A = 0,
    /// Secondary button.
    B = 1,
    /// Third button.
    C = 2,
    /// Power key.
    Power = 3,
}

impl ButtonId {
    const ALL: [ButtonId; 4] = [ButtonId::A, ButtonId::B, ButtonId::C, ButtonId::Power];
}

/// Kind of event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Became pressed (after debouncing).
    Pressed,
    /// Became released.
    Released,
    /// Released before the long-press time.
    Click,
    /// Held for at least the long-press time (emitted once while held).
    LongPress,
}

/// A button event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonEvent {
    /// Which button.
    pub id: ButtonId,
    /// What happened.
    pub kind: ButtonKind,
}

#[derive(Clone, Copy, Default)]
struct State {
    stable: bool,
    candidate: bool,
    since: u64,
    pressed_at: u64,
    long_sent: bool,
}

/// Debouncer and click / long-press detector for up to four buttons.
pub struct Buttons {
    st: [State; 4],
    debounce_ms: u64,
    long_ms: u64,
}

impl Default for Buttons {
    fn default() -> Self {
        Self::new(10, 600)
    }
}

impl Buttons {
    /// `debounce_ms`: time a raw level must be stable; `long_ms`: long-press threshold.
    pub const fn new(debounce_ms: u64, long_ms: u64) -> Self {
        Self { st: [State { stable: false, candidate: false, since: 0, pressed_at: 0, long_sent: false }; 4], debounce_ms, long_ms }
    }

    /// Whether `id` is currently (debounced) pressed.
    pub fn is_pressed(&self, id: ButtonId) -> bool {
        self.st[id as usize].stable
    }

    /// Feed raw state bits (bit n = [`ButtonId`] n pressed) sampled at `now_ms`.
    /// Calls `emit` for each resulting event.
    pub fn update(&mut self, raw: u8, now_ms: u64, mut emit: impl FnMut(ButtonEvent)) {
        for id in ButtonId::ALL {
            let s = &mut self.st[id as usize];
            let level = raw & (1 << id as u8) != 0;
            if level != s.candidate {
                s.candidate = level;
                s.since = now_ms;
            }
            if s.candidate != s.stable && now_ms.saturating_sub(s.since) >= self.debounce_ms {
                s.stable = s.candidate;
                if s.stable {
                    s.pressed_at = now_ms;
                    s.long_sent = false;
                    emit(ButtonEvent { id, kind: ButtonKind::Pressed });
                } else {
                    emit(ButtonEvent { id, kind: ButtonKind::Released });
                    if !s.long_sent {
                        emit(ButtonEvent { id, kind: ButtonKind::Click });
                    }
                }
            }
            if s.stable && !s.long_sent && now_ms.saturating_sub(s.pressed_at) >= self.long_ms {
                s.long_sent = true;
                emit(ButtonEvent { id, kind: ButtonKind::LongPress });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(b: &mut Buttons, raw: u8, t: u64) -> Vec<ButtonEvent> {
        let mut v = Vec::new();
        b.update(raw, t, |e| v.push(e));
        v
    }

    #[test]
    fn click_after_debounce() {
        let mut b = Buttons::new(10, 600);
        assert!(run(&mut b, 1, 0).is_empty());
        assert!(run(&mut b, 0, 5).is_empty()); // bounce
        assert!(run(&mut b, 1, 6).is_empty());
        assert_eq!(run(&mut b, 1, 16), vec![ButtonEvent { id: ButtonId::A, kind: ButtonKind::Pressed }]);
        run(&mut b, 0, 100);
        let ev = run(&mut b, 0, 110);
        assert_eq!(ev[1], ButtonEvent { id: ButtonId::A, kind: ButtonKind::Click });
    }

    #[test]
    fn long_press_suppresses_click() {
        let mut b = Buttons::new(0, 100);
        run(&mut b, 2, 0);
        assert_eq!(run(&mut b, 2, 100), vec![ButtonEvent { id: ButtonId::B, kind: ButtonKind::LongPress }]);
        let ev = run(&mut b, 0, 200);
        assert_eq!(ev, vec![ButtonEvent { id: ButtonId::B, kind: ButtonKind::Released }]);
    }
}
