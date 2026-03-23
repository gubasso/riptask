use console::style;
use std::io::IsTerminal;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn success(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{} {msg}", style("✓").green());
    } else {
        eprintln!("{msg}");
    }
}

pub fn info(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{} {msg}", style("ℹ").cyan());
    } else {
        eprintln!("{msg}");
    }
}

pub fn warn(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{} {msg}", style("⚠").yellow());
    } else {
        eprintln!("warning: {msg}");
    }
}

pub fn error(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{} {msg}", style("✗").red());
    } else {
        eprintln!("{msg}");
    }
}
