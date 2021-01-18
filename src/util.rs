use std::time::SystemTime;

pub fn sanitise_filename(name: &str) -> String {
    if cfg!(windows) {
        sanitize_filename::sanitize_with_options(
            name.trim(),
            sanitize_filename::Options {
                windows: true,
                truncate: true,
                replacement: "-",
            },
        )
    } else {
        name.replace("\0", "-").replace("/", "-")
    }
}

pub fn parse_time(time: &str) -> SystemTime {
    SystemTime::from(
        chrono::DateTime::<chrono::FixedOffset>::parse_from_rfc3339(time)
            .expect("Failed to parse last updated time"),
    )
}
