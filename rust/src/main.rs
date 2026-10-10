use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use ios_release_native::{
    api::Apple, archive, config::App, fsutil, metadata, onboarding, process::Native, qa, release,
    results, screenshots, signing, store, toolchain,
};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "ios-release",
    version = cli_version(),
    about = "Native iOS signing, QA, TestFlight and App Store releases",
    after_help = "Start in your app folder:\n  ios-release init\n  ios-release qa all\n\nGuides: https://github.com/northcutted/ios-release-workflows/blob/main/docs/README.md"
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "IOS_APP_ROOT",
        default_value = ".",
        help = "App folder containing the Xcode project and configuration"
    )]
    app_root: PathBuf,
    #[arg(
        long,
        global = true,
        env = "IOS_RELEASE_CONFIG",
        default_value = ".github/ios-release.json",
        help = "App configuration; falls back to .ios-release.json if the default is absent"
    )]
    config: PathBuf,
    #[arg(long, help = "Print the native command summary as JSON")]
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
    /// Configure protected GitHub Actions or retrieve a verified preparation
    Github {
        #[command(subcommand)]
        action: Github,
    },
    /// Discover an Xcode app and create configuration, store content, and Actions
    Init {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        scheme: Option<String>,
        #[arg(long)]
        team_id: Option<String>,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        xcode: Option<String>,
        #[arg(
            long,
            help = "Create local app configuration without an Actions workflow"
        )]
        no_workflows: bool,
        #[arg(long,action=clap::ArgAction::Set)]
        tracking: Option<bool>,
        #[arg(long,action=clap::ArgAction::Set)]
        non_exempt_encryption: Option<bool>,
        #[arg(long, help = "Hosted macOS runner for the generated Actions workflow")]
        runner: Option<String>,
        #[arg(long,default_value=env!("IOS_RELEASE_BUILD_REVISION"))]
        platform_revision: String,
    },
    /// Store or inspect your private App Store Connect team API credentials
    Auth {
        #[command(subcommand)]
        action: Auth,
    },
    /// Manage distribution certificates, profiles, and the encrypted signing vault
    Signing {
        #[command(subcommand)]
        action: Signing,
    },
    /// Prepare a tested, signed release with an exact IPA identity
    Release {
        #[command(subcommand)]
        action: Release,
    },
    /// Validate content, deliver a release, or manage its App Store state
    Store {
        #[command(subcommand)]
        action: Store,
        #[arg(long, global = true, default_value = "build/native-release")]
        release: PathBuf,
    },
    /// Read production and TestFlight status for the configured app
    Status,
    /// Show CLI/source identity and runtime requirements; optionally check Xcode
    Doctor {
        #[arg(long)]
        xcode: bool,
    },
    /// Validate configured Xcode and resolve exact simulator destinations
    Toolchain {
        #[arg(long)]
        compatibility: bool,
        #[arg(long)]
        screenshots: bool,
        #[arg(long)]
        destinations_json: bool,
    },
    /// Run configured tests, analysis, localization, and optional lint
    Qa {
        #[arg(value_parser = ["all", "lint", "localization", "analyze", "test", "test-compatibility"])]
        check: String,
    },
    /// Run the configured unit test targets (alias for qa test)
    Test,
    /// Reconcile exported XCTest summary and test cases into validated evidence
    XcresultReport {
        #[arg(long)]
        summary: PathBuf,
        #[arg(long)]
        tests: PathBuf,
    },
    /// Archive and export with installed signing assets, without delivery
    Archive {
        #[arg(long, env = "VERSION")]
        version: String,
        #[arg(long, env = "BUILD_NUMBER")]
        build_number: String,
    },
    /// Capture app-owned UI-test scenarios for configured devices and locales
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
        #[arg(
            long,
            help = "Use an app-local cache; requires a SnapshotHelper that honors IOS_RELEASE_SNAPSHOT_HOME"
        )]
        isolated_cache: bool,
    },
}

