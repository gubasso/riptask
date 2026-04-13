use console::style;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::borrow::Cow;
use std::io::IsTerminal;
use std::time::Duration;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn spinner(msg: impl Into<Cow<'static, str>>) -> Option<ProgressBar> {
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let pb = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed}] {msg}")
            .unwrap(),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(100));
    Some(pb)
}

pub fn spin_on<T, E, F>(msg: &str, f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E>,
    E: std::fmt::Display,
{
    let started = std::time::Instant::now();
    let pb = spinner(msg.to_owned());
    let result = f();
    if let Some(ref pb) = pb {
        pb.finish_and_clear();
    }
    let elapsed = format_elapsed(started.elapsed());
    match &result {
        Ok(_) => success(&format!("{msg} done ({elapsed})")),
        Err(_) => error(&format!("{msg} failed ({elapsed})")),
    }
    result
}

pub async fn spin_on_async<T, E, F>(msg: &str, fut: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let started = std::time::Instant::now();
    let pb = spinner(msg.to_owned());
    let result = fut.await;
    if let Some(ref pb) = pb {
        pb.finish_and_clear();
    }
    let elapsed = format_elapsed(started.elapsed());
    match &result {
        Ok(_) => success(&format!("{msg} done ({elapsed})")),
        Err(_) => error(&format!("{msg} failed ({elapsed})")),
    }
    result
}

pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
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
