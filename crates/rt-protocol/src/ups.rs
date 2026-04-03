use serde::{Deserialize, Serialize};

/// Raw UPS telemetry. Integer types avoid float comparison issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsStatus {
    pub battery_percent: u8,
    pub battery_voltage_mv: u16,
    pub charging: bool,
    pub power_plugged: bool,
    pub temperature_cdeg: i16,
    pub sampled_at: i64,
}

impl UpsStatus {
    /// Compare readings only, ignoring `sampled_at` which changes every poll.
    pub fn same_readings(&self, other: &Self) -> bool {
        self.battery_percent == other.battery_percent
            && self.battery_voltage_mv == other.battery_voltage_mv
            && self.charging == other.charging
            && self.power_plugged == other.power_plugged
            && self.temperature_cdeg == other.temperature_cdeg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_status() -> UpsStatus {
        UpsStatus {
            battery_percent: 85,
            battery_voltage_mv: 4120,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 2350,
            sampled_at: 1700000000,
        }
    }

    #[test]
    fn same_readings_ignores_sampled_at() {
        let a = sample_status();
        let mut b = a.clone();
        b.sampled_at = a.sampled_at + 9999;
        assert!(a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_battery_percent_change() {
        let a = sample_status();
        let mut b = a.clone();
        b.battery_percent = 84;
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_battery_voltage_change() {
        let a = sample_status();
        let mut b = a.clone();
        b.battery_voltage_mv = 4100;
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_charging_change() {
        let a = sample_status();
        let mut b = a.clone();
        b.charging = false;
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_power_plugged_change() {
        let a = sample_status();
        let mut b = a.clone();
        b.power_plugged = false;
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_temperature_change() {
        let a = sample_status();
        let mut b = a.clone();
        b.temperature_cdeg = 2400;
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn serde_round_trip() {
        let status = sample_status();
        let json = serde_json::to_string(&status).unwrap();
        let parsed: UpsStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, status);
    }
}
