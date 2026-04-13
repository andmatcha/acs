use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_u32_arg,
};
use crate::common::extend_unique_strings;
use super::config;
use super::help::{is_help_flag, print_route_help};
use super::signal;
use crate::pipeline::{
    FilterModuleConfig, PipelineDefinition, PipelineEngine, PipelineSpec, RouterModuleConfig,
    TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct RouteCliOptions {
    inputs: Vec<RoutePortBinding>,
    outputs: Vec<RoutePortBinding>,
    baud: Option<u32>,
    raw: bool,
    display: PortDisplayConfig,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct RoutePortBinding {
    id: String,
    port: String,
}

struct RouteSettings {
    session: SessionSpec,
    pipeline: PipelineSpec,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_route_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_route_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_route_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(log_path) => {
            println!("log saved to {}", log_path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: RouteCliOptions) -> Result<PathBuf, String> {
    let file_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, file_config)?;
    let mut engine = PipelineEngine::new(&settings.pipeline)?;
    let mut session = SessionRuntime::new(settings.session)?;

    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let input_summary = settings
        .pipeline
        .pipelines
        .iter()
        .flat_map(|pipeline| pipeline.inputs.iter().cloned())
        .collect::<Vec<_>>();
    let output_summary = collect_pipeline_outputs(&settings.pipeline);
    session.set_header_lines(vec![
        format!("inputs: {}", input_summary.join(", ")),
        format!("outputs: {}", output_summary.join(", ")),
        format!(
            "pipelines: {}",
            settings
                .pipeline
                .pipelines
                .iter()
                .map(|pipeline| pipeline.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);

    signal::install_handler();
    session.run_loop(WAIT_INTERVAL, signal::is_stop_requested, |frame, session| {
        for dispatch in engine.process_frame(frame)? {
            session.write_output(&dispatch.output_id, &dispatch.bytes)?;
        }
        Ok(())
    })
}

fn build_settings(
    cli_options: RouteCliOptions,
    file_config: config::AppConfig,
) -> Result<RouteSettings, String> {
    let route_config = file_config.route;
    let default_baud = cli_options
        .baud
        .or(route_config.baud)
        .unwrap_or_else(default_baud_rate);
    let raw = cli_options.raw || route_config.raw.unwrap_or(false);
    let mut display = route_config.display;
    display.merge_from(cli_options.display);
    let log_dir = cli_options
        .log_dir
        .or(route_config.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

    let inputs = normalize_inputs(&route_config.inputs, &cli_options.inputs, default_baud, &display)?;
    let outputs = normalize_outputs(&route_config.outputs, &cli_options.outputs, default_baud, &display)?;
    let pipeline = normalize_pipeline_spec(route_config.pipelines, &inputs, &outputs)?;

    Ok(RouteSettings {
        session: SessionSpec {
            title: String::from("acs route"),
        command_name: String::from("route"),
        raw_input: raw,
        log_dir,
        inputs,
        outputs,
    },
        pipeline,
    })
}

fn normalize_inputs(
    config_inputs: &[config::RouteInputConfig],
    cli_inputs: &[RoutePortBinding],
    default_baud: u32,
    display: &PortDisplayConfig,
) -> Result<Vec<SessionInputSpec>, String> {
    let mut merged = config_inputs
        .iter()
        .map(|input| {
            (
                input.id.clone(),
                input.port.clone(),
                input.baud.unwrap_or(default_baud),
            )
        })
        .collect::<Vec<_>>();

    for binding in cli_inputs {
        if let Some(existing) = merged.iter_mut().find(|(id, _, _)| id == &binding.id) {
            existing.1 = binding.port.clone();
            existing.2 = default_baud;
        } else {
            merged.push((binding.id.clone(), binding.port.clone(), default_baud));
        }
    }

    if merged.is_empty() {
        return Err(String::from("route requires at least one input port"));
    }

    let mut resolved = Vec::new();
    for (id, port, baud_rate) in merged {
        if resolved.iter().any(|input: &SessionInputSpec| input.id == id) {
            return Err(format!("duplicate route input id: {id}"));
        }
        let port = serial::resolve_port(Some(&port)).map_err(|error| error.to_string())?;
        resolved.push(SessionInputSpec {
            id,
            display_mode: display.resolve_input(&port),
            port,
            baud_rate,
        });
    }

    Ok(resolved)
}

fn normalize_outputs(
    config_outputs: &[config::RouteOutputConfig],
    cli_outputs: &[RoutePortBinding],
    default_baud: u32,
    display: &PortDisplayConfig,
) -> Result<Vec<SessionOutputSpec>, String> {
    let mut merged = config_outputs
        .iter()
        .map(|output| {
            (
                output.id.clone(),
                output.port.clone(),
                output.baud.unwrap_or(default_baud),
            )
        })
        .collect::<Vec<_>>();

    for binding in cli_outputs {
        if let Some(existing) = merged.iter_mut().find(|(id, _, _)| id == &binding.id) {
            existing.1 = binding.port.clone();
            existing.2 = default_baud;
        } else {
            merged.push((binding.id.clone(), binding.port.clone(), default_baud));
        }
    }

    if merged.is_empty() {
        return Err(String::from("route requires at least one output port"));
    }

    let mut resolved = Vec::new();
    for (id, port, baud_rate) in merged {
        if resolved.iter().any(|output: &SessionOutputSpec| output.id == id) {
            return Err(format!("duplicate route output id: {id}"));
        }
        let port = serial::resolve_port(Some(&port)).map_err(|error| error.to_string())?;
        resolved.push(SessionOutputSpec {
            id,
            display_mode: display.resolve_output(&port),
            format_name: String::from("bytes"),
            port,
            baud_rate,
        });
    }

    Ok(resolved)
}

fn normalize_pipeline_spec(
    mut pipeline: PipelineSpec,
    inputs: &[SessionInputSpec],
    outputs: &[SessionOutputSpec],
) -> Result<PipelineSpec, String> {
    let input_ids = inputs.iter().map(|input| input.id.clone()).collect::<Vec<_>>();
    let output_ids = outputs
        .iter()
        .map(|output| output.id.clone())
        .collect::<Vec<_>>();

    if pipeline.pipelines.is_empty() {
        pipeline.pipelines.push(PipelineDefinition {
            id: String::from("default"),
            inputs: input_ids.clone(),
            filter: FilterModuleConfig::AllowAll,
            transform: TransformChainConfig {
                modules: vec![TransformModuleConfig::Identity],
            },
            classify: Default::default(),
            router: RouterModuleConfig::Broadcast {
                outputs: output_ids.clone(),
            },
        });
    }

    for definition in &mut pipeline.pipelines {
        if definition.inputs.is_empty() {
            definition.inputs = input_ids.clone();
        }
    }

    for definition in &pipeline.pipelines {
        for input_id in &definition.inputs {
            if !input_ids.iter().any(|known| known == input_id) {
                return Err(format!(
                    "pipeline `{}` references unknown input id `{input_id}`",
                    definition.id
                ));
            }
        }

        for output_id in router_output_ids(&definition.router) {
            if !output_ids.iter().any(|known| known == &output_id) {
                return Err(format!(
                    "pipeline `{}` references unknown output id `{output_id}`",
                    definition.id
                ));
            }
        }
    }

    Ok(pipeline)
}

fn router_output_ids(router: &RouterModuleConfig) -> Vec<String> {
    match router {
        RouterModuleConfig::Broadcast { outputs }
        | RouterModuleConfig::RoundRobin { outputs } => outputs.clone(),
        RouterModuleConfig::SourceMap {
            routes,
            default_outputs,
        } => {
            let mut outputs = default_outputs.clone();
            for route_outputs in routes.values() {
                extend_unique_strings(&mut outputs, route_outputs);
            }
            outputs
        }
        RouterModuleConfig::TagBased {
            routes,
            default_outputs,
        } => {
            let mut outputs = default_outputs.clone();
            for rule in routes {
                extend_unique_strings(&mut outputs, &rule.outputs);
            }
            outputs
        }
    }
}

fn collect_pipeline_outputs(pipeline: &PipelineSpec) -> Vec<String> {
    let mut outputs = Vec::new();
    for definition in &pipeline.pipelines {
        extend_unique_strings(&mut outputs, &router_output_ids(&definition.router));
    }
    outputs
}

fn parse_route_args(args: Vec<String>) -> Result<RouteCliOptions, String> {
    let mut options = RouteCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input-port" | "-i" => options
                .inputs
                .push(parse_route_port_binding(&next_value(&mut iter, "--input-port")?)?),
            "--output-port" | "-o" => options
                .outputs
                .push(parse_route_port_binding(&next_value(&mut iter, "--output-port")?)?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--raw" => options.raw = true,
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                options.display.set_for_stream(
                    assignment.stream,
                    assignment.target,
                    assignment.mode,
                );
            }
            "--config" => {
                options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?))
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            other => return Err(format!("unknown option for route: {other}")),
        }
    }

    Ok(options)
}

fn parse_route_port_binding(value: &str) -> Result<RoutePortBinding, String> {
    if let Some((id, port)) = value.split_once('=') {
        if id.is_empty() || port.is_empty() {
            return Err(format!("invalid route port binding: {value}"));
        }
        return Ok(RoutePortBinding {
            id: id.to_owned(),
            port: port.to_owned(),
        });
    }

    if value.is_empty() {
        return Err(String::from("route port binding must not be empty"));
    }

    Ok(RoutePortBinding {
        id: value.to_owned(),
        port: value.to_owned(),
    })
}
