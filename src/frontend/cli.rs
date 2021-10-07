use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{prelude::*, BufReader, Read};
use std::path::PathBuf;
use std::{env, process};

use crate::generate::{
    self,
    changes::{Changes, TOMLEdition},
};
use crate::integrate::{self, DevnetOrchestrator};
use crate::poke::load_session;
use crate::publish::{publish_all_contracts, Network};
use crate::runnner::run_scripts;
use crate::types::{MainConfig, MainConfigFile, RequirementConfig};
use clarity_repl::repl;

use clap::Clap;
use toml;

#[derive(Clap)]
#[clap(version = option_env!("CARGO_PKG_VERSION").expect("Unable to detect version"))]
struct Opts {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Clap)]
enum Command {
    /// Create and scaffold a new project
    #[clap(name = "new")]
    New(GenerateProject),
    /// Contract subcommand
    #[clap(name = "contract")]
    Contract(Contract),
    /// Load contracts in a REPL for interactions
    #[clap(name = "poke")]
    Poke(Poke),
    #[clap(name = "console")]
    Console(Poke),
    /// Execute test suite
    #[clap(name = "test")]
    Test(Test),
    /// Check contracts syntax
    #[clap(name = "check")]
    Check(Check),
    /// Publish contracts on chain
    #[clap(name = "publish")]
    Publish(Publish),
    /// Execute Clarinet Extension
    #[clap(name = "run")]
    Run(Run),
    /// Work on contracts integration
    #[clap(name = "integrate")]
    Integrate(Integrate),
}

#[derive(Clap)]
enum Contract {
    /// New contract subcommand
    #[clap(name = "new")]
    NewContract(NewContract),
    /// Import contract subcommand
    #[clap(name = "requirement")]
    LinkContract(LinkContract),
    /// Fork contract subcommand
    #[clap(name = "fork")]
    ForkContract(ForkContract),
}

#[derive(Clap)]
struct GenerateProject {
    /// Project's name
    pub name: String,
}

#[derive(Clap)]
struct NewContract {
    /// Contract's name
    pub name: String,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
}

#[derive(Clap)]
struct LinkContract {
    /// Contract id
    pub contract_id: String,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
}

#[derive(Clap, Debug)]
struct ForkContract {
    /// Contract id
    pub contract_id: String,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
    // /// Fork contract and all its dependencies
    // #[clap(short = 'r')]
    // pub recursive: bool,
}

#[derive(Clap)]
struct Poke {
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
}

#[derive(Clap)]
struct Integrate {
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
    /// Display streams of logs instead of terminal UI dashboard
    #[clap(long = "no-dashboard")]
    pub no_dashboard: bool,
}

#[derive(Clap)]
struct Test {
    /// Generate coverage
    #[clap(long = "coverage")]
    pub coverage: bool,
    /// Generate costs report
    #[clap(long = "costs")]
    pub costs_report: bool,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
    /// Relaunch tests on updates
    #[clap(long = "watch")]
    pub watch: bool,
    /// Files to includes
    pub files: Vec<String>,
}

#[derive(Clap)]
struct Run {
    /// Script to run
    pub script: String,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
    /// Allow access to wallets
    #[clap(long = "allow-wallets")]
    pub allow_wallets: bool,
    /// Allow write access to disk
    #[clap(long = "allow-write")]
    pub allow_disk_write: bool,
    /// Allow read access to disk
    #[clap(long = "allow-read")]
    pub allow_disk_read: bool,
}

#[derive(Clap)]
struct Publish {
    /// Deploy contracts on devnet, using settings/Devnet.toml
    #[clap(
        long = "devnet",
        conflicts_with = "testnet",
        conflicts_with = "mainnet"
    )]
    pub devnet: bool,
    /// Deploy contracts on testnet, using settings/Testnet.toml
    #[clap(
        long = "testnet",
        conflicts_with = "devnet",
        conflicts_with = "mainnet"
    )]
    pub testnet: bool,
    /// Deploy contracts on mainnet, using settings/Mainnet.toml
    #[clap(
        long = "mainnet",
        conflicts_with = "testnet",
        conflicts_with = "devnet"
    )]
    pub mainnet: bool,
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
}

