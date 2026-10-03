use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use ios_release_native::{
    archive, config::App, fsutil, process::Native, qa, results, screenshots, toolchain,
};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Native iOS build and simulator automation; explicit opt-in, no legacy workflow changes"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    app_root: PathBuf,
    #[arg(
        long,
        global = true,
        env = "IOS_RELEASE_CONFIG",
        default_value = ".github/ios-release.json"
    )]
    config: PathBuf,
    #[arg(long)]
    commands_json: bool,
    #[arg(
        long,
        global = true,
        help = "Print the operation plan without invoking tools or changing files"
    )]
    plan: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Doctor {
        #[arg(long)]
        xcode: bool,
    },
    Toolchain {
        #[arg(long)]
        compatibility: bool,
        #[arg(long)]
        screenshots: bool,
        #[arg(long)]
        destinations_json: bool,
    },
    Qa {
        #[arg(value_parser = ["lint", "localization", "analyze", "test", "test-compatibility"])]
        check: String,
    },
    Test,
    XcresultReport {
        #[arg(long)]
        summary: PathBuf,
        #[arg(long)]
        tests: PathBuf,
    },
    Archive {
        #[arg(long, env = "VERSION")]
        version: String,
        #[arg(long, env = "BUILD_NUMBER")]
        build_number: String,
    },
    #[command(alias = "screenshots-capture")]
    Screenshots {
        #[arg(long)]
        scheme: String,
        #[arg(long)]
        test_target: String,
        #[arg(long, value_delimiter = ',')]
        devices: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        languages: Vec<String>,
        #[arg(long)]
        only_testing: Vec<String>,
        #[arg(long, default_value = "build/rust-screenshots/images")]
        output: PathBuf,
        #[arg(long)]
        photo: Vec<PathBuf>,
        #[arg(long)]
        video: Vec<PathBuf>,
    },
}

fn main() {
    match run(Cli::parse()) {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    if cli.commands_json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema_version":1,"implementation":"rust","experimental":true,"commands":["doctor","toolchain","qa","test","archive","screenshots","screenshots-capture","xcresult-report"],"apple_store_mutations":false})
            )?
        );
        return Ok(0);
    }
    let Some(command) = cli.command else {
        Cli::command().print_help()?;
        println!();
        return Ok(0);
    };
    if matches!(command, Commands::Doctor { xcode: false }) {
        println!(
            "{}",
            json!({"implementation":"rust","version":env!("CARGO_PKG_VERSION"),"runtime_dependencies":[],"native_builds_require":"macOS and the configured Xcode","legacy_workflows":"unchanged"})
        );
        return Ok(0);
    }
    if let Commands::XcresultReport { summary, tests } = &command {
        let summary = serde_json::from_slice(&std::fs::read(summary)?)?;
        let tests = serde_json::from_slice(&std::fs::read(tests)?)?;
        let report = results::junit(&summary, &tests)?;
        println!(
            "{}",
            json!({"passed":report.passed,"executed":report.executed,"bootstrap_recoverable":results::bootstrap_failure(&summary,&tests),"junit":report.xml})
        );
        return Ok(0);
    }
    let app = App::load(&cli.app_root, &cli.config)?;
    let mut executor = Native;
    match command {
        Commands::Doctor { .. }
        | Commands::Toolchain {
            compatibility: false,
            screenshots: false,
            destinations_json: false,
        } => {
            if cli.plan {
                println!(
                    "{}",
                    json!({"operation":"toolchain","expected":app.config["xcode"],"device":app.config["test_device"]})
                );
            } else {
                let selected = toolchain::resolve(
                    &app,
                    false,
                    &[app.text("test_device")?.to_owned()],
                    &mut executor,
                )?;
                println!("{}", serde_json::to_string_pretty(&selected.evidence)?);
            }
        }
        Commands::Toolchain {
            compatibility,
            screenshots,
            destinations_json,
        } => {
            let names = if screenshots {
                app.names("screenshot_devices")?
            } else {
                vec![app.text("test_device")?.to_owned()]
            };
            if cli.plan {
                println!(
                    "{}",
                    json!({"operation":"toolchain","expected":app.xcode(compatibility)?,"devices":names})
                );
            } else {
                let selected = toolchain::resolve(&app, compatibility, &names, &mut executor)?;
                fsutil::json(&app.root.join("build/build-env.json"), &selected.evidence)?;
                println!(
                    "{}",
                    if destinations_json {
                        serde_json::to_string_pretty(&selected.devices)?
                    } else {
                        serde_json::to_string_pretty(&selected.evidence)?
                    }
                );
            }
        }
        Commands::Qa { check } => {
            return run_qa(cli.plan, &app, &check, &mut executor);
        }
        Commands::Test => {
            return run_qa(cli.plan, &app, "test", &mut executor);
        }
        Commands::Archive {
            version,
            build_number,
        } => {
            if cli.plan {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"operation":"archive","steps":archive::steps(&app,&version,&build_number)?,"export_options":archive::options(&app)?})
                    )?
                );
            } else {
                archive::run(&app, &version, &build_number, &mut executor)?;
            }
        }
        Commands::Screenshots {
            scheme,
            test_target,
            devices,
            languages,
            only_testing,
            output,
            photo,
            video,
        } => {
            let options = screenshots::Options {
                scheme,
                test_target,
                devices: if devices.is_empty() {
                    app.names("screenshot_devices")?
                } else {
                    devices
                },
                languages: if languages.is_empty() {
                    app.names("locales")?
                } else {
                    languages
                },
                only_testing,
                output,
                photos: photo,
                videos: video,
            };
            options.validate(&app)?;
            if cli.plan {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"operation":"screenshots","options":options,"strategy":"build once per selected device; test-without-building per locale; one recognized hosted bootstrap recovery","steps":[screenshots::build_step(&app,&options,"<resolved-device-uuid>",&app.root.join("build/rust-screenshots/<job>/derived-data"))?,screenshots::test_step(&app,&options,"<resolved-device-uuid>",&app.root.join("build/rust-screenshots/<job>/Build/Products/tests.xctestrun"),&app.root.join("build/rust-screenshots/<job>/result.xcresult"))?]})
                    )?
                );
            } else {
                screenshots::run(&app, &options, &mut executor)?;
            }
        }
        Commands::XcresultReport { .. } => unreachable!(),
    }
    Ok(0)
}

fn run_qa(plan: bool, app: &App, check: &str, executor: &mut Native) -> Result<i32> {
    if plan {
        let workers = std::env::var("TEST_WORKERS").unwrap_or_else(|_| "1".into());
        let value = if ["lint", "localization"].contains(&check) {
            json!({"operation":"qa","check":check,"native":true})
        } else {
            json!({"operation":"qa","check":check,"step":qa::command(app,check,"platform=iOS Simulator,id=<resolved-device-uuid>",&app.root.join("build/test_output/<unique>.xcresult"),&workers)?})
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&value).context("Cannot serialize QA plan")?
        );
        Ok(0)
    } else {
        qa::run(app, check, executor)
    }
}
