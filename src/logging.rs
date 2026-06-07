use crate::{Config, config::LogFormat};
use tracing_subscriber::fmt;

pub(super) fn setup(config: &Config) {
    let format = fmt::format();
    let subscriber = fmt::fmt().with_max_level(config.logging.level.value);
    // TODO: handle path redacting
    match config.logging.format.value {
        LogFormat::Json => subscriber
            .event_format(format.json().flatten_event(true))
            .init(),
        LogFormat::Pretty => subscriber.event_format(format.pretty()).init(),
        LogFormat::Compact => subscriber.event_format(format.compact()).init(),
    }
}