#[derive(Clap)]
struct Check {
    /// Path to Clarinet.toml
    #[clap(long = "manifest-path")]
    pub manifest_path: Option<String>,
}

pub fn main() {
    let opts: Opts = Opts::parse();

    let hints_enabled = if env::var("CLARINET_DISABLE_HINTS") == Ok("1".into()) {
        false
    } else {
        true
    };

    match opts.command {
        Command::New(project_opts) => {
            let current_path = {
                let current_dir = env::current_dir().expect("Unable to read current directory");
                current_dir.to_str().unwrap().to_owned()
            };

            let changes = generate::get_changes_for_new_project(current_path, project_opts.name);
            execute_changes(changes);
            if hints_enabled {
                display_check_hint();
            }
        }
        Command::Contract(subcommand) => match subcommand {
            Contract::NewContract(new_contract) => {
                let manifest_path = get_manifest_path_or_exit(new_contract.manifest_path);

                let changes = generate::get_changes_for_new_contract(
                    manifest_path,
                    new_contract.name,
                    None,
                    true,
                    vec![],
                );
                execute_changes(changes);
                if hints_enabled {
                    display_check_hint();
                }    
            }
            Contract::LinkContract(required_contract) => {
                let manifest_path = get_manifest_path_or_exit(required_contract.manifest_path);

                let change = TOMLEdition {
                    comment: format!(
                        "Adding {} as a requirement to Clarinet.toml",
                        required_contract.contract_id
                    ),
                    manifest_path,
                    contracts_to_add: HashMap::new(),
                    requirements_to_add: vec![RequirementConfig {
                        contract_id: required_contract.contract_id.clone(),
                    }],
                };
                execute_changes(vec![Changes::EditTOML(change)]);
                if hints_enabled {
                    display_check_hint();
                }    
            }
            Contract::ForkContract(fork_contract) => {
                let manifest_path = get_manifest_path_or_exit(fork_contract.manifest_path);

                println!(
                    "Resolving {} and its dependencies...",
                    fork_contract.contract_id
                );

                let settings = repl::SessionSettings::default();
                let mut session = repl::Session::new(settings);

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .max_blocking_threads(32)
                    .build()
                    .unwrap();

                let res = rt.block_on(session.resolve_link(&repl::settings::InitialLink {
                    contract_id: fork_contract.contract_id.clone(),
                    stacks_node_addr: None,
                    cache: None,
                }));
                let contracts = res.unwrap();
                let mut changes = vec![];
                for (contract_id, code, deps) in contracts.into_iter() {
                    let components: Vec<&str> = contract_id.split('.').collect();
                    let contract_name = components.last().unwrap();

                    if &contract_id == &fork_contract.contract_id {
                        let mut change_set = generate::get_changes_for_new_contract(
                            manifest_path.clone(),
                            contract_name.to_string(),
                            Some(code),
                            false,
                            vec![],
                        );
                        changes.append(&mut change_set);

                        for dep in deps.iter() {
                            let mut change_set = generate::get_changes_for_new_link(
                                manifest_path.clone(),
                                dep.clone(),
                                None,
                            );
                            changes.append(&mut change_set);
                        }
                    }
                }
                execute_changes(changes);
                if hints_enabled {
                    display_check_hint();
                }    
            }
        },
        Command::Poke(cmd) | Command::Console(cmd) => {
            let manifest_path = get_manifest_path_or_exit(cmd.manifest_path);
            let start_repl = true;
            load_session(manifest_path, start_repl, Network::Devnet).expect("Unable to start REPL");
            if hints_enabled {
                display_write_tests_hint();
            }
        }
        Command::Check(cmd) => {
            let manifest_path = get_manifest_path_or_exit(cmd.manifest_path);
            let start_repl = false;
            match load_session(manifest_path, start_repl, Network::Devnet) {
                Err(e) => {
                    println!("{}", e);
                }
                Ok(session) => {
                    println!(
                        "{} Syntax of {} contract(s) successfully checked",
                        green!("✔"),
                        session.settings.initial_contracts.len()
                    );
                }
            }
            if hints_enabled {
                display_write_tests_hint();
            }
        }
        Command::Test(cmd) => {
            let manifest_path = get_manifest_path_or_exit(cmd.manifest_path);
            let start_repl = false;
            let res = load_session(manifest_path.clone(), start_repl, Network::Devnet);
            let session = match res {
                Ok(session) => session,
                Err(e) => {
                    println!("{}", e);
                    return;
                }
            };
            run_scripts(
                cmd.files,
                cmd.coverage,
                cmd.costs_report,
                cmd.watch,
                true,
                false,
                manifest_path,
                Some(session),
            );
            if hints_enabled {
                display_measure_cov_hint();
            }
        }
        Command::Run(cmd) => {
            let manifest_path = get_manifest_path_or_exit(cmd.manifest_path);
            let start_repl = false;
            let res = load_session(manifest_path.clone(), start_repl, Network::Devnet);
            let session = match res {
                Ok(session) => session,
                Err(e) => {
                    println!("{}", e);
                    return;
                }
            };
            run_scripts(
                vec![cmd.script],
                false,
                false,
                false,
                cmd.allow_wallets,
                cmd.allow_disk_write,
                manifest_path,
                Some(session),
            );
        }
        Command::Publish(deploy) => {
            let manifest_path = get_manifest_path_or_exit(deploy.manifest_path);

            let network = if deploy.devnet == true {
                Network::Devnet
            } else if deploy.testnet == true {
                Network::Testnet
            } else if deploy.mainnet == true {
                // Network::Mainnet
                // TODO(ludo): before supporting mainnet deployments, we want to add a pass
                // making sure that addresses are consistent + handle other hard coded flags.
                // Search for "mainnet handling".
                panic!("Target deployment must be specified with --devnet, --testnet,  --mainnet")
            } else {
                panic!("Target deployment must be specified with --devnet, --testnet,  --mainnet")
            };
            match publish_all_contracts(manifest_path, network) {
                Ok(results) => println!("{}", results.join("\n")),
                Err(results) => println!("{}", results.join("\n")),
            };
        }
        Command::Integrate(cmd) => {
            let manifest_path = get_manifest_path_or_exit(cmd.manifest_path);
            let devnet = DevnetOrchestrator::new(manifest_path);
            integrate::run_devnet(devnet, None, !cmd.no_dashboard);
            if hints_enabled {
                display_deploy_hint();
            }
        }
    };
}

