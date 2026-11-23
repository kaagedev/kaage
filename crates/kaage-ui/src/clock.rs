#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClockMode {
    #[default]
    Time,
    TimeSeconds,
    DateTime,
}

impl ClockMode {
    pub fn next(self) -> Self {
        match self {
            Self::Time => Self::TimeSeconds,
            Self::TimeSeconds => Self::DateTime,
            Self::DateTime => Self::Time,
        }
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[cfg(target_arch = "wasm32")]
pub fn format_now(mode: ClockMode) -> String {
    let d = js_sys::Date::new_0();
    let hour = d.get_hours();
    let min = d.get_minutes();
    let sec = d.get_seconds();
    let wday = WEEKDAYS[(d.get_day() as usize).min(6)];
    let mon = MONTHS[(d.get_month() as usize).min(11)];
    let mday = d.get_date();

    match mode {
        ClockMode::Time => format!("{hour:02}:{min:02}"),
        ClockMode::TimeSeconds => format!("{hour:02}:{min:02}:{sec:02}"),
        ClockMode::DateTime => format!("{wday}, {mon} {mday} · {hour:02}:{min:02}"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn format_now(mode: ClockMode) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let hour = sod / 3_600;
    let min = (sod % 3_600) / 60;
    let sec = sod % 60;

    // Howard Hinnant's civil-from-days algorithm (UTC)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let wday_idx = (days + 4).rem_euclid(7) as usize;
    let wday = WEEKDAYS[wday_idx];
    let mon = MONTHS[((month as usize).saturating_sub(1)).min(11)];

    match mode {
        ClockMode::Time => format!("{hour:02}:{min:02} UTC"),
        ClockMode::TimeSeconds => format!("{hour:02}:{min:02}:{sec:02} UTC"),
        ClockMode::DateTime => format!("{wday}, {mon} {mday} · {hour:02}:{min:02} UTC"),
    }
}
