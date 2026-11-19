// SPDX-License-Identifier: MIT OR Apache-2.0
//! The numbers a fleet card shows, from one sample.

use ztop_core::model::Sample;

/// One card's figures; `None` where the host does not report it.
#[derive(Debug, Clone, PartialEq)]
pub struct CardSummary {
    /// Mean busy percent over all cores, 0 to 100.
    pub cpu_percent: Option<f64>,
    /// Used memory over total, 0 to 100.
    pub memory_percent: Option<f64>,
    /// "3d 4h", "5h 12m" or "7m".
    pub uptime: Option<String>,
    /// "Linux 6.8.0-45" and the like.
    pub system: String,
    pub hostname: String,
}

impl CardSummary {
    pub fn of(sample: &Sample) -> Self {
        let busy = &sample.cpu.busy_percent;
        let cpu_percent = (!busy.is_empty())
            .then(|| (busy.iter().sum::<f64>() / busy.len() as f64).clamp(0.0, 100.0));
        let memory = &sample.memory;
        let memory_percent =
            (memory.total.available && memory.used.available && memory.total.value > 0.0)
                .then(|| (memory.used.value / memory.total.value * 100.0).clamp(0.0, 100.0));
        let uptime = sample
            .uptime
            .available
            .then(|| uptime_text(sample.uptime.value));
        Self {
            cpu_percent,
            memory_percent,
            uptime,
            system: sample.system.clone(),
            hostname: sample.hostname.clone(),
        }
    }
}

/// Seconds as the two largest units, like the iPad and Mac app's cards.
pub fn uptime_text(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (days, hours, minutes) = (total / 86_400, total % 86_400 / 3_600, total % 3_600 / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztop_core::model::{Metric, Unit};

    #[test]
    fn a_card_averages_the_cores_and_divides_memory() {
        let mut sample = crate::blank_sample();
        sample.hostname = "host-a".into();
        sample.cpu.busy_percent = vec![10.0, 30.0, 50.0, 70.0];
        sample.memory.total = Metric::available(8.0, Unit::Bytes);
        sample.memory.used = Metric::available(2.0, Unit::Bytes);
        sample.uptime = Metric::available(3.0 * 86_400.0 + 4.0 * 3_600.0 + 59.0, Unit::Seconds);
        let card = CardSummary::of(&sample);
        assert_eq!(card.cpu_percent, Some(40.0));
        assert_eq!(card.memory_percent, Some(25.0));
        assert_eq!(card.uptime.as_deref(), Some("3d 4h"));
        assert_eq!(card.hostname, "host-a");
    }

    #[test]
    fn unreported_figures_are_none_not_zero() {
        let card = CardSummary::of(&crate::blank_sample());
        assert_eq!(card.cpu_percent, None);
        assert_eq!(card.memory_percent, None);
        assert_eq!(card.uptime, None);
    }

    #[test]
    fn uptime_uses_the_two_largest_units() {
        assert_eq!(uptime_text(59.0), "0m");
        assert_eq!(uptime_text(7.0 * 60.0), "7m");
        assert_eq!(uptime_text(5.0 * 3600.0 + 12.0 * 60.0), "5h 12m");
        assert_eq!(uptime_text(-1.0), "0m");
    }
}
