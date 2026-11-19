// SPDX-License-Identifier: MIT OR Apache-2.0
//! A host's recent samples and the series its detail page draws.
//!
//! The page opens with the agent's own history (`/v1/history`) and then
//! appends every sample the fleet poll reads, so the charts keep
//! moving without asking the agent for the whole window again.

use std::collections::VecDeque;

use ztop_core::model::Sample;

/// How far back the detail page looks.
pub const WINDOW_SECONDS: u64 = 15 * 60;

/// One line on a chart: `(seconds before the newest sample, value)`,
/// oldest first. Gaps (a sample without the figure) are left out.
pub type Points = Vec<(f64, f64)>;

/// A named line, as a legend shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub label: String,
    pub points: Points,
}

/// Samples in time order, at most [`WINDOW_SECONDS`] apart from the
/// newest, without their process lists (the page never reads them).
#[derive(Debug, Default, Clone)]
pub struct History {
    samples: VecDeque<Sample>,
}

impl History {
    /// Adds samples in any order; a timestamp already held is skipped,
    /// so the agent's history and the poll can overlap.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Sample>) {
        for sample in samples {
            self.push(sample);
        }
    }

    pub fn push(&mut self, mut sample: Sample) {
        sample.processes.clear();
        let at = sample.timestamp_ms;
        match self.samples.back() {
            Some(last) if last.timestamp_ms == at => return,
            Some(last) if last.timestamp_ms < at => self.samples.push_back(sample),
            None => self.samples.push_back(sample),
            Some(_) => {
                let index = self.samples.partition_point(|s| s.timestamp_ms < at);
                if self
                    .samples
                    .get(index)
                    .is_some_and(|s| s.timestamp_ms == at)
                {
                    return;
                }
                self.samples.insert(index, sample);
            }
        }
        let newest = self.samples.back().map_or(0, |s| s.timestamp_ms);
        let oldest_kept = newest.saturating_sub(WINDOW_SECONDS * 1000);
        while self
            .samples
            .front()
            .is_some_and(|s| s.timestamp_ms < oldest_kept)
        {
            self.samples.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    /// One value per sample, placed by its age.
    fn series(&self, value: impl Fn(&Sample) -> Option<f64>) -> Points {
        let Some(newest) = self.latest().map(|s| s.timestamp_ms) else {
            return Vec::new();
        };
        self.samples
            .iter()
            .filter_map(|s| {
                let age = newest.saturating_sub(s.timestamp_ms) as f64 / 1000.0;
                value(s).filter(|v| v.is_finite()).map(|v| (age, v))
            })
            .collect()
    }

    /// Mean busy percent over all cores.
    pub fn cpu(&self) -> Points {
        self.series(|s| {
            let busy = &s.cpu.busy_percent;
            (!busy.is_empty()).then(|| busy.iter().sum::<f64>() / busy.len() as f64)
        })
    }

    /// Used memory and used swap, as percents of their totals.
    pub fn memory(&self) -> Vec<Line> {
        let percent = |used: &ztop_core::model::Metric, total: &ztop_core::model::Metric| {
            (used.available && total.available && total.value > 0.0)
                .then(|| used.value / total.value * 100.0)
        };
        let lines = [
            Line {
                label: "used".into(),
                points: self.series(|s| percent(&s.memory.used, &s.memory.total)),
            },
            Line {
                label: "swap".into(),
                points: self.series(|s| percent(&s.memory.swap_used, &s.memory.swap_total)),
            },
        ];
        lines.into_iter().filter(|l| !l.points.is_empty()).collect()
    }

    /// The 1, 5 and 15 minute load averages.
    pub fn load(&self) -> Vec<Line> {
        ["1 min", "5 min", "15 min"]
            .into_iter()
            .enumerate()
            .map(|(i, label)| Line {
                label: label.into(),
                points: self.series(|s| Some(s.load_avg[i])),
            })
            .collect()
    }

    /// Read and write bytes per second, summed over every disk.
    pub fn disks(&self) -> Vec<Line> {
        let sum = |pick: fn(&ztop_core::model::Disk) -> Option<f64>| {
            move |s: &Sample| {
                let rates: Vec<f64> = s.disks.iter().filter_map(pick).collect();
                (!rates.is_empty()).then(|| rates.iter().sum())
            }
        };
        let lines = [
            Line {
                label: "read".into(),
                points: self.series(sum(|d| d.read_rate)),
            },
            Line {
                label: "write".into(),
                points: self.series(sum(|d| d.write_rate)),
            },
        ];
        lines.into_iter().filter(|l| !l.points.is_empty()).collect()
    }

    /// Received and sent bytes per second over every interface except
    /// loopback, which only shows the machine talking to itself.
    pub fn network(&self) -> Vec<Line> {
        let sum = |pick: fn(&ztop_core::model::NetIf) -> Option<f64>| {
            move |s: &Sample| {
                let rates: Vec<f64> = s
                    .net
                    .iter()
                    .filter(|n| !is_loopback(&n.name))
                    .filter_map(pick)
                    .collect();
                (!rates.is_empty()).then(|| rates.iter().sum())
            }
        };
        let lines = [
            Line {
                label: "received".into(),
                points: self.series(sum(|n| n.rx_rate)),
            },
            Line {
                label: "sent".into(),
                points: self.series(sum(|n| n.tx_rate)),
            },
        ];
        lines.into_iter().filter(|l| !l.points.is_empty()).collect()
    }

    /// One line per temperature sensor the newest sample reports, by
    /// label, hottest first, at most `limit`.
    pub fn temperatures(&self, limit: usize) -> Vec<Line> {
        let Some(latest) = self.latest() else {
            return Vec::new();
        };
        let mut sensors: Vec<(&str, f64)> = latest
            .sensors
            .temps
            .iter()
            .filter(|t| t.celsius.available)
            .map(|t| (t.label.as_str(), t.celsius.value))
            .collect();
        sensors.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        sensors.dedup_by(|a, b| a.0 == b.0);
        sensors
            .into_iter()
            .take(limit)
            .map(|(label, _)| Line {
                label: label.to_string(),
                points: self.series(|s| {
                    s.sensors
                        .temps
                        .iter()
                        .find(|t| t.label == label && t.celsius.available)
                        .map(|t| t.celsius.value)
                }),
            })
            .collect()
    }

    /// One line per fan the newest sample reports, in RPM.
    pub fn fans(&self) -> Vec<Line> {
        let Some(latest) = self.latest() else {
            return Vec::new();
        };
        latest
            .sensors
            .fans
            .iter()
            .filter(|f| f.rpm.available)
            .map(|fan| Line {
                label: fan.label.clone(),
                points: self.series(|s| {
                    s.sensors
                        .fans
                        .iter()
                        .find(|f| f.label == fan.label && f.rpm.available)
                        .map(|f| f.rpm.value)
                }),
            })
            .collect()
    }
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo0")
}

/// The top of a chart's scale: the largest value with some headroom,
/// rounded up to a 1, 2 or 5 step, so axes read as round numbers.
/// Percent charts pass `Some(100.0)` and keep it.
pub fn scale_top(lines: &[&Points], fixed: Option<f64>) -> f64 {
    if let Some(top) = fixed {
        return top;
    }
    let max = lines
        .iter()
        .flat_map(|points| points.iter().map(|p| p.1))
        .fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f64.powf((max * 1.1).log10().floor());
    for step in [1.0, 2.0, 5.0, 10.0] {
        if magnitude * step >= max * 1.1 {
            return magnitude * step;
        }
    }
    magnitude * 10.0
}

/// Bytes per second as the largest unit that keeps the figure under
/// 1000: "512 B/s", "1.5 MB/s".
pub fn rate_text(bytes_per_second: f64) -> String {
    format!("{}/s", bytes_text(bytes_per_second))
}

/// Bytes in decimal units, as the agent's peers (GNOME's monitor) do.
pub fn bytes_text(bytes: f64) -> String {
    let mut value = bytes.max(0.0);
    for unit in ["B", "kB", "MB", "GB", "TB"] {
        if value < 1000.0 || unit == "TB" {
            return if unit == "B" {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1000.0;
    }
    unreachable!("the loop returns at TB")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztop_core::model::{Metric, Unit};

    fn at(seconds: u64, busy: f64) -> Sample {
        let mut sample = crate::blank_sample();
        sample.timestamp_ms = 1_700_000_000_000 + seconds * 1000;
        sample.cpu.busy_percent = vec![busy, busy];
        sample
    }

    #[test]
    fn history_keeps_time_order_skips_repeats_and_trims_the_window() {
        let mut history = History::default();
        history.extend([at(10, 1.0), at(12, 2.0), at(11, 3.0), at(12, 9.0)]);
        assert_eq!(history.len(), 3);
        assert_eq!(history.cpu(), vec![(2.0, 1.0), (1.0, 3.0), (0.0, 2.0)]);
        history.push(at(10 + WINDOW_SECONDS + 1, 4.0));
        assert_eq!(
            history.len(),
            3,
            "only the sample at 10 s fell out of the window"
        );
        assert_eq!(history.cpu()[0], (WINDOW_SECONDS as f64, 3.0));
        assert!(history.latest().unwrap().processes.is_empty());
    }

    #[test]
    fn series_leave_out_samples_without_the_figure() {
        let mut history = History::default();
        let mut no_cpu = at(1, 0.0);
        no_cpu.cpu.busy_percent.clear();
        history.extend([at(0, 50.0), no_cpu, at(2, 70.0)]);
        assert_eq!(history.cpu(), vec![(2.0, 50.0), (0.0, 70.0)]);
        // The fixture blanks memory, so the memory chart has no lines.
        assert!(history.memory().is_empty());
    }

    #[test]
    fn memory_is_a_percent_and_swap_only_with_a_total() {
        let mut sample = at(0, 0.0);
        sample.memory.total = Metric::available(8.0, Unit::Bytes);
        sample.memory.used = Metric::available(2.0, Unit::Bytes);
        sample.memory.swap_total = Metric::available(0.0, Unit::Bytes);
        sample.memory.swap_used = Metric::available(0.0, Unit::Bytes);
        let mut history = History::default();
        history.push(sample);
        let lines = history.memory();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].label, "used");
        assert_eq!(lines[0].points, vec![(0.0, 25.0)]);
    }

    #[test]
    fn network_sums_interfaces_but_not_loopback() {
        let mut sample = at(0, 0.0);
        let interface = |name: &str, rx: f64| ztop_core::model::NetIf {
            name: name.into(),
            rx_bytes: 0,
            rx_packets: 0,
            tx_bytes: 0,
            tx_packets: 0,
            rx_rate: Some(rx),
            tx_rate: Some(1.0),
        };
        sample.net = vec![
            interface("lo", 1e9),
            interface("eth0", 100.0),
            interface("wlan0", 50.0),
        ];
        let mut history = History::default();
        history.push(sample);
        let lines = history.network();
        assert_eq!(lines[0].points, vec![(0.0, 150.0)]);
        assert_eq!(lines[1].points, vec![(0.0, 2.0)]);
    }

    #[test]
    fn temperatures_follow_the_hottest_sensors_and_fans_their_rpm() {
        use ztop_core::model::{Fan, TempSensor};
        let reading = |seconds: u64, cpu: f64, nvme: f64, rpm: f64| {
            let mut sample = at(seconds, 0.0);
            sample.sensors.temps = vec![
                TempSensor {
                    label: "nvme".into(),
                    celsius: Metric::available(nvme, Unit::Celsius),
                },
                TempSensor {
                    label: "cpu".into(),
                    celsius: Metric::available(cpu, Unit::Celsius),
                },
                TempSensor {
                    label: "acpi".into(),
                    celsius: Metric::unavailable(Unit::Celsius),
                },
            ];
            sample.sensors.fans = vec![Fan {
                label: "fan1".into(),
                rpm: Metric::available(rpm, Unit::RevolutionsPerMinute),
            }];
            sample
        };
        let mut history = History::default();
        history.extend([
            reading(0, 50.0, 40.0, 1200.0),
            reading(1, 61.0, 41.0, 1500.0),
        ]);
        let temps = history.temperatures(6);
        let labels: Vec<&str> = temps.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(
            labels,
            ["cpu", "nvme"],
            "hottest first, unreadable sensors left out"
        );
        assert_eq!(temps[0].points, vec![(1.0, 50.0), (0.0, 61.0)]);
        assert_eq!(history.temperatures(1).len(), 1);
        let fans = history.fans();
        assert_eq!(fans[0].label, "fan1");
        assert_eq!(fans[0].points, vec![(1.0, 1200.0), (0.0, 1500.0)]);
    }

    #[test]
    fn scales_round_up_to_one_two_or_five() {
        let points: Points = vec![(0.0, 37.0)];
        assert_eq!(scale_top(&[&points], None), 50.0);
        let points: Points = vec![(0.0, 1_400.0)];
        assert_eq!(scale_top(&[&points], None), 2_000.0);
        let points: Points = vec![(0.0, 9.5)];
        assert_eq!(scale_top(&[&points], None), 20.0);
        assert_eq!(scale_top(&[], None), 1.0);
        assert_eq!(scale_top(&[&points], Some(100.0)), 100.0);
    }

    #[test]
    fn rates_use_decimal_units() {
        assert_eq!(rate_text(512.0), "512 B/s");
        assert_eq!(rate_text(1_500_000.0), "1.5 MB/s");
        assert_eq!(bytes_text(2.5e12), "2.5 TB");
        assert_eq!(bytes_text(-3.0), "0 B");
    }
}
