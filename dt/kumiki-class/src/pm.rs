//! Power management building blocks (doc/13).
//!
//! * [`DevicePm`]: suspend / resume one device. Implemented by every device
//!   class a board hands out that has something to power down; there is no
//!   default implementation, so a device cannot silently stay powered when
//!   "suspended". Devices that cannot suspend yet say so with an error.
//! * [`BootInfo`]: why the chip started (power-on, reset, deep-sleep wake).
//!   A deep-sleep wake goes through the whole boot again, so drivers and
//!   applications use this to tell a cold boot from a warm one.
//! * [`RailCount`] + [`Rail`]: a regulator shared by several consumers
//!   (devices owned separately) is switched on while at least one of them
//!   holds it, like Linux's regulator use counts.

use core::sync::atomic::{AtomicU32, Ordering};

/// What [`DevicePm::pm`] should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PmAction {
    /// Enter the device's lowest-power state that keeps its configuration
    /// (or that [`Resume`](PmAction::Resume) can restore).
    Suspend,
    /// Back to normal operation.
    Resume,
}

/// Suspend / resume of one device (Zephyr's `pm_device_action_cb_t`).
///
/// The owner of the device calls it; suspend in the reverse of the board's
/// bring-up order and resume in that order (generated as `SUSPEND_ORDER` /
/// `INIT_ORDER`). Calling the device's other methods while it is suspended
/// is the class's business (documented per implementation).
pub trait DevicePm {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Suspend or resume. Suspending a suspended device, or resuming a
    /// running one, does nothing.
    fn pm(&mut self, action: PmAction) -> Result<(), Self::Error>;
}

/// Why the chip last reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetReason {
    /// Power was applied (or brown-out recovery that reset everything).
    PowerOn,
    /// Software reset (restart, debugger, USB-JTAG / USB-UART reset).
    Software,
    /// Woke from deep sleep (the chip boots again; see [`WakeCause`]).
    DeepSleep,
    /// A watchdog fired.
    Watchdog,
    /// The supply dipped below the brown-out threshold.
    BrownOut,
    /// Anything else, with the chip's raw reason code.
    Other(u8),
}

/// What ended a sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeCause {
    /// The chip did not come out of a sleep.
    NotFromSleep,
    /// The sleep timer.
    Timer,
    /// A GPIO (RTC / LP IO in deep sleep).
    Gpio,
    /// A UART.
    Uart,
    /// The touch sensor.
    Touch,
    /// Woke from sleep, cause not decoded on this chip yet.
    Unknown,
}

/// How the chip started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootInfo {
    /// Reset reason.
    pub reset: ResetReason,
    /// Sleep wake-up cause.
    pub wake: WakeCause,
}

impl BootInfo {
    /// Woke from deep sleep: RAM is gone, but off-chip devices kept their
    /// state (and power, unless the board cut it) and RTC / LP memory holds
    /// what was put there.
    pub fn is_warm(&self) -> bool {
        self.reset == ResetReason::DeepSleep
    }
}

/// Switches one regulator (an LDO of a PMIC, an expander line, …).
pub trait RegulatorSwitch {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Turn the output on or off.
    fn set_enabled(&mut self, on: bool) -> Result<(), Self::Error>;
}

/// The use count of one regulator, shared by its [`Rail`] handles (put it in
/// a `static`; the generator emits one per DT regulator node).
///
/// Like the shared I²C buses, rails are meant for one core: two handles of
/// the same rail must not be switched from different cores at once.
#[derive(Debug)]
pub struct RailCount {
    users: AtomicU32,
    always_on: bool,
}

impl RailCount {
    /// A rail nobody holds yet. `always_on` (DT `regulator-always-on`): the
    /// rail is never switched off.
    pub const fn new(always_on: bool) -> Self {
        Self { users: AtomicU32::new(0), always_on }
    }

    /// Current number of holders.
    pub fn users(&self) -> u32 {
        self.users.load(Ordering::Relaxed)
    }

    /// Whether the rail is never switched off.
    pub fn always_on(&self) -> bool {
        self.always_on
    }
}

/// One consumer's handle on a shared regulator. Each handle counts at most
/// once: enabling twice is the same as once.
///
/// Dropping a held handle does not switch the rail off (that needs bus
/// access, which can fail); call [`disable`](Self::disable) first.
pub struct Rail<S> {
    count: &'static RailCount,
    switch: S,
    held: bool,
}

impl<S: RegulatorSwitch> Rail<S> {
    /// A handle on `count`, switching through `switch`.
    pub fn new(count: &'static RailCount, switch: S) -> Self {
        Self { count, switch, held: false }
    }

    /// Hold the rail; the first holder switches it on. On error the handle
    /// does not hold it.
    pub fn enable(&mut self) -> Result<(), S::Error> {
        if self.held {
            return Ok(());
        }
        if self.count.users.fetch_add(1, Ordering::AcqRel) == 0
            && let Err(e) = self.switch.set_enabled(true)
        {
            self.count.users.fetch_sub(1, Ordering::AcqRel);
            return Err(e);
        }
        self.held = true;
        Ok(())
    }

    /// Release the rail; the last holder switches it off (unless it is
    /// always on). On error the handle no longer holds it, but the rail may
    /// still be on.
    pub fn disable(&mut self) -> Result<(), S::Error> {
        if !self.held {
            return Ok(());
        }
        self.held = false;
        if self.count.users.fetch_sub(1, Ordering::AcqRel) == 1 && !self.count.always_on {
            self.switch.set_enabled(false)?;
        }
        Ok(())
    }

    /// Whether this handle holds the rail.
    pub fn is_enabled(&self) -> bool {
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    #[derive(Clone, Default)]
    struct Log(Rc<RefCell<Vec<bool>>>, bool);
    impl RegulatorSwitch for Log {
        type Error = ();
        fn set_enabled(&mut self, on: bool) -> Result<(), ()> {
            if self.1 {
                return Err(());
            }
            self.0.borrow_mut().push(on);
            Ok(())
        }
    }

    #[test]
    fn first_on_last_off() {
        static C: RailCount = RailCount::new(false);
        let log = Log::default();
        let (mut a, mut b) = (Rail::new(&C, log.clone()), Rail::new(&C, log.clone()));
        a.enable().unwrap();
        a.enable().unwrap(); // counts once
        b.enable().unwrap();
        assert_eq!(C.users(), 2);
        a.disable().unwrap();
        a.disable().unwrap();
        assert_eq!(*log.0.borrow(), [true]);
        b.disable().unwrap();
        assert_eq!(*log.0.borrow(), [true, false]);
        assert_eq!(C.users(), 0);
    }

    #[test]
    fn always_on_is_never_switched_off() {
        static C: RailCount = RailCount::new(true);
        let log = Log::default();
        let mut a = Rail::new(&C, log.clone());
        a.enable().unwrap();
        a.disable().unwrap();
        assert_eq!(*log.0.borrow(), [true]);
    }

    #[test]
    fn failed_enable_does_not_hold() {
        static C: RailCount = RailCount::new(false);
        let mut a = Rail::new(&C, Log(Default::default(), true));
        assert!(a.enable().is_err());
        assert!(!a.is_enabled());
        assert_eq!(C.users(), 0);
    }

    #[test]
    fn warm_boot() {
        let b = BootInfo { reset: ResetReason::DeepSleep, wake: WakeCause::Timer };
        assert!(b.is_warm());
        assert!(!BootInfo { reset: ResetReason::PowerOn, wake: WakeCause::NotFromSleep }.is_warm());
    }
}
