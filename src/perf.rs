//! Opt-in frame-cost probe: `L48_PERF=1` prints how long each egui pass spends in the
//! named sections, and how much wall time separates two passes.

use std::cell::{Cell, RefCell};
use std::time::Instant;

thread_local! {
    static PASSES: Cell<u32> = const { Cell::new(0) };
    static GAP: Cell<f64> = const { Cell::new(0.0) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    static SECTIONS: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
}

pub fn on() -> bool {
    fn cached() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("L48_PERF").is_ok())
    }
    cached()
}

pub struct Guard {
    start: Instant,
    name: &'static str,
    gap: f64,
    on: bool,
}

impl Guard {
    pub fn new(name: &'static str) -> Self {
        if !on() {
            return Guard {
                start: Instant::now(),
                name,
                gap: 0.0,
                on: false,
            };
        }
        let now = Instant::now();
        let gap = if name == "ui" {
            LAST.with(|l| {
                let prev = l.get();
                l.set(Some(now));
                prev.map(|p| p.elapsed().as_secs_f64() * 1000.0)
                    .unwrap_or(0.0)
            })
        } else {
            0.0
        };
        Guard {
            start: now,
            name,
            gap,
            on: true,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.on {
            return;
        }
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        SECTIONS.with(|s| {
            let mut s = s.borrow_mut();
            match s.iter_mut().find(|(n, _)| *n == self.name) {
                Some(e) => e.1 += ms,
                None => s.push((self.name, ms)),
            }
        });
        if self.name != "ui" {
            return;
        }
        PASSES.with(|c| GAP.with(|g| {
            let n = c.get() + 1;
            c.set(n);
            g.set(g.get() + self.gap);
            if n % 40 == 0 {
                let parts: Vec<String> = SECTIONS.with(|s| {
                    let mut v = s.borrow_mut();
                    let out = v
                        .iter()
                        .map(|(name, acc)| format!("{name}={:.2}ms", acc / 40.0))
                        .collect();
                    for e in v.iter_mut() {
                        e.1 = 0.0;
                    }
                    out
                });
                eprintln!(
                    "PERF {}  period={:.2}ms ({} fps)",
                    parts.join(" "),
                    g.get() / 40.0,
                    (1000.0 / (g.get() / 40.0).max(0.01)) as u32
                );
                g.set(0.0);
            }
        }));
    }
}
