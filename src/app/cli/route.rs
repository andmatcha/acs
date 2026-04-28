use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_key_value_args, parse_port_spec,
    parse_u32_arg,
};
use super::help::{is_help_flag, print_route_help};
use super::io::{
    display_mode_value, format_command_preview, format_input_display_value,
    prompt_display_mode_with_preview, prompt_serial_port_with_preview,
    prompt_u32_choice_with_preview,
};
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
    inputs: Vec<RouteBindingArg>,
    outputs: Vec<RouteBindingArg>,
    template: Option<String>,
    list_templates: bool,
    baud: Option<u32>,
    display: PortDisplayConfig,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Default)]
struct RouteRuntimeOptions {
    inputs: Vec<RoutePortBinding>,
    outputs: Vec<RoutePortBinding>,
    template: Option<String>,
    baud: Option<u32>,
    display: PortDisplayConfig,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteBindingArg {
    Provided(String),
    Prompt,
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
    executed_command: Option<String>,
}

struct RouteRunResult {
    logging_enabled: bool,
    log_path: PathBuf,
    executed_command: Option<String>,
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

    let runtime_options = match resolve_route_options(cli_options) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    match run_with_options(runtime_options) {
        Ok(result) => {
            if result.logging_enabled {
                println!("log saved to {}", result.log_path.display());
            }
            if let Some(command) = &result.executed_command {
                println!("Command:");
                println!("{command}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: RouteRuntimeOptions) -> Result<RouteRunResult, String> {
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
        executed_command: settings.executed_command,
    })
}

fn build_settings(cli_options: RouteRuntimeOptions) -> Result<RouteSettings, String> {
    let template_name = cli_options.template.clone();
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let display = cli_options.display;
    let inputs = normalize_inputs(&cli_options.inputs, default_baud, &display)?;
    let outputs = normalize_outputs(&cli_options.outputs, default_baud, &display)?;
    let pipeline = normalize_pipeline_spec(
        resolve_pipeline_spec(template_name.as_deref(), &inputs, &outputs)?,
        &inputs,
        &outputs,
    )?;
    let explicit_log_dir = cli_options.log_dir.clone();
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);
    let s3b = cli_options.s3b;
    let executed_command = Some(build_route_executed_command(
        template_name.as_deref(),
        &inputs,
        &outputs,
        explicit_log_dir.as_ref(),
        cli_options.no_log,
        s3b,
    ));

    Ok(RouteSettings {
        session: SessionSpec {
            title: executed_command
                .clone()
                .unwrap_or_else(|| String::from("acs route")),
            command_name: String::from("route"),
            log_dir,
            logging_enabled: !cli_options.no_log,
            xbee_s3b_recovery: s3b,
            inputs,
            outputs,
        },
        pipeline,
        template_name,
        executed_command,
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

#[derive(Debug, Clone, Copy)]
struct RoutePromptPortPosition {
    kind: &'static str,
    index: usize,
    total: usize,
}

impl RoutePromptPortPosition {
    fn input(index: usize, total: usize) -> Self {
        Self {
            kind: "入力",
            index,
            total,
        }
    }

    fn output(index: usize, total: usize) -> Self {
        Self {
            kind: "出力",
            index,
            total,
        }
    }

    fn label(self, prompt: &str) -> String {
        if self.total > 1 {
            format!(
                "{prompt} ({}ポート {}/{})",
                self.kind, self.index, self.total
            )
        } else {
            prompt.to_owned()
        }
    }
}

#[derive(Debug, Clone)]
struct RoutePromptCommand {
    template: Option<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, Copy)]
enum RoutePromptBindingPreview<'a> {
    Input(&'a str),
    Output(&'a str),
}

impl RoutePromptCommand {
    fn new(options: &RouteRuntimeOptions) -> Self {
        Self {
            template: options.template.clone(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            log_dir: options.log_dir.clone(),
            no_log: options.no_log,
            s3b: options.s3b,
        }
    }

    fn push_input(&mut self, binding: String) {
        self.inputs.push(binding);
    }

    fn push_output(&mut self, binding: String) {
        self.outputs.push(binding);
    }

    fn render(&self, candidate: Option<RoutePromptBindingPreview<'_>>) -> String {
        let mut args = vec![String::from("acs"), String::from("route")];

        if let Some(template) = &self.template {
            args.push(template.clone());
        }

        for input in &self.inputs {
            args.push(String::from("-i"));
            args.push(input.clone());
        }
        if let Some(RoutePromptBindingPreview::Input(input)) = candidate {
            args.push(String::from("-i"));
            args.push(input.to_owned());
        }

        for output in &self.outputs {
            args.push(String::from("-o"));
            args.push(output.clone());
        }
        if let Some(RoutePromptBindingPreview::Output(output)) = candidate {
            args.push(String::from("-o"));
            args.push(output.to_owned());
        }

        append_route_prompt_common_args(&mut args, self.log_dir.as_ref(), self.no_log, self.s3b);
        format_command_preview(&args)
    }
}

fn resolve_route_options(cli_options: RouteCliOptions) -> Result<RouteRuntimeOptions, String> {
    let total_inputs = cli_options.inputs.len();
    let total_outputs = cli_options.outputs.len();
    let mut input_index = 0;
    let mut output_index = 0;
    let mut runtime_options = RouteRuntimeOptions {
        template: cli_options.template,
        baud: cli_options.baud,
        display: cli_options.display,
        log_dir: cli_options.log_dir,
        no_log: cli_options.no_log,
        s3b: cli_options.s3b,
        ..RouteRuntimeOptions::default()
    };
    let mut prompt_command = RoutePromptCommand::new(&runtime_options);

    for input in cli_options.inputs {
        input_index += 1;
        let position = RoutePromptPortPosition::input(input_index, total_inputs);
        let value = match input {
            RouteBindingArg::Provided(value) => value,
            RouteBindingArg::Prompt => prompt_route_input_binding(&prompt_command, position)?,
        };
        runtime_options
            .inputs
            .push(parse_route_port_binding(&value)?);
        prompt_command.push_input(value);
    }

    for output in cli_options.outputs {
        output_index += 1;
        let position = RoutePromptPortPosition::output(output_index, total_outputs);
        let value = match output {
            RouteBindingArg::Provided(value) => value,
            RouteBindingArg::Prompt => prompt_route_output_binding(&prompt_command, position)?,
        };
        runtime_options
            .outputs
            .push(parse_route_port_binding(&value)?);
        prompt_command.push_output(value);
    }

    Ok(runtime_options)
}

fn prompt_route_input_binding(
    command: &RoutePromptCommand,
    position: RoutePromptPortPosition,
) -> Result<String, String> {
    let id = default_route_input_id(position.index);
    let port_prompt = position.label("入力ポートを選択");
    let port = prompt_serial_port_with_preview(&port_prompt, |port| {
        Some(command.render(Some(RoutePromptBindingPreview::Input(
            &format_prompt_route_port_binding(&id, port, None, None),
        ))))
    })?;
    let baud_prompt = position.label("入力ボーレート");
    let baud = prompt_u32_choice_with_preview(
        &baud_prompt,
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(RoutePromptBindingPreview::Input(
                &format_prompt_route_port_binding(&id, &port, Some(baud), None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let display_prompt = position.label("入力表示形式");
    let display = prompt_display_mode_with_preview(&display_prompt, true, |display| {
        Some(command.render(Some(RoutePromptBindingPreview::Input(
            &format_prompt_route_port_binding(&id, &port, Some(&baud_text), Some(display)),
        ))))
    })?;

    Ok(format_route_port_binding(
        &id,
        &port,
        baud,
        display.as_deref(),
    ))
}

fn prompt_route_output_binding(
    command: &RoutePromptCommand,
    position: RoutePromptPortPosition,
) -> Result<String, String> {
    let id = default_route_output_id(position.index, position.total);
    let port_prompt = position.label("出力ポートを選択");
    let port = prompt_serial_port_with_preview(&port_prompt, |port| {
        Some(command.render(Some(RoutePromptBindingPreview::Output(
            &format_prompt_route_port_binding(&id, port, None, None),
        ))))
    })?;
    let baud_prompt = position.label("出力ボーレート");
    let baud = prompt_u32_choice_with_preview(
        &baud_prompt,
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(RoutePromptBindingPreview::Output(
                &format_prompt_route_port_binding(&id, &port, Some(baud), None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let display_prompt = position.label("出力表示形式");
    let display = prompt_display_mode_with_preview(&display_prompt, false, |display| {
        Some(command.render(Some(RoutePromptBindingPreview::Output(
            &format_prompt_route_port_binding(&id, &port, Some(&baud_text), Some(display)),
        ))))
    })?;

    Ok(format_route_port_binding(
        &id,
        &port,
        baud,
        display.as_deref(),
    ))
}

fn default_route_input_id(index: usize) -> String {
    format_route_letter_id("in", index)
}

fn default_route_output_id(index: usize, total: usize) -> String {
    if total == 1 {
        String::from("out_main")
    } else {
        format_route_letter_id("out", index)
    }
}

fn format_route_letter_id(prefix: &str, index: usize) -> String {
    if (1..=26).contains(&index) {
        let letter = (b'a' + index as u8 - 1) as char;
        format!("{prefix}_{letter}")
    } else {
        format!("{prefix}_{index}")
    }
}

fn format_route_port_binding(id: &str, port: &str, baud: u32, display: Option<&str>) -> String {
    let mut value = format!("{id}={port}@{baud}");
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
    }
    value
}

fn format_prompt_route_port_binding(
    id: &str,
    port: &str,
    baud: Option<&str>,
    display: Option<&str>,
) -> String {
    let mut value = format!("{id}={port}");
    if let Some(baud) = baud {
        value.push('@');
        value.push_str(baud);
    }
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
    }
    value
}

fn build_route_executed_command(
    template: Option<&str>,
    inputs: &[SessionInputSpec],
    outputs: &[SessionOutputSpec],
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) -> String {
    let mut args = vec![String::from("acs"), String::from("route")];

    if let Some(template) = template {
        args.push(template.to_owned());
    }

    for input in inputs {
        args.push(String::from("-i"));
        args.push(format_route_input_command_binding(input));
    }

    for output in outputs {
        args.push(String::from("-o"));
        args.push(format_route_output_command_binding(output));
    }

    append_route_prompt_common_args(&mut args, log_dir, no_log, s3b);
    format_command_preview(&args)
}

fn format_route_input_command_binding(input: &SessionInputSpec) -> String {
    format_route_port_binding(
        &input.id,
        &input.port,
        input.baud_rate,
        Some(&format_input_display_value(
            input.display_mode,
            input.line_break_mode,
        )),
    )
}

fn format_route_output_command_binding(output: &SessionOutputSpec) -> String {
    format_route_port_binding(
        &output.id,
        &output.port,
        output.baud_rate,
        Some(display_mode_value(output.display_mode)),
    )
}

fn append_route_prompt_common_args(
    args: &mut Vec<String>,
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) {
    if let Some(log_dir) = log_dir {
        args.push(String::from("--log-dir"));
        args.push(log_dir.display().to_string());
    }
    if no_log {
        args.push(String::from("--no-log"));
    }
    if s3b {
        args.push(String::from("--s3b"));
    }
}

fn parse_route_args(args: Vec<String>) -> Result<RouteCliOptions, String> {
    let mut options = RouteCliOptions::default();
    let mut iter = args.into_iter().peekable();

    while let Some(arg) = iter.next() {
        if let Some(value) = strip_route_value(&arg, &["-i", "--input-port"]) {
            options.inputs.push(RouteBindingArg::Provided(value));
            continue;
        }
        if let Some(value) = strip_route_value(&arg, &["-o", "--output-port"]) {
            options.outputs.push(RouteBindingArg::Provided(value));
            continue;
        }

        match arg.as_str() {
            "--config" => {
                apply_route_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
            "--template" => options.template = Some(next_value(&mut iter, "--template")?),
            "--list-templates" => options.list_templates = true,
            "--input-port" | "-i" => {
                options.inputs.push(
                    next_optional_route_binding(&mut iter)
                        .map_or(RouteBindingArg::Prompt, RouteBindingArg::Provided),
                );
            }
            "--output-port" | "-o" => {
                options.outputs.push(
                    next_optional_route_binding(&mut iter)
                        .map_or(RouteBindingArg::Prompt, RouteBindingArg::Provided),
                );
            }
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

fn strip_route_value(arg: &str, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| arg.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}

fn next_optional_route_binding(
    iter: &mut std::iter::Peekable<impl Iterator<Item = String>>,
) -> Option<String> {
    match iter.peek() {
        Some(next) if !next.starts_with('-') => iter.next(),
        _ => None,
    }
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
    use super::{
        RouteBindingArg, RoutePromptBindingPreview, RoutePromptCommand, RoutePromptPortPosition,
        RouteRuntimeOptions, build_route_executed_command, normalize_pipeline_spec,
        parse_route_args, resolve_pipeline_spec,
    };
    use crate::pipeline::{
        ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec,
        RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
    };
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use crate::session::runtime::{SessionInputSpec, SessionOutputSpec};
    use std::path::PathBuf;

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
    fn parse_route_args_accepts_bare_input_output_for_prompting() {
        let options = parse_route_args(vec![
            String::from("merge"),
            String::from("-i"),
            String::from("-o"),
        ])
        .unwrap();

        assert_eq!(options.template.as_deref(), Some("merge"));
        assert_eq!(options.inputs, vec![RouteBindingArg::Prompt]);
        assert_eq!(options.outputs, vec![RouteBindingArg::Prompt]);
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
    fn route_executed_command_includes_resolved_ports_defaults_and_flags() {
        let command = build_route_executed_command(
            Some("merge"),
            &[SessionInputSpec {
                id: String::from("in_a"),
                port: String::from("/dev/ttyUSB0"),
                baud_rate: 921_600,
                display_mode: PortDisplayMode::Utf8,
                line_break_mode: LineBreakMode::Packet,
            }],
            &[SessionOutputSpec {
                id: String::from("out_main"),
                port: String::from("/dev/ttyUSB1"),
                baud_rate: 115_200,
                format_name: String::from("bytes"),
                display_mode: PortDisplayMode::Hex,
            }],
            Some(&PathBuf::from("tmp/route logs")),
            true,
            true,
        );

        assert_eq!(
            command,
            "acs route merge -i in_a=/dev/ttyUSB0@921600,utf8+packet -o out_main=/dev/ttyUSB1@115200,hex --log-dir 'tmp/route logs' --no-log --s3b"
        );
    }

    #[test]
    fn route_prompt_command_renders_current_candidate() {
        let options = RouteRuntimeOptions {
            template: Some(String::from("merge")),
            log_dir: Some(PathBuf::from("tmp/route logs")),
            no_log: true,
            s3b: true,
            ..RouteRuntimeOptions::default()
        };
        let mut command = RoutePromptCommand::new(&options);
        command.push_input(String::from("in_a=/dev/ttyUSB0@921600,utf8+packet"));

        assert_eq!(
            command.render(Some(RoutePromptBindingPreview::Output(
                "out_main=/dev/ttyUSB1@115200,hex"
            ))),
            "acs route merge -i in_a=/dev/ttyUSB0@921600,utf8+packet -o out_main=/dev/ttyUSB1@115200,hex --log-dir 'tmp/route logs' --no-log --s3b"
        );
    }

    #[test]
    fn route_prompt_port_position_labels_only_multiple_ports() {
        assert_eq!(
            RoutePromptPortPosition::input(1, 1).label("入力ポートを選択"),
            "入力ポートを選択"
        );
        assert_eq!(
            RoutePromptPortPosition::input(2, 3).label("入力ボーレート"),
            "入力ボーレート (入力ポート 2/3)"
        );
        assert_eq!(
            RoutePromptPortPosition::output(1, 2).label("出力ボーレート"),
            "出力ボーレート (出力ポート 1/2)"
        );
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
