use chrono::SecondsFormat;

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
