use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_key_value_args, parse_port_spec,
    parse_u32_arg,
};
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
    display: PortDisplayConfig,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone)]
struct RoutePortBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

struct RouteSettings {
    session: SessionSpec,
    pipeline: PipelineSpec,
    template_name: Option<String>,
}

struct RouteRunResult {
    logging_enabled: bool,
    log_path: PathBuf,
}

#[derive(Debug, Clone)]
struct RouteTemplateConfig {
    id: String,
    description: Option<String>,
    pipelines: PipelineSpec,
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
        print_available_templates(bin_name);
        return ExitCode::SUCCESS;
    }

    match run_with_options(cli_options) {
        Ok(result) => {
            if result.logging_enabled {
                println!("log saved to {}", result.log_path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: RouteCliOptions) -> Result<RouteRunResult, String> {
    let settings = build_settings(cli_options)?;
    let mut engine = PipelineEngine::new(&settings.pipeline)?;
    let logging_enabled = settings.session.logging_enabled;
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
    header_lines.push(format!(
        "pipelines: {}",
        settings
            .pipeline
            .pipelines
            .iter()
            .map(|pipeline| pipeline.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if logging_enabled {
        header_lines.push(format!("log: {log_path_display}"));
    } else {
        header_lines.push(String::from("log: disabled (--no-log)"));
    }
    header_lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
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
    )?;
    Ok(RouteRunResult {
        logging_enabled,
        log_path,
    })
}

fn build_settings(cli_options: RouteCliOptions) -> Result<RouteSettings, String> {
    let template_name = cli_options.template;
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let display = cli_options.display;
    let inputs = normalize_inputs(&cli_options.inputs, default_baud, &display)?;
    let outputs = normalize_outputs(&cli_options.outputs, default_baud, &display)?;
    let pipeline = normalize_pipeline_spec(
        resolve_pipeline_spec(template_name.as_deref(), &inputs, &outputs)?,
        &inputs,
        &outputs,
    )?;
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);
    let s3b = cli_options.s3b;

    Ok(RouteSettings {
        session: SessionSpec {
            title: String::from("acs route"),
            command_name: String::from("route"),
            log_dir,
            logging_enabled: !cli_options.no_log,
            xbee_s3b_recovery: s3b,
            inputs,
            outputs,
        },
        pipeline,
        template_name,
    })
}

fn normalize_inputs(
    bindings: &[RoutePortBinding],
    default_baud: u32,
    display: &PortDisplayConfig,
) -> Result<Vec<SessionInputSpec>, String> {
    if bindings.is_empty() {
        return Err(String::from("route requires at least one input port"));
    }

    let mut resolved = Vec::new();
    for binding in bindings {
        if resolved
            .iter()
            .any(|input: &SessionInputSpec| input.id == binding.id)
        {
            return Err(format!("duplicate route input id: {}", binding.id));
        }
        let port = serial::resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?;
        resolved.push(SessionInputSpec {
            id: binding.id.clone(),
            display_mode: binding.display_mode.unwrap_or(display.resolve_input(&port)),
            line_break_mode: binding
                .line_break_mode
                .unwrap_or(display.resolve_line_break_input(&port)),
            port,
            baud_rate: binding.baud.unwrap_or(default_baud),
        });
    }

    Ok(resolved)
}

fn normalize_outputs(
    bindings: &[RoutePortBinding],
    default_baud: u32,
    display: &PortDisplayConfig,
) -> Result<Vec<SessionOutputSpec>, String> {
    if bindings.is_empty() {
        return Err(String::from("route requires at least one output port"));
    }

    let mut resolved = Vec::new();
    for binding in bindings {
        if resolved
            .iter()
            .any(|output: &SessionOutputSpec| output.id == binding.id)
        {
            return Err(format!("duplicate route output id: {}", binding.id));
        }
        let port = serial::resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?;
        resolved.push(SessionOutputSpec {
            id: binding.id.clone(),
            display_mode: binding
                .display_mode
                .unwrap_or(display.resolve_output(&port)),
            format_name: String::from("bytes"),
            port,
            baud_rate: binding.baud.unwrap_or(default_baud),
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
    template_name: Option<&str>,
    inputs: &[SessionInputSpec],
    outputs: &[SessionOutputSpec],
) -> Result<PipelineSpec, String> {
    let Some(template_name) = template_name else {
        return Ok(PipelineSpec::default());
    };

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
            "不明なルートテンプレート `{template_name}` です。利用可能なテンプレートは `acs route --list-templates` で確認できます"
        )),
    }
}

fn print_available_templates(bin_name: &str) {
    println!("利用可能なルートテンプレート:");
    for template in builtin_route_templates() {
        println!(
            "  {:<16} {}",
            template.id,
            template
                .description
                .unwrap_or_else(|| String::from("組み込みテンプレート"))
        );
    }
    println!();
    println!("例:");
    println!(
        "  {bin_name} route merge -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_main=/dev/ttyUSB2"
    );
    println!(
        "  {bin_name} route one-to-one -i in_a=/dev/ttyUSB0 -i in_b=/dev/ttyUSB1 -o out_a=/dev/ttyUSB2 -o out_b=/dev/ttyUSB3"
    );
}

fn builtin_route_templates() -> Vec<RouteTemplateConfig> {
    vec![
        builtin_route_template(
            "merge",
            "複数入力を来た順にそのまま全出力へ流します。出力が1つなら単純マージです。",
            TransformModuleConfig::Identity,
            RouterModuleConfig::Broadcast {
                outputs: Vec::new(),
            },
        ),
        RouteTemplateConfig {
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
) -> RouteTemplateConfig {
    RouteTemplateConfig {
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
) -> RouteTemplateConfig {
    let routes = inputs
        .iter()
        .zip(outputs.iter())
        .map(|(input, output)| (input.id.clone(), vec![output.id.clone()]))
        .collect::<BTreeMap<_, _>>();

    RouteTemplateConfig {
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
        RouterModuleConfig::Broadcast { outputs } if outputs.is_empty() => {
            *outputs = output_ids.to_vec();
        }
        _ => {}
    }
}

fn router_output_ids(router: &RouterModuleConfig) -> Vec<String> {
    match router {
        RouterModuleConfig::Broadcast { outputs } => outputs.clone(),
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
            "--config" => {
                apply_route_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
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
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.display);
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            "--s3b" => options.s3b = true,
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

fn apply_route_config_args(options: &mut RouteCliOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "TEMPLATE" => options.template = Some(assignment.value),
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown route config key: {other}")),
        }
    }

    Ok(())
}

fn parse_route_port_binding(value: &str) -> Result<RoutePortBinding, String> {
    if let Some((id, port)) = value.split_once('=') {
        if id.is_empty() || port.is_empty() {
            return Err(format!("invalid route port binding: {value}"));
        }
        let port_spec = parse_port_spec("route port binding", port)?;
        return Ok(RoutePortBinding {
            id: id.to_owned(),
            port: port_spec.port,
            baud: port_spec.baud,
            display_mode: port_spec.display_mode,
            line_break_mode: port_spec.line_break_mode,
        });
    }

    if value.is_empty() {
        return Err(String::from("route port binding must not be empty"));
    }

    let port_spec = parse_port_spec("route port binding", value)?;

    Ok(RoutePortBinding {
        id: port_spec.port.clone(),
        port: port_spec.port,
        baud: port_spec.baud,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_pipeline_spec, parse_route_args, resolve_pipeline_spec};
    use crate::pipeline::{
        ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec,
        RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
    };
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use crate::session::runtime::{SessionInputSpec, SessionOutputSpec};

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
    fn parse_route_args_accepts_config_and_no_log() {
        let options = parse_route_args(vec![
            String::from("--config"),
            String::from(
                "TEMPLATE=one-to-one,DISPLAY=input:default=utf8+packet,LOG_DIR=tmp/route-logs",
            ),
            String::from("-i"),
            String::from("in_a=/dev/ttyUSB0@921600"),
            String::from("-o"),
            String::from("out_a=/dev/ttyUSB1@115200"),
            String::from("--no-log"),
        ])
        .unwrap();

        assert_eq!(options.template.as_deref(), Some("one-to-one"));
        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.outputs.len(), 1);
        assert_eq!(
            options.log_dir,
            Some(std::path::PathBuf::from("tmp/route-logs"))
        );
        assert!(options.no_log);
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
                    line_break_mode: LineBreakMode::Line,
                },
                SessionInputSpec {
                    id: String::from("in_b"),
                    port: String::from("/dev/ttyUSB1"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                    line_break_mode: LineBreakMode::Line,
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
    fn builtin_one_to_one_pairs_inputs_and_outputs_in_order() {
        let resolved = resolve_pipeline_spec(
            Some("one-to-one"),
            &[
                SessionInputSpec {
                    id: String::from("in_a"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                    line_break_mode: LineBreakMode::Line,
                },
                SessionInputSpec {
                    id: String::from("in_b"),
                    port: String::from("/dev/ttyUSB1"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                    line_break_mode: LineBreakMode::Line,
                },
                SessionInputSpec {
                    id: String::from("in_c"),
                    port: String::from("/dev/ttyUSB2"),
                    baud_rate: 115200,
                    display_mode: PortDisplayMode::HexUtf8,
                    line_break_mode: LineBreakMode::Line,
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
