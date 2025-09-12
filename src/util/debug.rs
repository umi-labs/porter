use colored::Colorize;

static mut DEBUG_ENABLED: bool = false;

/// Initialize debug printing based on a flag
pub fn init(enabled: bool) {
    unsafe { DEBUG_ENABLED = enabled; }
}

/// Print a debug line if enabled
pub fn log<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            println!("{} {}", "[DEBUG]:".red().bold(), message.as_ref());
        }
    }
}

/// Print a formatted debug line if enabled
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {{
        $crate::util::debug::log(format!($($arg)*));
    }};
}


