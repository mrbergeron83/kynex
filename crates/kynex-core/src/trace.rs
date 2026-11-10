//! Timing, for finding what blocks the window: run Kynex with `KYNEX_TRACE=1` and every
//! [`span`] that takes 5 ms or more prints one line to stderr when it ends. Off, a span
//! costs a clock read.
//!
//! ```text
//! [trace]   41.2 ms  select_context
//! ```

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Spans shorter than this aren't worth a line.
const MIN: Duration = Duration::from_millis(5);

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KYNEX_TRACE").is_some())
}

/// Times from [`span`] to the end of the scope that holds it.
#[must_use = "a span measures until it is dropped; bind it to a name"]
pub struct Span {
    name: &'static str,
    start: Instant,
}

/// Starts timing `name`; bind it (`let _t = trace::span("…");`) so it lasts the scope.
pub fn span(name: &'static str) -> Span {
    Span { name, start: Instant::now() }
}

impl Drop for Span {
    fn drop(&mut self) {
        let took = self.start.elapsed();
        if took >= MIN && on() {
            eprintln!("[trace] {:>7.1} ms  {}", took.as_secs_f64() * 1000.0, self.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_measures_and_never_panics() {
        let s = span("test");
        std::thread::sleep(Duration::from_millis(6));
        assert!(s.start.elapsed() >= MIN);
        drop(s);
    }
}
