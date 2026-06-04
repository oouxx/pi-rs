use std::time::Instant;

pub struct Timings {
    enabled: bool,
    timings: Vec<(String, u128)>,
    last_time: Instant,
}

impl Timings {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            timings: Vec::new(),
            last_time: Instant::now(),
        }
    }

    pub fn new_with_env() -> Self {
        let enabled = std::env::var("PI_TIMING")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        Self::new(enabled)
    }

    pub fn reset(&mut self) {
        if !self.enabled {
            return;
        }
        self.timings.clear();
        self.last_time = Instant::now();
    }

    pub fn time(&mut self, label: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_time).as_millis();
        self.timings.push((label.to_string(), elapsed));
        self.last_time = now;
    }

    pub fn print(&self) {
        if !self.enabled || self.timings.is_empty() {
            return;
        }
        eprintln!("\n--- Startup Timings ---");
        let total: u128 = self.timings.iter().map(|(_, ms)| ms).sum();
        for (label, ms) in &self.timings {
            eprintln!("  {}: {}ms", label, ms);
        }
        eprintln!("  TOTAL: {}ms", total);
        eprintln!("------------------------\n");
    }

    pub fn timings(&self) -> &[(String, u128)] {
        &self.timings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_does_nothing() {
        let mut t = Timings::new(false);
        t.time("test");
        assert!(t.timings().is_empty());
    }

    #[test]
    fn test_enabled_records_timings() {
        let mut t = Timings::new(true);
        t.time("first");
        std::thread::sleep(std::time::Duration::from_millis(1));
        t.time("second");
        assert_eq!(t.timings().len(), 2);
    }

    #[test]
    fn test_reset_clears() {
        let mut t = Timings::new(true);
        t.time("first");
        assert_eq!(t.timings().len(), 1);
        t.reset();
        assert!(t.timings().is_empty());
    }

    #[test]
    fn test_print_doesnt_panic_when_disabled() {
        let t = Timings::new(false);
        t.print();
    }
}