#[derive(Subcommand)]
enum Auth {
    /// Save a team API key privately outside the app repository
    Login {
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        issuer_id: String,
        #[arg(long)]
        key_file: PathBuf,
    },
    /// Inspect the stored key's public identity
    Status,
}
#[derive(Subcommand)]
enum Signing {
    /// Export a public signing-input snapshot for separated Actions jobs
    Export {
        #[arg(long, default_value = "build/native-signing/signing.json")]
        output: PathBuf,
    },
    /// Apply a signing-input snapshot to the archive configuration
    Apply {
        #[arg(long)]
        input: PathBuf,
    },
    /// Import an existing distribution identity from a password-protected P12
    Import {
        #[arg(long)]
        p12: PathBuf,
        #[arg(long, default_value = "IOS_RELEASE_P12_PASSWORD")]
        password_env: String,
    },
    /// Reconcile distribution certificates/profiles and update archive signing
    Sync {
        #[arg(long)]
        no_project_changes: bool,
    },
    /// Inspect vault assets and renewal state
    Status,
    /// Configure manual signing for the archive configuration
    ConfigureProject,
}
#[derive(Subcommand)]
enum Release {
    /// Build and export a signed archive (advanced preparation step)
    Build {
        #[arg(long)]
        version: String,
        #[arg(long)]
        build_number: String,
        #[arg(long)]
        installed_signing: bool,
    },
    /// Independently verify an exported IPA and matching symbols
    VerifyArchive {
        #[arg(long)]
        version: String,
        #[arg(long)]
        build_number: String,
    },
    /// Package an existing preparation directory
    Pack {
        #[arg(long, default_value = "build/native-release")]
        release: PathBuf,
        #[arg(long, default_value = "build/native-release.zip")]
        output: PathBuf,
    },
    /// Run configured QA, build a signed IPA, and seal a local release
    Prepare {
        #[arg(long, default_value = "1.0.0")]
        version: String,
        #[arg(long)]
        build_number: Option<String>,
        #[arg(long)]
        installed_signing: bool,
    },
    /// Seal existing QA/archive evidence (advanced preparation step)
    Seal {
        #[arg(long)]
        version: String,
        #[arg(long)]
        build_number: String,
        #[arg(long, default_value = "build/native-release")]
        output: PathBuf,
    },
    /// Find the next build number from Apple state
    NextBuildNumber,
}
#[derive(Subcommand)]
enum Store {
    /// List or create an app-owned TestFlight group and save its ID
    BetaGroups {
        #[arg(long)]
        create: Option<String>,
        #[arg(long, requires = "create")]
        external: bool,
    },
    /// Check local metadata and screenshots before Apple writes
    Validate,
    /// Release an approved version awaiting manual release
    Publish {
        #[arg(long)]
        confirm: bool,
    },
    /// Set a phased release to ACTIVE, PAUSED, or COMPLETE
    Phased {
        #[arg(value_parser=["ACTIVE","PAUSED","COMPLETE"])]
        state: String,
    },
    /// Read the selected app/release state
    Status,
    /// Upload the preparation's exact IPA and retain its recovery receipt
    Upload,
    /// Wait for the uploaded build to finish Apple processing
    Wait {
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
    },
    /// Assign the processed build to configured beta groups
    Testflight,
    /// Select the processed build and apply store content
    Stage,
    /// Apply metadata and screenshots to the selected store version
    Metadata,
    /// Request App Review for the selected version
    Submit {
        #[arg(long)]
        confirm: bool,
    },
}
#[derive(Subcommand)]
enum Github {
    /// Verify repository protections for native release operations
    Controls {
        #[arg(long, default_value = "xcode-27")]
        runner: String,
    },
    /// Authenticate and download an exact Actions preparation and receipts
    Fetch {
        #[arg(long)]
        run: u64,
        #[arg(long, default_value = "build/native-release")]
        output: PathBuf,
        #[arg(long)]
        github_output: bool,
    },
    /// Configure native environments, secrets, and production approval
    Setup {
        #[arg(long)]
        reviewer: Option<String>,
    },
}

