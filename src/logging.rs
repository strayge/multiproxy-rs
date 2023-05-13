pub fn config_logging() -> Result<(), Box<dyn std::error::Error>> {
    let config = simplelog::ConfigBuilder::new()
        .set_time_format_custom(simplelog::format_description!(
            "[hour]:[minute]:[second].[subsecond digits:3]"
        ))
        .set_level_padding(simplelog::LevelPadding::Right)
        .set_thread_level(simplelog::LevelFilter::Off)
        .build();
    simplelog::TermLogger::init(
        log::LevelFilter::Debug,
        config,
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )?;
    Ok(())
}
