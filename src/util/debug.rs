use colored::Colorize;
use std::sync::atomic::{AtomicUsize, Ordering};

static mut DEBUG_ENABLED: bool = false;
static INDENT_LEVEL: AtomicUsize = AtomicUsize::new(0);

/// Initialize debug printing based on a flag
pub fn init(enabled: bool) {
    unsafe { DEBUG_ENABLED = enabled; }
}

/// Increase indentation level for nested debug messages
pub fn indent() {
    INDENT_LEVEL.fetch_add(1, Ordering::Relaxed);
}

/// Decrease indentation level
pub fn dedent() {
    let current = INDENT_LEVEL.load(Ordering::Relaxed);
    if current > 0 {
        INDENT_LEVEL.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Reset indentation to zero
pub fn reset_indent() {
    INDENT_LEVEL.store(0, Ordering::Relaxed);
}

/// Get current indentation level
fn get_indent_level() -> usize {
    INDENT_LEVEL.load(Ordering::Relaxed)
}

/// Generate indentation string
fn get_indent_string() -> String {
    let level = get_indent_level();
    "  ".repeat(level)
}

/// Print a debug line with enhanced formatting
pub fn log<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}🔍", indent).cyan().bold();
            let content = message.as_ref();
            
            // Split content into lines and format each
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.cyan());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.cyan(), line.cyan());
                }
            }
        }
    }
}

/// Print a debug line with info-level styling (lighter blue)
pub fn log_info<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}ℹ️ ", indent).blue().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.blue());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.blue(), line.blue());
                }
            }
        }
    }
}

/// Print a debug line with success styling (green)
pub fn log_success<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}✅", indent).green().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.green());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.green(), line.green());
                }
            }
        }
    }
}

/// Print a debug line with warning styling (yellow)
pub fn log_warning<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}⚠️ ", indent).yellow().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.yellow());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.yellow(), line.yellow());
                }
            }
        }
    }
}

/// Print a debug line with error styling (red)
pub fn log_error<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}❌", indent).red().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.red());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.red(), line.red());
                }
            }
        }
    }
}

/// Print a debug line with step styling (purple)
pub fn log_step<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}▶️ ", indent).purple().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.purple());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.purple(), line.purple());
                }
            }
        }
    }
}

/// Print a debug line with data styling (gray)
pub fn log_data<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}📊", indent).bright_black().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.bright_black());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.bright_black(), line.bright_black());
                }
            }
        }
    }
}

/// Print a debug line with file styling (magenta)
pub fn log_file<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}📁", indent).magenta().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.magenta());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.magenta(), line.magenta());
                }
            }
        }
    }
}

/// Print a debug line with processing styling (bright cyan)
pub fn log_processing<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}⚙️ ", indent).bright_cyan().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.bright_cyan());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.bright_cyan(), line.bright_cyan());
                }
            }
        }
    }
}

/// Print a debug line with result styling (bright green)
pub fn log_result<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}🎯", indent).bright_green().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line.bright_green());
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent.bright_green(), line.bright_green());
                }
            }
        }
    }
}

/// Print a debug line with separator styling
pub fn log_separator() {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let separator = "─".repeat(50);
            println!("{}{}", indent.cyan(), separator.cyan());
        }
    }
}

/// Print a debug line with section header styling
pub fn log_section<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let content = message.as_ref();
            let header = format!("{}📋 {}", indent, content).cyan().bold();
            let underline = "─".repeat(content.len() + 3);
            println!("{}", header);
            println!("{}{}", indent.cyan(), underline.cyan());
        }
    }
}

/// Print a debug line if enabled (backward compatibility)
pub fn log_legacy<T: AsRef<str>>(message: T) {
    unsafe {
        if DEBUG_ENABLED {
            let indent = get_indent_string();
            let prefix = format!("{}[DEBUG]:", indent).red().bold();
            let content = message.as_ref();
            
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    println!("{} {}", prefix, line);
                } else {
                    let line_indent = format!("{}  ", indent);
                    println!("{}{}", line_indent, line);
                }
            }
        }
    }
}

/// Print a formatted debug line if enabled (backward compatibility)
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {{
        $crate::util::debug::log(format!($($arg)*));
    }};
}

/// Print a formatted debug line with info styling
#[macro_export]
macro_rules! dlog_info {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_info(format!($($arg)*));
    }};
}

/// Print a formatted debug line with success styling
#[macro_export]
macro_rules! dlog_success {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_success(format!($($arg)*));
    }};
}

/// Print a formatted debug line with warning styling
#[macro_export]
macro_rules! dlog_warning {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_warning(format!($($arg)*));
    }};
}

/// Print a formatted debug line with error styling
#[macro_export]
macro_rules! dlog_error {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_error(format!($($arg)*));
    }};
}

/// Print a formatted debug line with step styling
#[macro_export]
macro_rules! dlog_step {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_step(format!($($arg)*));
    }};
}

/// Print a formatted debug line with data styling
#[macro_export]
macro_rules! dlog_data {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_data(format!($($arg)*));
    }};
}

/// Print a formatted debug line with file styling
#[macro_export]
macro_rules! dlog_file {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_file(format!($($arg)*));
    }};
}

/// Print a formatted debug line with processing styling
#[macro_export]
macro_rules! dlog_processing {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_processing(format!($($arg)*));
    }};
}

/// Print a formatted debug line with result styling
#[macro_export]
macro_rules! dlog_result {
    ($($arg:tt)*) => {{
        $crate::util::debug::log_result(format!($($arg)*));
    }};
}


