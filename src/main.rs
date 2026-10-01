use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;
use serde_json::json;
use zixcel_local_inference::{
    CatalogStore, RuntimeRouteRegistry, Selection, install_from_file, installation_statuses,
    installed_models, remove_installation,
};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            let value = json!({
                "schema": "zixcel://local-inference/cli-error/v2",
                "status": "error",
                "reasonCode": code
            });
            match serde_json::to_string_pretty(&value) {
                Ok(encoded) => eprintln!("{encoded}"),
                Err(_) => eprintln!("zixcel-local-inference-cli-error"),
            }
            ExitCode::from(2)
        }
    }
}

fn run(arguments: &[String]) -> Result<(), &'static str> {
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err("usage-invalid");
    };
    if matches!(command, "help" | "--help" | "-h") {
        print_help();
        return Ok(());
    }
    if matches!(command, "version" | "--version" | "-V") {
        println!("zixcel-local-inference {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if command == "runtime-child" {
        return zixcel_local_inference::process::child(&arguments[1..]).map_err(|e| e.code());
    }
    let parsed = ParsedArguments::parse(&arguments[1..])?;
    match command {
        "runtime-serve" => {
            parsed.require_shape(0, &["state"], &[])?;
            zixcel_local_inference::process::serve(Path::new(parsed.require("state")?))
                .map_err(|e| e.code())
        }
        "runtime-supervise" => {
            parsed.require_shape(0, &["state", "runtime"], &[])?;
            zixcel_local_inference::process::supervise(
                Path::new(parsed.require("state")?),
                parsed.optional("runtime"),
            )
            .map_err(|e| e.code())
        }
        "registry" => registry_command(&parsed),
        "provision" => provision(&parsed),
        "source-add-file" => source_add_file(&parsed),
        "source-add-remote" => source_add_remote(&parsed),
        "source-list" => source_list(&parsed),
        "source-request" => source_request(&parsed),
        "refresh" => refresh(&parsed),
        "candidates" => candidates(&parsed),
        "inspect" => inspect(&parsed),
        "plan" => plan(&parsed),
        "install" => install(&parsed),
        "status" => status(&parsed),
        "installed" => installed(&parsed),
        "remove" => remove(&parsed),
        "route-add" => runtime_add(&parsed),
        "route-list" => runtime_list(&parsed),
        _ => Err("usage-invalid"),
    }
}

fn registry_command(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state", "request"], &[])?;
    let path = Path::new(arguments.require("request")?);
    let command =
        zixcel_local_inference::RegistryCommand::from_file(path).map_err(|error| error.code())?;
    let reply =
        zixcel_local_inference::manage_registry(Path::new(arguments.require("state")?), command)
            .map_err(|error| error.code())?;
    print_json(&reply)
}

fn provision(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state"], &[])?;
    let path = Path::new(arguments.require("state")?);
    CatalogStore::provision(path).map_err(|error| error.code())?;
    RuntimeRouteRegistry::provision(path).map_err(|error| error.code())?;
    print_json(&json!({ "state": "provisioned", "modelSelected": false }))
}

fn source_add_file(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(
        1,
        &["state", "catalog", "key-id", "public-key-file", "priority"],
        &[],
    )?;
    let store = store(arguments)?;
    let public_key = read_public_key(arguments.require("public-key-file")?)?;
    let source = store
        .add_file_source(
            &arguments.positionals[0],
            parse_priority(arguments.optional("priority"))?,
            Path::new(arguments.require("catalog")?),
            arguments.require("key-id")?,
            &public_key,
        )
        .map_err(|error| error.code())?;
    print_json(&source)
}

fn source_add_remote(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(
        1,
        &["state", "endpoint", "key-id", "public-key-file", "priority"],
        &[],
    )?;
    let store = store(arguments)?;
    let public_key = read_public_key(arguments.require("public-key-file")?)?;
    let source = store
        .add_remote_source(
            &arguments.positionals[0],
            parse_priority(arguments.optional("priority"))?,
            arguments.require("endpoint")?,
            arguments.require("key-id")?,
            &public_key,
        )
        .map_err(|error| error.code())?;
    print_json(&source)
}

fn source_list(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state"], &[])?;
    print_json(&store(arguments)?.sources().map_err(|error| error.code())?)
}

