//! The console behind [`println!`](crate::println) and the HAL's bring-up
//! log lines. Where it goes is the program's choice: USB Serial/JTAG by
//! default (feature `usj-console`), anything else through [`set_sink`] (an
//! OS kernel that uses USB Serial/JTAG for its own protocol, a UART, a log
//! ring, …).

use core::sync::atomic::{AtomicPtr, Ordering};

/// A console output function: writes all of `bytes` or drops them.
pub type Sink = fn(&[u8]);

fn discard(_: &[u8]) {}

#[cfg(feature = "usj-console")]
const DEFAULT: Sink = crate::usj::write;
#[cfg(not(feature = "usj-console"))]
const DEFAULT: Sink = discard;

static SINK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Sends the console to `sink` from now on.
pub fn set_sink(sink: Sink) {
    SINK.store(sink as *mut (), Ordering::Release);
}

/// Drops all console output from now on.
pub fn silence() {
    set_sink(discard);
}

/// Writes `bytes` to the console.
pub fn write(bytes: &[u8]) {
    let p = SINK.load(Ordering::Acquire);
    let sink: Sink = if p.is_null() {
        DEFAULT
    } else {
        // SAFETY: only `set_sink` stores into SINK, always a `Sink`.
        unsafe { core::mem::transmute::<*mut (), Sink>(p) }
    };
    sink(bytes);
}

/// `core::fmt::Write` for the console.
pub struct Console;

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write(s.as_bytes());
        Ok(())
    }
}

/// Prints to the console (see [`console`](crate::console)).
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::console::Console, format_args!($($arg)*));
    }};
}

/// Prints a line to the console (see [`console`](crate::console)).
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\r\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\r\n");
    }};
}
