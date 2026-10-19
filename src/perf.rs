//! Opt-in frame-cost probe: `L48_PERF=1` prints how long each egui pass spends
//! in `App::ui` and how much wall time separates two passes.

use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(0) };
    static CPU: Cell<f64> = const { Cell::new(0.0) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    static GAP: Cell<f64> = const { Cell::new(0.0) };
}

pub struct Guard {
    start: Instant,
    gap: f64,
    on: bool,
}

impl Guard {
    pub fn new() -> Self {
        let on = std::env::var("L48_PERF").is_ok();
        if !on {
            return Guard {
                start: Instant::now(),
                gap: 0.0,
                on,
            };
        }
        let now = Instant::now();
        let gap = LAST.with(|l| {
            let prev = l.get();
            l.set(Some(now));
            prev.map(|p| p.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0)
        });
        Guard {
            start: now,
            gap,
            on,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.on {
            return;
        }
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        COUNT.with(|c| {
            CPU.with(|a| GAP.with(|g| {
                let n = c.get() + 1;
                c.set(n);
                a.set(a.get() + ms);
                g.set(g.get() + self.gap);
                if n % 40 == 0 {
                    eprintln!(
                        "PERF ui={:.2}ms/frame  period={:.2}ms  ({:.1} fps)",
                        a.get() / 40.0,
                        g.get() / 40.0,
                        1000.0 / (g.get() / 40.0).max(0.01)
                    );
                    a.set(0.0);
                    g.set(0.0);
                }
            }))
        });
    }
}