fn source_request(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["state"], &[])?;
    print_json(
        &store(arguments)?
            .delivery_request(&arguments.positionals[0])
            .map_err(|error| error.code())?,
    )
}

fn refresh(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["state", "from"], &[])?;
    let delivered = arguments.optional("from").map(Path::new);
    print_json(
        &store(arguments)?
            .refresh(&arguments.positionals[0], delivered)
            .map_err(|error| error.code())?,
    )
}

fn candidates(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state"], &["json"])?;
    let values = store(arguments)?
        .candidates()
        .map_err(|error| error.code())?;
    if arguments.switch("json") {
        return print_json(&json!({
            "schema": "zixcel://local-inference/candidate-list/v2",
            "candidates": values,
            "containsExecutableCode": false
        }));
    }
    println!("RANK  MODEL ID                         LICENSE      APPROX Q4   STATE");
    for value in values {
        println!(
            "{:<5} {:<32} {:<12} {:>7} MB   {}",
            value.recommendation_rank,
            value.id,
            value.license_spdx,
            value.estimated_q4_bytes / 1_000_000,
            value.release_state
        );
        println!("      capabilities: {}", value.capabilities.join(", "));
    }
    Ok(())
}

fn inspect(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["state"], &[])?;
    print_json(
        &store(arguments)?
            .model(&arguments.positionals[0])
            .map_err(|error| error.code())?,
    )
}

fn plan(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["state", "root", "runtime", "format"], &[])?;
    let selection = selection(arguments)?;
    print_json(
        &store(arguments)?
            .acquisition_plan(
                &arguments.positionals[0],
                Path::new(arguments.require("root")?),
                &selection,
            )
            .map_err(|error| error.code())?,
    )
}

fn install(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(
        1,
        &["state", "root", "runtime", "format", "artifact-file"],
        &[],
    )?;
    let selection = selection(arguments)?;
    print_json(
        &install_from_file(
            &store(arguments)?,
            &arguments.positionals[0],
            Path::new(arguments.require("root")?),
            &selection,
            Path::new(arguments.require("artifact-file")?),
        )
        .map_err(|error| error.code())?,
    )
}

fn status(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["state", "root"], &[])?;
    print_json(
        &installation_statuses(
            &store(arguments)?,
            Path::new(arguments.require("root")?),
            &arguments.positionals[0],
        )
        .map_err(|error| error.code())?,
    )
}

fn installed(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state", "root"], &[])?;
    print_json(
        &installed_models(&store(arguments)?, Path::new(arguments.require("root")?))
            .map_err(|error| error.code())?,
    )
}

fn remove(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(1, &["root", "release", "confirm"], &[])?;
    remove_installation(
        Path::new(arguments.require("root")?),
        &arguments.positionals[0],
        arguments.require("release")?,
        arguments.require("confirm")?,
    )
    .map_err(|error| error.code())?;
    print_json(&json!({
        "schema": "zixcel://local-inference/removal-receipt/v1",
        "modelId": arguments.positionals[0],
        "releaseId": arguments.require("release")?,
        "state": "removed"
    }))
}

fn runtime_add(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(
        1,
        &["state", "engine", "protocol", "endpoint", "capabilities"],
        &[],
    )?;
    let capabilities = arguments
        .require("capabilities")?
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect();
    print_json(
        &RuntimeRouteRegistry::open_existing(Path::new(arguments.require("state")?))
            .map_err(|error| error.code())?
            .add(
                &arguments.positionals[0],
                arguments.require("engine")?,
                arguments.require("protocol")?,
                arguments.require("endpoint")?,
                capabilities,
            )
            .map_err(|error| error.code())?,
    )
}

fn runtime_list(arguments: &ParsedArguments) -> Result<(), &'static str> {
    arguments.require_shape(0, &["state"], &[])?;
    print_json(
        &RuntimeRouteRegistry::open_existing(Path::new(arguments.require("state")?))
            .map_err(|error| error.code())?
            .routes()
            .map_err(|error| error.code())?,
    )
}

fn store(arguments: &ParsedArguments) -> Result<CatalogStore, &'static str> {
    CatalogStore::open_existing(Path::new(arguments.require("state")?))
        .map_err(|error| error.code())
}

