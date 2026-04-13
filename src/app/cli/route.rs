use super::common::{default_baud_rate, default_log_dir, next_value, parse_u32_arg};
use super::config;
use super::help::{is_help_flag, print_route_help};
use super::signal;
use crate::common::extend_unique_strings;
use crate::pipeline::{
    ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineEngine, PipelineSpec,
    RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct RouteCliOptions {
    inputs: Vec<RoutePortBinding>,
    outputs: Vec<RoutePortBinding>,
    template: Option<String>,
    list_templates: bool,
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
    template_name: Option<String>,
}

#[derive(Debug, Clone)]
struct AvailableRouteTemplate {
    id: String,
    description: String,
    source: &'static str,
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

    if cli_options.list_templates {
        return match print_available_templates(cli_options.config_path.as_deref(), bin_name) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        };
    }

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
    let mut header_lines = vec![
        format!("inputs: {}", input_summary.join(", ")),
        format!("outputs: {}", output_summary.join(", ")),
    ];
    if let Some(template_name) = &settings.template_name {
        header_lines.push(format!("template: {template_name}"));
    }
    header_lines.extend([
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
    session.set_header_lines(header_lines);

    signal::install_handler();
    session.run_loop(
        WAIT_INTERVAL,
        signal::is_stop_requested,
        |frame, session| {
            for dispatch in engine.process_frame(frame)? {
                session.write_output(&dispatch.output_id, &dispatch.bytes)?;
            }
            Ok(())
        },
    )
}

fn build_settings(
    cli_options: RouteCliOptions,
    file_config: config::AppConfig,
) -> Result<RouteSettings, String> {
    let route_config = file_config.route;
    let template_name = cli_options
        .template
        .or_else(|| route_config.template.clone());
    let default_baud = cli_options
        .baud
        .or(route_config.baud)
        .unwrap_or_else(default_baud_rate);
    let raw = cli_options.raw || route_config.raw.unwrap_or(false);
    let mut display = route_config.display.clone();
    display.merge_from(cli_options.display);

    let inputs = normalize_inputs(
        &route_config.inputs,
        &cli_options.inputs,
        default_baud,
        &display,
    )?;
    let outputs = normalize_outputs(
        &route_config.outputs,
        &cli_options.outputs,
        default_baud,
        &display,
    )?;
    let pipeline = normalize_pipeline_spec(
        resolve_pipeline_spec(&route_config, template_name.as_deref(), &inputs, &outputs)?,
        &inputs,
        &outputs,
    )?;
    let log_dir = cli_options
        .log_dir
        .or(route_config.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

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
        template_name,
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
        if resolved
            .iter()
            .any(|input: &SessionInputSpec| input.id == id)
        {
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
        if resolved
            .iter()
            .any(|output: &SessionOutputSpec| output.id == id)
        {
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
    let input_ids = inputs
        .iter()
        .map(|input| input.id.clone())
        .collect::<Vec<_>>();
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
        apply_default_router_outputs(&mut definition.router, &output_ids);
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

fn resolve_pipeline_spec(
    route_config: &config::RouteConfig,
    template_name: Option<&str>,
    inputs: &[SessionInputSpec],
    outputs: &[SessionOutputSpec],
) -> Result<PipelineSpec, String> {
    let Some(template_name) = template_name else {
        return Ok(route_config.pipelines.clone());
    };

    if let Some(template) = route_config.templates.get(template_name) {
        return Ok(template.pipelines.clone());
    }

    match template_name {
        "merge" => Ok(builtin_route_template(
            "merge",
            "複数入力を来た順にそのまま全出力へ流します。出力が1つなら単純マージです。",
            TransformModuleConfig::Identity,
            RouterModuleConfig::Broadcast {
                outputs: Vec::new(),
            },
        )
        .pipelines),
        "one-to-one" => Ok(builtin_one_to_one_template(inputs, outputs).pipelines),
        _ => Err(format!(
            "unknown route template `{template_name}`; use `acs route --list-templates` to inspect available templates"
        )),
    }
}

fn print_available_templates(
    config_path: Option<&std::path::Path>,
    bin_name: &str,
) -> Result<(), String> {
    let file_config = config::load_config_or_default(config_path)?;
    let templates = collect_available_templates(&file_config.route);

    println!("Available route templates:");
    for template in templates.values() {
        println!(
            "  {:<16} [{}] {}",
            template.id, template.source, template.description
        );
    }
    println!();
    println!("Examples:");
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_main=/dev/ttyUSB2"
    );
    println!(
        "  {bin_name} route one-to-one --config {}",
        config::DEFAULT_CONFIG_DIR_NAME
    );

    Ok(())
}

fn collect_available_templates(
    route_config: &config::RouteConfig,
) -> BTreeMap<String, AvailableRouteTemplate> {
    let mut templates = builtin_route_templates()
        .into_iter()
        .map(|template| {
            (
                template.id.clone(),
                AvailableRouteTemplate {
                    id: template.id,
                    description: template
                        .description
                        .unwrap_or_else(|| String::from("built-in template")),
                    source: "built-in",
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for template in route_config.templates.values() {
        templates.insert(
            template.id.clone(),
            AvailableRouteTemplate {
                id: template.id.clone(),
                description: template
                    .description
                    .clone()
                    .unwrap_or_else(|| String::from("config-defined template")),
                source: "config",
            },
        );
    }

    templates
}

fn builtin_route_templates() -> Vec<config::RouteTemplateConfig> {
    vec![
        builtin_route_template(
            "merge",
            "複数入力を来た順にそのまま全出力へ流します。出力が1つなら単純マージです。",
            TransformModuleConfig::Identity,
            RouterModuleConfig::Broadcast {
                outputs: Vec::new(),
            },
        ),
        config::RouteTemplateConfig {
            id: String::from("one-to-one"),
            description: Some(String::from(
                "入力配列順と出力配列順を1対1に対応させ、対応する相手へだけそのまま流します。",
            )),
            pipelines: PipelineSpec::default(),
        },
    ]
}

fn builtin_route_template(
    id: &str,
    description: &str,
    transform: TransformModuleConfig,
    router: RouterModuleConfig,
) -> config::RouteTemplateConfig {
    config::RouteTemplateConfig {
        id: id.to_owned(),
        description: Some(description.to_owned()),
        pipelines: PipelineSpec {
            pipelines: vec![PipelineDefinition {
                id: id.to_owned(),
                inputs: Vec::new(),
                filter: FilterModuleConfig::AllowAll,
                transform: TransformChainConfig {
                    modules: vec![transform],
                },
                classify: ClassifyModuleConfig::None,
                router,
            }],
        },
    }
}

fn builtin_one_to_one_template(
    inputs: &[SessionInputSpec],
    outputs: &[SessionOutputSpec],
) -> config::RouteTemplateConfig {
    let routes = inputs
        .iter()
        .zip(outputs.iter())
        .map(|(input, output)| (input.id.clone(), vec![output.id.clone()]))
        .collect::<BTreeMap<_, _>>();

    config::RouteTemplateConfig {
        id: String::from("one-to-one"),
        description: Some(String::from(
            "入力配列順と出力配列順を1対1に対応させ、対応する相手へだけそのまま流します。",
        )),
        pipelines: PipelineSpec {
            pipelines: vec![PipelineDefinition {
                id: String::from("one-to-one"),
                inputs: inputs.iter().map(|input| input.id.clone()).collect(),
                filter: FilterModuleConfig::AllowAll,
                transform: TransformChainConfig {
                    modules: vec![TransformModuleConfig::Identity],
                },
                classify: ClassifyModuleConfig::None,
                router: RouterModuleConfig::SourceMap {
                    routes,
                    default_outputs: Vec::new(),
                },
            }],
        },
    }
}

fn apply_default_router_outputs(router: &mut RouterModuleConfig, output_ids: &[String]) {
    match router {
        RouterModuleConfig::Broadcast { outputs } | RouterModuleConfig::RoundRobin { outputs }
            if outputs.is_empty() =>
        {
            *outputs = output_ids.to_vec();
        }
        _ => {}
    }
}

fn router_output_ids(router: &RouterModuleConfig) -> Vec<String> {
    match router {
        RouterModuleConfig::Broadcast { outputs } | RouterModuleConfig::RoundRobin { outputs } => {
            outputs.clone()
        }
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
            "--template" => options.template = Some(next_value(&mut iter, "--template")?),
            "--list-templates" => options.list_templates = true,
            "--input-port" | "-i" => options.inputs.push(parse_route_port_binding(&next_value(
                &mut iter,
                "--input-port",
            )?)?),
            "--output-port" | "-o" => options.outputs.push(parse_route_port_binding(&next_value(
                &mut iter,
                "--output-port",
            )?)?),
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
            other if other.starts_with('-') => {
                return Err(format!("unknown option for route: {other}"));
            }
            other => {
                if options.template.is_some() {
                    return Err(format!("unexpected positional argument for route: {other}"));
                }
                options.template = Some(other.to_owned());
            }
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

#[cfg(test)]
mod tests {
    use super::{normalize_pipeline_spec, parse_route_args, resolve_pipeline_spec};
    use crate::app::cli::config::{RouteConfig, RouteTemplateConfig};
    use crate::pipeline::{
        ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec,
        RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
    };
    use crate::port_display::PortDisplayMode;
    use crate::session::runtime::{SessionInputSpec, SessionOutputSpec};
    use std::collections::BTreeMap;

    #[test]
    fn parse_route_args_accepts_positional_template() {
        let options = parse_route_args(vec![
            String::from("merge"),
            String::from("-i"),
            String::from("in_a=/dev/ttyUSB0"),
            String::from("-o"),
            String::from("out_main=/dev/ttyUSB1"),
        ])
        .unwrap();

        assert_eq!(options.template.as_deref(), Some("merge"));
        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.outputs.len(), 1);
    }

    #[test]
    fn normalize_pipeline_spec_fills_missing_inputs_and_outputs() {
        let spec = PipelineSpec {
            pipelines: vec![PipelineDefinition {
                id: String::from("merge"),
                inputs: Vec::new(),
                filter: FilterModuleConfig::AllowAll,
                transform: TransformChainConfig {
                    modules: vec![TransformModuleConfig::Identity],
                },
                classify: ClassifyModuleConfig::None,
                router: RouterModuleConfig::Broadcast {
                    outputs: Vec::new(),
                },
            }],
        };

        let normalized = normalize_pipeline_spec(
            spec,
            &[
                SessionInputSpec {
                    id: String::from("in_a"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                },
                SessionInputSpec {
                    id: String::from("in_b"),
                    port: String::from("/dev/ttyUSB1"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                },
            ],
            &[SessionOutputSpec {
                id: String::from("out_main"),
                port: String::from("/dev/ttyUSB2"),
                baud_rate: 115200,
                format_name: String::from("bytes"),
                display_mode: PortDisplayMode::HexUtf8,
            }],
        )
        .unwrap();

        assert_eq!(normalized.pipelines[0].inputs, vec!["in_a", "in_b"]);
        match &normalized.pipelines[0].router {
            RouterModuleConfig::Broadcast { outputs } => {
                assert_eq!(outputs, &vec![String::from("out_main")]);
            }
            other => panic!("unexpected router: {other:?}"),
        }
    }

    #[test]
    fn config_defined_template_overrides_builtin_template() {
        let mut route_config = RouteConfig::default();
        route_config.templates = BTreeMap::from([(
            String::from("merge"),
            RouteTemplateConfig {
                id: String::from("merge"),
                description: Some(String::from("custom merge")),
                pipelines: PipelineSpec {
                    pipelines: vec![PipelineDefinition {
                        id: String::from("custom_merge"),
                        inputs: Vec::new(),
                        filter: FilterModuleConfig::AllowAll,
                        transform: TransformChainConfig {
                            modules: vec![TransformModuleConfig::JoinLatest {
                                separator: vec![b','],
                                require_all: true,
                            }],
                        },
                        classify: ClassifyModuleConfig::None,
                        router: RouterModuleConfig::Broadcast {
                            outputs: Vec::new(),
                        },
                    }],
                },
            },
        )]);

        let resolved = resolve_pipeline_spec(&route_config, Some("merge"), &[], &[]).unwrap();

        assert_eq!(resolved.pipelines[0].id, "custom_merge");
    }

    #[test]
    fn builtin_one_to_one_pairs_inputs_and_outputs_in_order() {
        let resolved = resolve_pipeline_spec(
            &RouteConfig::default(),
            Some("one-to-one"),
            &[
                SessionInputSpec {
                    id: String::from("in_a"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                },
                SessionInputSpec {
                    id: String::from("in_b"),
                    port: String::from("/dev/ttyUSB1"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                },
                SessionInputSpec {
                    id: String::from("in_c"),
                    port: String::from("/dev/ttyUSB2"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                },
            ],
            &[
                SessionOutputSpec {
                    id: String::from("out_a"),
                    port: String::from("/dev/ttyUSB3"),
                    baud_rate: 115200,
                    format_name: String::from("bytes"),
                    display_mode: PortDisplayMode::HexUtf8,
                },
                SessionOutputSpec {
                    id: String::from("out_b"),
                    port: String::from("/dev/ttyUSB4"),
                    baud_rate: 115200,
                    format_name: String::from("bytes"),
                    display_mode: PortDisplayMode::HexUtf8,
                },
            ],
        )
        .unwrap();

        match &resolved.pipelines[0].router {
            RouterModuleConfig::SourceMap {
                routes,
                default_outputs,
            } => {
                assert_eq!(routes.get("in_a"), Some(&vec![String::from("out_a")]));
                assert_eq!(routes.get("in_b"), Some(&vec![String::from("out_b")]));
                assert!(!routes.contains_key("in_c"));
                assert!(default_outputs.is_empty());
            }
            other => panic!("unexpected router: {other:?}"),
        }
    }
}