fn get_manifest_path_or_exit(path: Option<String>) -> PathBuf {
    println!("");
    if let Some(path) = path {
        let manifest_path = PathBuf::from(path);
        if !manifest_path.exists() {
            println!("Could not find Clarinet.toml");
            process::exit(1);
        }
        manifest_path
    } else {
        let mut current_dir = env::current_dir().unwrap();
        loop {
            current_dir.push("Clarinet.toml");

            if current_dir.exists() {
                break current_dir;
            }
            current_dir.pop();

            if !current_dir.pop() {
                println!("Could not find Clarinet.toml");
                process::exit(1);
            }
        }
    }
}

fn execute_changes(changes: Vec<Changes>) {
    let mut shared_config = None;
    let mut path = PathBuf::new();

    for mut change in changes.into_iter() {
        match change {
            Changes::AddFile(options) => {
                if let Ok(entry) = fs::metadata(&options.path) {
                    if entry.is_file() {
                        println!("File already exists at path {}", options.path);
                        continue;
                    }
                }
                println!("{}", options.comment);
                let mut file = File::create(options.path.clone()).expect("Unable to create file");
                file.write_all(options.content.as_bytes())
                    .expect("Unable to write file");
            }
            Changes::AddDirectory(options) => {
                println!("{}", options.comment);
                fs::create_dir_all(options.path.clone()).expect("Unable to create directory");
            }
            Changes::EditTOML(ref mut options) => {
                let mut config = match shared_config.take() {
                    Some(config) => config,
                    None => {
                        path = options.manifest_path.clone();
                        let file = File::open(path.clone()).unwrap();
                        let mut config_file_reader = BufReader::new(file);
                        let mut config_file = vec![];
                        config_file_reader.read_to_end(&mut config_file).unwrap();
                        let config_file: MainConfigFile =
                            toml::from_slice(&config_file[..]).unwrap();
                        MainConfig::from_config_file(config_file)
                    }
                };

                let mut requirements = match config.project.requirements.take() {
                    Some(requirements) => requirements,
                    None => vec![],
                };
                for requirement in options.requirements_to_add.drain(..) {
                    if !requirements.contains(&requirement) {
                        requirements.push(requirement);
                    }
                }
                config.project.requirements = Some(requirements);

                for (contract_name, contract_config) in options.contracts_to_add.drain() {
                    config.contracts.insert(contract_name, contract_config);
                }

                shared_config = Some(config);
                println!("{}", options.comment);
            }
        }
    }

    if let Some(config) = shared_config {
        let toml = toml::to_string(&config).unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(&toml.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
}

fn display_separator() {
    println!("{}", yellow!("----------------------------"));
}

fn display_hint_header() {
    display_separator();
    println!("{}", yellow!("Hint: what's next?"));
}

fn display_hint_footer() {
    println!("{}", yellow!("Disable these hints with the env var CLARINET_DISABLE_HINTS=1"));
    display_separator();
}

fn display_check_hint() {
    println!("");
    display_hint_header();
    println!("{}", yellow!("Start implementing your contracts with Clarity."));
    println!("{}", yellow!("Documentation: https://docs.stacks.co/write-smart-contracts/hello-world-tutorial#step-3-add-code-to-the-hello-world-contract"));
    println!("{}", yellow!("Then run the command:"));
    println!("{}", yellow!("$ clarinet check"));
    println!("{}", yellow!("to get the results"));
    display_hint_footer();
}

fn display_write_tests_hint() {
    println!("");
    display_hint_header();
    println!("{}", yellow!("Write unit tests for your contracts in Typescript, using the clarinet test framework."));
    println!("{}", yellow!("Documentation: https://docs.stacks.co/write-smart-contracts/clarinet#testing-with-the-test-harness"));
    println!("{}", yellow!("Then run the command:"));
    println!("{}", yellow!("$ clarinet test"));
    println!("{}", yellow!("to get the results"));
    display_hint_footer();
}

fn display_measure_cov_hint() {
    println!("");
    display_separator();
    println!("{}", yellow!("Pro tips:"));
    println!("{}", yellow!("* Try the command `clarinet test --watch` to get live test results, when you save your files"));
    println!("{}", yellow!("* Inspect the costs of your contracts with the option `clarinet test --costs`"));
    println!("{}", yellow!("* Measure test coverage with the command `clarinet test --coverage` and the lcov tooling suite"));
    println!("{}", yellow!("* Once ready, deploy your contracts to your local devnet, using the command `clarinet integrate`"));
    display_hint_footer();
}

fn display_deploy_hint() {
    println!("");
    display_hint_header();
    println!("{}", yellow!("Once timing feels right, run the command"));
    println!("{}", yellow!("$ clarinet deploy --testnet"));
    println!("{}", yellow!("to deploy your contracts to Testnet / Mainnet"));
    display_hint_footer(); 
}