fn cli_version() -> &'static str {
    let distribution = env!("IOS_RELEASE_DISTRIBUTION_VERSION");
    if distribution.is_empty() {
        env!("CARGO_PKG_VERSION")
    } else {
        distribution
    }
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
                &json!({"schema_version":1,"implementation":"rust","experimental":true,"commands":{
                    "doctor":{"description":"Inspect native runtime requirements and optional configured Xcode"},
                    "toolchain":{"description":"Validate Xcode and resolve exact simulators"},
                    "qa":{"description":"Run lint, localization, analysis or configured tests; retain QA evidence"},
                    "test":{"description":"Run configured simulator tests"},
                    "archive":{"description":"Archive and export locally using installed signing assets"},
                    "screenshots":{"description":"Capture app-owned screenshot scenarios using compiled tests"},
                    "screenshots-capture":{"description":"Compatibility alias for screenshots"},
                    "xcresult-report":{"description":"Reconcile exported XCTest summary and cases"}
                    ,"init":{"description":"Discover a native app and create its configuration and pinned GitHub Actions"}
                    ,"auth":{"description":"Save an App Store Connect team API key privately"}
                    ,"signing":{"description":"Manage owned distribution certificates, renewable profiles and encrypted signing assets"}
                    ,"release":{"description":"Prepare and seal QA-verified releases with an exact IPA identity"}
                    ,"store":{"description":"Upload, reconcile processing, stage metadata and request App Review"}
                    ,"status":{"description":"Read production and TestFlight status"}
                    ,"github":{"description":"Configure protected Actions and authenticate prepared releases and recovery receipts"}
                },"apple_store_mutations":true})
            )?
        );
        return Ok(0);
    }
    let Some(command) = cli.command else {
        Cli::command().print_help()?;
        println!();
        return Ok(0);
    };
    if let Commands::Init {
        project,
        workspace,
        scheme,
        team_id,
        repository,
        xcode,
        no_workflows,
        platform_revision,
        tracking,
        non_exempt_encryption,
        runner,
    } = &command
    {
        let options = onboarding::Options {
            project: project.clone(),
            workspace: workspace.clone(),
            scheme: scheme.clone(),
            team: team_id.clone(),
            repository: repository.clone(),
            xcode: xcode.clone(),
            no_workflows: *no_workflows,
            platform_revision: platform_revision.clone(),
            tracking: *tracking,
            non_exempt_encryption: *non_exempt_encryption,
            runner: runner.clone(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&onboarding::init(
                &cli.app_root,
                &options,
                &mut Native,
                cli.plan
            )?)?
        );
        return Ok(0);
    }
    if let Commands::Auth { action } = &command {
        if cli.plan {
            println!(
                "{}",
                json!({"operation":"auth","writes_private_user_credentials":matches!(action,Auth::Login{..}),"apple_mutations":false})
            );
            return Ok(0);
        }
        match action {
            Auth::Login {
                key_id,
                issuer_id,
                key_file,
            } => onboarding::login(key_id, issuer_id, key_file)?,
            Auth::Status => {
                let value = onboarding::local_credentials()?;
                println!(
                    "{}",
                    json!({"key_id":value["key_id"],"issuer_id":value["issuer_id"],"private_key":"saved privately"})
                );
            }
        }
        return Ok(0);
    }
    if matches!(command, Commands::Doctor { xcode: false }) {
        println!(
            "{}",
            json!({"implementation":"rust","version":cli_version(),"cargo_version":env!("CARGO_PKG_VERSION"),"platform_revision":env!("IOS_RELEASE_BUILD_REVISION"),"runtime_dependencies":[],"native_builds_require":"macOS and the configured Xcode","legacy_workflows":"unchanged"})
        );
        return Ok(0);
    }
    if let Commands::XcresultReport { summary, tests } = &command {
        let summary = serde_json::from_slice(&std::fs::read(summary)?)?;
        let tests = serde_json::from_slice(&std::fs::read(tests)?)?;
        let report = results::junit(&summary, &tests)?;
        println!(
            "{}",
            json!({"passed":report.passed,"cases":report.cases,"executed":report.executed,"bootstrap_recoverable":results::bootstrap_failure(&summary,&tests),"junit":report.xml})
        );
        return Ok(0);
    }
    let mut app = App::load(&cli.app_root, &cli.config)?;
    let mut executor = Native;
    match command {
        Commands::Github { action } => match action {
            Github::Controls { runner } => {
                if cli.plan {
                    println!(
                        "{}",
                        json!({"operation":"github-controls","runner":runner,"branch":"main","production_requires_reviewers":true})
                    );
                } else {
                    ios_release_native::github::controls(&app, &runner, &mut executor)?;
                }
            }
            Github::Fetch {
                run,
                output,
                github_output,
            } => {
                if cli.plan {
                    println!(
                        "{}",
                        json!({"operation":"github-fetch","run":run,"output":output,"requires":"exact producer, successful main preparation, source ancestry, attestation, manifest and receipt identity"})
                    );
                } else {
                    let manifest = ios_release_native::github::fetch(
                        &app,
                        run,
                        &app.root.join(output),
                        &mut executor,
                    )?;
                    if github_output {
                        let path = std::env::var("GITHUB_OUTPUT")
                            .context("--github-output requires GITHUB_OUTPUT")?;
                        ios_release_native::github::selection_output(&manifest, Path::new(&path))?;
                    }
                }
            }
            Github::Setup { reviewer } => {
                if cli.plan {
                    println!(
                        "{}",
                        json!({"operation":"github-setup","repository":app.config["repository"],"environments":["native-signing-admin","native-signing","native-app-store","native-production"],"production_reviewers":reviewer,"branch":"main"})
                    );
                } else {
                    ios_release_native::github::setup(&app, reviewer.as_deref(), &mut executor)?;
                }
            }
        },
        Commands::Signing { action } => {
            if cli.plan {
                println!(
                    "{}",
                    json!({"operation":"signing","certificate_policy":"reuse owned valid certificates; renew before expiry; never revoke unrelated certificates","targets":app.config["targets"],"vault":signing::path(&app)?})
                );
                return Ok(0);
            }
            match action {
                Signing::Export { output } => {
                    ios_release_native::signing_inputs::export(&app, &app.root.join(output))?
                }
                Signing::Apply { input } => {
                    ios_release_native::signing_inputs::apply(&app, &app.root.join(input))?
                }
                Signing::Import { p12, password_env } => {
                    signing::import(&app, &p12, &password_env, &mut executor)?
                }
                Signing::Sync { no_project_changes } => {
                    signing::sync(&mut app, &mut Apple::new()?, &mut executor)?;
                    if app.config["schema_version"] == 2 && !no_project_changes {
                        ios_release_native::project::configure(&app)?;
                    }
                }
                Signing::Status => {
                    println!("{}", serde_json::to_string_pretty(&signing::status(&app)?)?)
                }
                Signing::ConfigureProject => ios_release_native::project::configure(&app)?,
            }
        }
        Commands::Release { action } => {
            if cli.plan {
                println!(
                    "{}",
                    json!({"operation":"release","steps":["QA","isolated signing installation","archive and validation","seal exact release identity"],"archive_configuration":app.configuration("archive")})
                );
                return Ok(0);
            }
            match action {
                Release::Prepare {
                    version,
                    build_number,
                    installed_signing,
                } => {
                    let number = if let Some(number) = build_number {
                        number
                    } else {
                        store::next_number(&app, &mut Apple::new()?)?
                    };
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&release::prepare(
                            &mut app,
                            &version,
                            &number,
                            &mut executor,
                            !installed_signing
                        )?)?
                    );
                }
                Release::Seal {
                    version,
                    build_number,
                    output,
                } => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&release::seal(
                            &app,
                            &version,
                            &build_number,
                            &app.root.join(output)
                        )?)?
                    );
                }
                Release::NextBuildNumber => {
                    println!("{}", store::next_number(&app, &mut Apple::new()?)?)
                }
                Release::Build {
                    version,
                    build_number,
                    installed_signing,
                } => release::build(
                    &app,
                    &version,
                    &build_number,
                    &mut executor,
                    !installed_signing,
                )?,
                Release::VerifyArchive {
                    version,
                    build_number,
                } => release::verify_archive(&app, &version, &build_number, &mut executor)?,
                Release::Pack {
                    release: directory,
                    output,
                } => release::pack(&app, &app.root.join(directory), &app.root.join(output))?,
            }
        }
        Commands::Store {
            action,
            release: directory,
        } => {
            if cli.plan {
                println!(
                    "{}",
                    json!({"operation":"store","release":directory,"apple_mutations":!matches!(action,Store::Status|Store::Validate|Store::BetaGroups{create:None,..}|Store::Wait{..}),"submission_requires_confirmation":matches!(action,Store::Submit{..}),"bundle_id":app.config["app_store"]["bundle_id"]})
                );
                return Ok(0);
            }
            if matches!(action, Store::Validate) {
                let locales = metadata::preflight(&app)?
                    .into_iter()
                    .map(|(locale, _, _)| locale)
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    json!({"valid":true,"locales":locales,"apple_mutations":false})
                );
                return Ok(0);
            }
            let mut api = Apple::new()?;
            if let Store::BetaGroups { create, external } = action {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&store::beta_groups(
                        &mut app,
                        &mut api,
                        create.as_deref(),
                        external
                    )?)?
                );
                return Ok(0);
            }
            if matches!(action, Store::Status) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&store::status(&app, &mut api)?)?
                );
                return Ok(0);
            }
            let mut release = store::Release::load(&app, &app.root.join(directory))?;
            match action {
                Store::Publish { confirm } => {
                    store::publish(&app, &mut release, &mut api, confirm)?
                }
                Store::Phased { state } => store::phased(&app, &mut release, &mut api, &state)?,
                Store::Upload => store::upload(&app, &mut release, &mut api)?,
                Store::Wait { timeout } => {
                    store::wait(&app, &mut release, &mut api, timeout)?;
                }
                Store::Testflight => store::testflight(&app, &mut release, &mut api)?,
                Store::Stage => {
                    store::stage(&app, &mut release, &mut api)?;
                }
                Store::Metadata => metadata::sync(&app, &mut release, &mut api)?,
                Store::Submit { confirm } => store::submit(&app, &mut release, &mut api, confirm)?,
                Store::Status | Store::Validate | Store::BetaGroups { .. } => unreachable!(),
            }
            println!("{}", serde_json::to_string_pretty(&release.receipt)?);
        }
        Commands::Status => {
            if cli.plan {
                println!("{}", json!({"operation":"status","apple_mutations":false}));
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&store::status(&app, &mut Apple::new()?)?)?
                );
            }
        }
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
            isolated_cache,
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
                isolated_cache,
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
        Commands::XcresultReport { .. } | Commands::Init { .. } | Commands::Auth { .. } => {
            unreachable!()
        }
    }
    Ok(0)
}

fn run_qa(plan: bool, app: &App, check: &str, executor: &mut Native) -> Result<i32> {
    if check == "all" {
        let checks = app.qa_checks()?;
        for name in checks {
            let status = run_qa(plan, app, &name, executor)?;
            if status != 0 {
                return Ok(status);
            }
        }
        return Ok(0);
    }
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
