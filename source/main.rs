use clap::Parser as _;
use std::path;

#[derive(clap::Parser)]
struct Arguments {
  #[clap(flatten)]
  arguments: sin::Arguments,
  #[arg(
    long = "log-directory",
    help = "Log directory",
    default_value_t = String::from("$ENV{XDG_RUNTIME_DIR}")
  )]
  pub log_directory: String,
  #[clap(flatten)]
  verbose: clap_verbosity_flag::Verbosity<clap_verbosity_flag::InfoLevel>,
}

fn main() -> anyhow::Result<()> {
  let arguments = Arguments::parse();

  let encoder = Box::new(log4rs::encode::pattern::PatternEncoder::new(
    "{d(%F %T)} {l} {t} - {m}{n}",
  ));
  log4rs::init_config(
    log4rs::config::Config::builder()
      .appender(
        log4rs::config::Appender::builder()
          .filter(Box::new(log4rs::filter::threshold::ThresholdFilter::new(
            log::LevelFilter::Trace,
          )))
          .build(
            "file",
            Box::new(
              log4rs::append::file::FileAppender::builder()
                .encoder(encoder.clone())
                .build(
                  path::Path::new(&arguments.log_directory)
                    .join(format!("{}.log", arguments.arguments.namespace)),
                )?,
            ),
          ),
      )
      .appender(
        log4rs::config::Appender::builder()
          .filter(Box::new(log4rs::filter::threshold::ThresholdFilter::new(
            arguments.verbose.log_level_filter(),
          )))
          .build(
            "console",
            Box::new(
              log4rs::append::console::ConsoleAppender::builder()
                .encoder(encoder)
                .build(),
            ),
          ),
      )
      .build(
        log4rs::config::Root::builder()
          .appenders(["console", "file"])
          .build(log::LevelFilter::Trace),
      )?,
  )?;

  sin::run(&arguments.arguments)
}