fn selection(arguments: &ParsedArguments) -> Result<Selection, &'static str> {
    let runtime_engine = if let Some(runtime_id) = arguments.optional("runtime") {
        Some(
            RuntimeRouteRegistry::open_existing(Path::new(arguments.require("state")?))
                .map_err(|error| error.code())?
                .route(runtime_id)
                .map_err(|error| error.code())?
                .engine,
        )
    } else {
        None
    };
    Ok(Selection {
        format: arguments.optional("format").map(str::to_owned),
        runtime_engine,
    })
}

fn read_public_key(value: &str) -> Result<String, &'static str> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("public-key-file-invalid");
    }
    fs::read_to_string(path)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "public-key-file-invalid")
}

fn parse_priority(value: Option<&str>) -> Result<u16, &'static str> {
    value
        .unwrap_or("100")
        .parse()
        .map_err(|_| "priority-invalid")
}

fn print_json(value: &impl Serialize) -> Result<(), &'static str> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|_| "json-serialization-failed")?
    );
    Ok(())
}

fn print_help() {
    println!(
        "zixcel-local-inference\n\n\
         Exact admission (explicit owner JSON request, no inference):\n  \
         registry --state ABS --request ABS\n\
         runtime-serve --state ABS (explicit foreground process owner; Linux only)\n  \
         runtime-supervise --state ABS [--runtime REF] (start an exact admitted runtime under the foreground owner)\n\n\
         Explicit storage provisioning:\n  \
         provision --state ABS\n\n\
         Catalog sources:\n  \
         source-add-file ID --state ABS --catalog ABS --key-id ID --public-key-file ABS [--priority N]\n  \
         source-add-remote ID --state ABS --endpoint URL --key-id ID --public-key-file ABS [--priority N]\n  \
         source-list --state ABS\n  \
         source-request ID --state ABS\n  \
         refresh ID --state ABS [--from CROWSI_DELIVERED_FILE]\n\n\
         Models:\n  \
         candidates --state ABS [--json]\n  \
         inspect MODEL --state ABS\n  \
         plan MODEL --state ABS --root ABS [--runtime ID] [--format FORMAT]\n  \
         install MODEL --state ABS --root ABS --artifact-file ABS [--runtime ID] [--format FORMAT]\n  \
         status MODEL --state ABS --root ABS\n  \
         installed --state ABS --root ABS\n  \
         remove MODEL --root ABS --release ID --confirm MODEL@ID\n\n\
         Runtime routes:\n  \
         route-add ID --state ABS --engine ID --protocol openai-compatible|zixcel-inference-v1 \\\n+           --endpoint URL --capabilities A,B\n  \
         route-list --state ABS"
    );
}

#[derive(Debug)]
struct ParsedArguments {
    positionals: Vec<String>,
    flags: BTreeMap<String, String>,
    switches: BTreeSet<String>,
}

impl ParsedArguments {
    fn parse(arguments: &[String]) -> Result<Self, &'static str> {
        let mut positionals = Vec::new();
        let mut flags = BTreeMap::new();
        let mut switches = BTreeSet::new();
        let mut index = 0;
        while index < arguments.len() {
            let value = &arguments[index];
            if let Some(name) = value.strip_prefix("--") {
                if name == "json" {
                    if !switches.insert(name.to_owned()) {
                        return Err("argument-duplicate");
                    }
                    index += 1;
                    continue;
                }
                let next = arguments.get(index + 1).ok_or("argument-value-missing")?;
                if next.starts_with("--") || flags.insert(name.to_owned(), next.clone()).is_some() {
                    return Err("argument-invalid");
                }
                index += 2;
            } else {
                positionals.push(value.clone());
                index += 1;
            }
        }
        Ok(Self {
            positionals,
            flags,
            switches,
        })
    }

    fn require_shape(
        &self,
        positional_count: usize,
        allowed_flags: &[&str],
        allowed_switches: &[&str],
    ) -> Result<(), &'static str> {
        if self.positionals.len() != positional_count
            || self
                .flags
                .keys()
                .any(|key| !allowed_flags.contains(&key.as_str()))
            || self
                .switches
                .iter()
                .any(|key| !allowed_switches.contains(&key.as_str()))
        {
            return Err("usage-invalid");
        }
        Ok(())
    }

    fn require(&self, name: &str) -> Result<&str, &'static str> {
        self.optional(name).ok_or("argument-required")
    }

    fn optional(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.contains(name)
    }
}
