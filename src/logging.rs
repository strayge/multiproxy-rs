pub fn config_logging(
    info: bool,
    debug: bool,
    trace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut level = log::LevelFilter::Warn;
    if info {
        level = log::LevelFilter::Info;
    } else if debug {
        level = log::LevelFilter::Debug;
    } else if trace {
        level = log::LevelFilter::Trace;
    }
    let config = simplelog::ConfigBuilder::new()
        .set_time_format_custom(simplelog::format_description!(
            "[hour]:[minute]:[second].[subsecond digits:3]"
        ))
        .set_level_padding(simplelog::LevelPadding::Right)
        .set_thread_level(simplelog::LevelFilter::Off)
        .set_location_level(simplelog::LevelFilter::Off)
        .build();
    simplelog::TermLogger::init(
        level,
        config,
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )?;
    Ok(())
}
