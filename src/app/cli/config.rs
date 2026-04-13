use crate::common::extend_unique_strings;
use crate::pipeline::{
    ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec, RouterModuleConfig,
    TagRoutingRule, TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{PortDisplayConfig, PortDisplayMode};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_CONFIG_FILE_NAME: &str = "acs.config.json";
pub(crate) const DEFAULT_CONFIG_DIR_NAME: &str = "config";

#[derive(Debug, Default, Clone)]
pub(crate) struct AppConfig {
    pub log_dir: Option<PathBuf>,
    pub control: ControlConfig,
    pub monitor: MonitorConfig,
    pub route: RouteConfig,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ControlConfig {
    pub port: Option<String>,
    pub baud: Option<u32>,
    pub controller: Option<String>,
    pub format: Option<String>,
    pub raw: Option<bool>,
    pub display: PortDisplayConfig,
    pub monitor_ports: Vec<String>,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MonitorConfig {
    pub ports: Vec<String>,
    pub baud: Option<u32>,
    pub raw: Option<bool>,
    pub display: PortDisplayConfig,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RouteConfig {
    pub inputs: Vec<RouteInputConfig>,
    pub outputs: Vec<RouteOutputConfig>,
    pub pipelines: PipelineSpec,
    pub template: Option<String>,
    pub templates: BTreeMap<String, RouteTemplateConfig>,
    pub baud: Option<u32>,
    pub raw: Option<bool>,
    pub display: PortDisplayConfig,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteTemplateConfig {
    pub id: String,
    pub description: Option<String>,
    pub pipelines: PipelineSpec,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteInputConfig {
    pub id: String,
    pub port: String,
    pub baud: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteOutputConfig {
    pub id: String,
    pub port: String,
    pub baud: Option<u32>,
}

#[derive(Debug, Clone)]
enum JsonValue {
    Object(BTreeMap<String, JsonValue>),
    Array(Vec<JsonValue>),
    String(String),
    Number(String),
    Bool(bool),
    Null,
}

type JsonObject = BTreeMap<String, JsonValue>;

impl AppConfig {
    fn merge_from(&mut self, other: AppConfig) {
        merge_option(&mut self.log_dir, other.log_dir);
        self.control.merge_from(other.control);
        self.monitor.merge_from(other.monitor);
        self.route.merge_from(other.route);
    }
}

impl ControlConfig {
    fn merge_from(&mut self, other: ControlConfig) {
        merge_option(&mut self.port, other.port);
        merge_option(&mut self.baud, other.baud);
        merge_option(&mut self.controller, other.controller);
        merge_option(&mut self.format, other.format);
        merge_option(&mut self.raw, other.raw);
        self.display.merge_from(other.display);
        extend_unique_strings(&mut self.monitor_ports, &other.monitor_ports);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl MonitorConfig {
    fn merge_from(&mut self, other: MonitorConfig) {
        extend_unique_strings(&mut self.ports, &other.ports);
        merge_option(&mut self.baud, other.baud);
        merge_option(&mut self.raw, other.raw);
        self.display.merge_from(other.display);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl RouteConfig {
    fn merge_from(&mut self, other: RouteConfig) {
        merge_route_inputs(&mut self.inputs, other.inputs);
        merge_route_outputs(&mut self.outputs, other.outputs);
        self.pipelines.merge_from(other.pipelines);
        merge_option(&mut self.template, other.template);
        for template in other.templates.into_values() {
            self.templates.insert(template.id.clone(), template);
        }
        merge_option(&mut self.baud, other.baud);
        merge_option(&mut self.raw, other.raw);
        self.display.merge_from(other.display);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl PipelineSpec {
    fn merge_from(&mut self, other: PipelineSpec) {
        for pipeline in other.pipelines {
            if let Some(existing) = self
                .pipelines
                .iter_mut()
                .find(|existing| existing.id == pipeline.id)
            {
                *existing = pipeline;
            } else {
                self.pipelines.push(pipeline);
            }
        }
    }
}

fn merge_option<T>(target: &mut Option<T>, other: Option<T>) {
    if let Some(value) = other {
        *target = Some(value);
    }
}

fn merge_route_inputs(target: &mut Vec<RouteInputConfig>, inputs: Vec<RouteInputConfig>) {
    for input in inputs {
        if let Some(existing) = target.iter_mut().find(|existing| existing.id == input.id) {
            *existing = input;
        } else {
            target.push(input);
        }
    }
}

fn merge_route_outputs(target: &mut Vec<RouteOutputConfig>, outputs: Vec<RouteOutputConfig>) {
    for output in outputs {
        if let Some(existing) = target.iter_mut().find(|existing| existing.id == output.id) {
            *existing = output;
        } else {
            target.push(output);
        }
    }
}

pub(crate) fn load_config(path: &Path) -> Result<AppConfig, String> {
    if path.is_dir() {
        return load_config_dir(path);
    }

    load_config_file(path)
}

fn load_config_file(path: &Path) -> Result<AppConfig, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read config file {}: {error}", path.display()))?;
    let root = JsonParser::new(&text)
        .parse()
        .map_err(|error| format!("failed to parse config file {}: {error}", path.display()))?;
    let root =
        expect_object(&root, "root").map_err(|error| format!("{}: {error}", path.display()))?;
    // 相対パスは config ファイル基準で解決しておくと扱いやすい。
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let top_level_log_dir = optional_path(root, "log_dir", base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let control = parse_control_config(root.get("control"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let monitor = parse_monitor_config(root.get("monitor"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let route = parse_route_config(root.get("route"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    Ok(AppConfig {
        log_dir: top_level_log_dir,
        control,
        monitor,
        route,
    })
}

fn load_config_dir(path: &Path) -> Result<AppConfig, String> {
    let mut files = Vec::new();
    collect_config_files(path, &mut files)?;
    files.sort();

    let mut merged = AppConfig::default();
    for file in files {
        merged.merge_from(load_config_file(&file)?);
    }

    Ok(merged)
}

fn collect_config_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(path).map_err(|error| {
        format!(
            "failed to read config directory {}: {error}",
            path.display()
        )
    })?;
    let mut children = entries
        .map(|entry| {
            entry.map(|entry| entry.path()).map_err(|error| {
                format!(
                    "failed to read config directory {}: {error}",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();

    for child in children {
        if child.is_dir() {
            collect_config_files(&child, files)?;
            continue;
        }

        if child.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(child);
        }
    }

    Ok(())
}

pub(crate) fn load_config_or_default(explicit_path: Option<&Path>) -> Result<AppConfig, String> {
    match explicit_path {
        Some(path) => load_config(path),
        None => match find_default_config_path() {
            Some(path) => load_config(&path),
            None => Ok(AppConfig::default()),
        },
    }
}

fn find_default_config_path() -> Option<PathBuf> {
    let current_dir = std::env::current_dir().ok()?;
    let config_dir = current_dir.join(DEFAULT_CONFIG_DIR_NAME);
    if config_dir.is_dir() {
        return Some(config_dir);
    }

    let config_file = current_dir.join(DEFAULT_CONFIG_FILE_NAME);
    config_file.is_file().then_some(config_file)
}

fn parse_control_config(
    value: Option<&JsonValue>,
    base_dir: &Path,
) -> Result<ControlConfig, String> {
    let Some(value) = value else {
        return Ok(ControlConfig::default());
    };
    let object = expect_object(value, "control")?;

    Ok(ControlConfig {
        port: optional_string(object, "port")?,
        baud: optional_u32(object, "baud")?,
        controller: optional_string(object, "controller")?,
        format: optional_string(object, "format")?,
        raw: optional_bool(object, "raw")?,
        display: optional_display_config(object, "display")?,
        monitor_ports: optional_string_list(object, "monitor_ports")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_monitor_config(
    value: Option<&JsonValue>,
    base_dir: &Path,
) -> Result<MonitorConfig, String> {
    let Some(value) = value else {
        return Ok(MonitorConfig::default());
    };
    let object = expect_object(value, "monitor")?;
    let mut ports = optional_string_list(object, "ports")?;
    if ports.is_empty()
        && let Some(port) = optional_string(object, "port")?
    {
        ports.push(port);
    }

    Ok(MonitorConfig {
        ports,
        baud: optional_u32(object, "baud")?,
        raw: optional_bool(object, "raw")?,
        display: optional_display_config(object, "display")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_route_config(value: Option<&JsonValue>, base_dir: &Path) -> Result<RouteConfig, String> {
    let Some(value) = value else {
        return Ok(RouteConfig::default());
    };
    let object = expect_object(value, "route")?;

    Ok(RouteConfig {
        inputs: optional_route_inputs(object, "inputs")?,
        outputs: optional_route_outputs(object, "outputs")?,
        pipelines: optional_pipeline_spec(object, "pipelines")?,
        template: optional_string(object, "template")?,
        templates: optional_route_templates(object.get("templates"))?,
        baud: optional_u32(object, "baud")?,
        raw: optional_bool(object, "raw")?,
        display: optional_display_config(object, "display")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn optional_string(object: &JsonObject, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(type_error(key, "string")),
    }
}

fn optional_string_list(object: &JsonObject, key: &str) -> Result<Vec<String>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::String(value)) => Ok(vec![value.clone()]),
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(text) => Ok(text.clone()),
                _ => Err(type_error(key, "array of strings")),
            })
            .collect(),
        Some(_) => Err(type_error(key, "string or array of strings")),
    }
}

fn optional_u32(object: &JsonObject, key: &str) -> Result<Option<u32>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(value)) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("`{key}` must be an unsigned integer")),
        Some(_) => Err(type_error(key, "number")),
    }
}

fn optional_bool(object: &JsonObject, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(type_error(key, "boolean")),
    }
}

fn optional_path(
    object: &JsonObject,
    key: &str,
    base_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(value) = optional_string(object, key)? else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Ok(Some(path));
    }
    Ok(Some(base_dir.join(path)))
}

fn optional_display_config(object: &JsonObject, key: &str) -> Result<PortDisplayConfig, String> {
    let Some(value) = object.get(key) else {
        return Ok(PortDisplayConfig::default());
    };
    let JsonValue::Object(entries) = value else {
        return Err(type_error(key, "object"));
    };

    let mut config = PortDisplayConfig::default();
    for (target, value) in entries {
        match (target.as_str(), value) {
            ("input" | "rx", JsonValue::Object(scope)) => {
                for (scope_target, scope_value) in scope {
                    let JsonValue::String(mode) = scope_value else {
                        return Err(format!("`{key}.{target}.{scope_target}` must be string"));
                    };
                    config.set_input(scope_target.clone(), PortDisplayMode::parse(mode)?);
                }
            }
            ("output" | "tx", JsonValue::Object(scope)) => {
                for (scope_target, scope_value) in scope {
                    let JsonValue::String(mode) = scope_value else {
                        return Err(format!("`{key}.{target}.{scope_target}` must be string"));
                    };
                    config.set_output(scope_target.clone(), PortDisplayMode::parse(mode)?);
                }
            }
            (_, JsonValue::String(mode)) => {
                config.set_both(target.clone(), PortDisplayMode::parse(mode)?);
            }
            _ => return Err(format!("`{key}.{target}` must be string or object")),
        }
    }

    Ok(config)
}

fn expect_object<'a>(value: &'a JsonValue, name: &str) -> Result<&'a JsonObject, String> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("`{name}` must be a JSON object")),
    }
}

fn expect_array<'a>(value: &'a JsonValue, name: &str) -> Result<&'a Vec<JsonValue>, String> {
    match value {
        JsonValue::Array(values) => Ok(values),
        _ => Err(format!("`{name}` must be a JSON array")),
    }
}

fn required_string(object: &JsonObject, key: &str) -> Result<String, String> {
    optional_string(object, key)?.ok_or_else(|| format!("`{key}` is required"))
}

fn type_error(key: &str, expected: &str) -> String {
    format!("`{key}` must be {expected}")
}

fn optional_route_inputs(object: &JsonObject, key: &str) -> Result<Vec<RouteInputConfig>, String> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let values = expect_array(value, key)?;
    let mut inputs = Vec::new();

    for value in values {
        let entry = expect_object(value, key)?;
        inputs.push(RouteInputConfig {
            id: required_string(entry, "id")?,
            port: required_string(entry, "port")?,
            baud: optional_u32(entry, "baud")?,
        });
    }

    Ok(inputs)
}

fn optional_route_outputs(
    object: &JsonObject,
    key: &str,
) -> Result<Vec<RouteOutputConfig>, String> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let values = expect_array(value, key)?;
    let mut outputs = Vec::new();

    for value in values {
        let entry = expect_object(value, key)?;
        outputs.push(RouteOutputConfig {
            id: required_string(entry, "id")?,
            port: required_string(entry, "port")?,
            baud: optional_u32(entry, "baud")?,
        });
    }

    Ok(outputs)
}

fn optional_pipeline_spec(object: &JsonObject, key: &str) -> Result<PipelineSpec, String> {
    let Some(value) = object.get(key) else {
        return Ok(PipelineSpec::default());
    };

    parse_pipeline_spec_value(value, key)
}

fn parse_pipeline_spec_value(value: &JsonValue, key: &str) -> Result<PipelineSpec, String> {
    let values = expect_array(value, key)?;
    let mut pipelines = Vec::new();

    for value in values {
        let pipeline = expect_object(value, key)?;
        pipelines.push(PipelineDefinition {
            id: required_string(pipeline, "id")?,
            inputs: optional_string_list(pipeline, "inputs")?,
            filter: parse_filter_module(pipeline.get("filter"))?,
            transform: parse_transform_chain(pipeline.get("transform"))?,
            classify: parse_classify_module(pipeline.get("classify"))?,
            router: parse_router_module(pipeline.get("route"))?,
        });
    }

    Ok(PipelineSpec { pipelines })
}

fn optional_route_templates(
    value: Option<&JsonValue>,
) -> Result<BTreeMap<String, RouteTemplateConfig>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    match value {
        JsonValue::Object(entries) => {
            let mut templates = BTreeMap::new();
            for (id, value) in entries {
                let template = parse_route_template(value, id, "route.templates")?;
                templates.insert(id.clone(), template);
            }
            Ok(templates)
        }
        JsonValue::Array(values) => {
            let mut templates = BTreeMap::new();
            for value in values {
                let object = expect_object(value, "route.templates")?;
                let id = required_string(object, "id")?;
                let template = parse_route_template(value, &id, "route.templates")?;
                templates.insert(id, template);
            }
            Ok(templates)
        }
        _ => Err(type_error("route.templates", "object or array")),
    }
}

fn parse_route_template(
    value: &JsonValue,
    id: &str,
    name: &str,
) -> Result<RouteTemplateConfig, String> {
    let object = expect_object(value, name)?;
    let Some(pipelines_value) = object.get("pipelines") else {
        return Err(format!("`{name}.{id}.pipelines` is required"));
    };

    Ok(RouteTemplateConfig {
        id: id.to_owned(),
        description: optional_string(object, "description")?,
        pipelines: parse_pipeline_spec_value(pipelines_value, "pipelines")?,
    })
}

fn parse_filter_module(value: Option<&JsonValue>) -> Result<FilterModuleConfig, String> {
    let Some(value) = value else {
        return Ok(FilterModuleConfig::AllowAll);
    };
    let object = expect_object(value, "filter")?;
    let module = required_string(object, "module")?;

    match module.as_str() {
        "allow_all" => Ok(FilterModuleConfig::AllowAll),
        "drop_empty" => Ok(FilterModuleConfig::DropEmpty),
        "match_source" => Ok(FilterModuleConfig::MatchSource {
            input_ids: optional_string_list(object, "input_ids")?,
        }),
        "match_prefix" => Ok(FilterModuleConfig::MatchPrefix {
            prefix: parse_hex_bytes(&required_string(object, "prefix_hex")?)?,
        }),
        other => Err(format!("unsupported filter module: {other}")),
    }
}

fn parse_transform_chain(value: Option<&JsonValue>) -> Result<TransformChainConfig, String> {
    let Some(value) = value else {
        return Ok(TransformChainConfig::default());
    };
    let object = expect_object(value, "transform")?;

    if let Some(modules_value) = object.get("modules") {
        let modules = expect_array(modules_value, "transform.modules")?
            .iter()
            .map(parse_transform_module)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(TransformChainConfig { modules });
    }

    Ok(TransformChainConfig {
        modules: vec![parse_transform_module(value)?],
    })
}

fn parse_transform_module(value: &JsonValue) -> Result<TransformModuleConfig, String> {
    let object = expect_object(value, "transform module")?;
    let module = required_string(object, "module")?;

    match module.as_str() {
        "identity" => Ok(TransformModuleConfig::Identity),
        "ds4_to_compact" => Ok(TransformModuleConfig::Ds4ToCompact),
        "arm9_encode" => Ok(TransformModuleConfig::Arm9Encode),
        "join_latest" => Ok(TransformModuleConfig::JoinLatest {
            separator: parse_hex_bytes(
                &optional_string(object, "separator_hex")?.unwrap_or_default(),
            )?,
            require_all: optional_bool(object, "require_all")?.unwrap_or(true),
        }),
        other => Err(format!("unsupported transform module: {other}")),
    }
}

fn parse_classify_module(value: Option<&JsonValue>) -> Result<ClassifyModuleConfig, String> {
    let Some(value) = value else {
        return Ok(ClassifyModuleConfig::None);
    };
    let object = expect_object(value, "classify")?;
    let module = required_string(object, "module")?;

    match module.as_str() {
        "none" => Ok(ClassifyModuleConfig::None),
        "by_source" => Ok(ClassifyModuleConfig::BySource),
        "tag_static" => Ok(ClassifyModuleConfig::TagStatic {
            tags: optional_string_list(object, "tags")?,
        }),
        "match_prefix" => Ok(ClassifyModuleConfig::MatchPrefix {
            prefix: parse_hex_bytes(&required_string(object, "prefix_hex")?)?,
            tag: required_string(object, "tag")?,
        }),
        other => Err(format!("unsupported classify module: {other}")),
    }
}

fn parse_router_module(value: Option<&JsonValue>) -> Result<RouterModuleConfig, String> {
    let Some(value) = value else {
        return Err(String::from(
            "`route` module is required for every pipeline",
        ));
    };
    let object = expect_object(value, "route")?;
    let module = required_string(object, "module")?;

    match module.as_str() {
        "broadcast" => Ok(RouterModuleConfig::Broadcast {
            outputs: optional_string_list(object, "outputs")?,
        }),
        "round_robin" | "alternate" => Ok(RouterModuleConfig::RoundRobin {
            outputs: optional_string_list(object, "outputs")?,
        }),
        "source_map" => Ok(RouterModuleConfig::SourceMap {
            routes: parse_output_map(object.get("routes"), "route.routes")?,
            default_outputs: optional_string_list(object, "default_outputs")?,
        }),
        "tag_based" => Ok(RouterModuleConfig::TagBased {
            routes: parse_tag_routes(object.get("routes"), "route.routes")?,
            default_outputs: optional_string_list(object, "default_outputs")?,
        }),
        other => Err(format!("unsupported route module: {other}")),
    }
}

fn parse_output_map(
    value: Option<&JsonValue>,
    name: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = expect_object(value, name)?;
    let mut routes = BTreeMap::new();

    for (key, value) in object {
        routes.insert(key.clone(), json_string_list(value, name)?);
    }

    Ok(routes)
}

fn parse_tag_routes(value: Option<&JsonValue>, name: &str) -> Result<Vec<TagRoutingRule>, String> {
    let map = parse_output_map(value, name)?;
    Ok(map
        .into_iter()
        .map(|(tag, outputs)| TagRoutingRule { tag, outputs })
        .collect())
}

fn json_string_list(value: &JsonValue, key: &str) -> Result<Vec<String>, String> {
    match value {
        JsonValue::String(text) => Ok(vec![text.clone()]),
        JsonValue::Array(values) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(text) => Ok(text.clone()),
                _ => Err(type_error(key, "array of strings")),
            })
            .collect(),
        _ => Err(type_error(key, "string or array of strings")),
    }
}

fn parse_hex_bytes(value: &str) -> Result<Vec<u8>, String> {
    let compact = value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if compact.len() % 2 != 0 {
        return Err(format!("invalid hex byte string: {value}"));
    }

    let mut bytes = Vec::new();
    for index in (0..compact.len()).step_by(2) {
        let byte = u8::from_str_radix(&compact[index..index + 2], 16)
            .map_err(|_| format!("invalid hex byte string: {value}"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

struct JsonParser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn parse(mut self) -> Result<JsonValue, String> {
        self.consume_whitespace();
        let value = self.parse_value()?;
        self.consume_whitespace();
        if self.peek_char().is_some() {
            return Err(String::from(
                "unexpected trailing characters in config file",
            ));
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        // この設定ファイルで必要になる JSON の最小セットだけを手書きで読む。
        match self.peek_char() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('t') => self.parse_keyword("true", JsonValue::Bool(true)),
            Some('f') => self.parse_keyword("false", JsonValue::Bool(false)),
            Some('n') => self.parse_keyword("null", JsonValue::Null),
            Some('-' | '0'..='9') => self.parse_number().map(JsonValue::Number),
            Some(other) => Err(format!("unexpected character in config file: `{other}`")),
            None => Err(String::from("unexpected end of config file")),
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect_char('{')?;
        self.consume_whitespace();
        let mut object = BTreeMap::new();

        if self.peek_char() == Some('}') {
            self.next_char();
            return Ok(JsonValue::Object(object));
        }

        loop {
            self.consume_whitespace();
            let key = self.parse_string()?;
            self.consume_whitespace();
            self.expect_char(':')?;
            self.consume_whitespace();
            let value = self.parse_value()?;
            object.insert(key, value);
            self.consume_whitespace();

            match self.peek_char() {
                Some(',') => {
                    self.next_char();
                }
                Some('}') => {
                    self.next_char();
                    return Ok(JsonValue::Object(object));
                }
                _ => return Err(String::from("expected `,` or `}` in config object")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect_char('[')?;
        self.consume_whitespace();
        let mut values = Vec::new();

        if self.peek_char() == Some(']') {
            self.next_char();
            return Ok(JsonValue::Array(values));
        }

        loop {
            self.consume_whitespace();
            values.push(self.parse_value()?);
            self.consume_whitespace();

            match self.peek_char() {
                Some(',') => {
                    self.next_char();
                }
                Some(']') => {
                    self.next_char();
                    return Ok(JsonValue::Array(values));
                }
                _ => return Err(String::from("expected `,` or `]` in config array")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_char('"')?;
        let mut value = String::new();

        loop {
            let ch = self
                .next_char()
                .ok_or_else(|| String::from("unterminated string in config file"))?;

            match ch {
                '"' => return Ok(value),
                '\\' => value.push(self.parse_escape_sequence()?),
                other => value.push(other),
            }
        }
    }

    fn parse_escape_sequence(&mut self) -> Result<char, String> {
        match self
            .next_char()
            .ok_or_else(|| String::from("incomplete escape sequence in config file"))?
        {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            '/' => Ok('/'),
            'b' => Ok('\u{0008}'),
            'f' => Ok('\u{000C}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'u' => {
                let hex = self.take_chars(4)?;
                let code = u32::from_str_radix(&hex, 16)
                    .map_err(|_| String::from("invalid unicode escape in config file"))?;
                char::from_u32(code)
                    .ok_or_else(|| String::from("invalid unicode scalar in config file"))
            }
            other => Err(format!("unsupported escape sequence: \\{other}")),
        }
    }

    fn parse_number(&mut self) -> Result<String, String> {
        let start = self.position;

        if self.peek_char() == Some('-') {
            self.next_char();
        }

        match self.peek_char() {
            Some('0') => {
                self.next_char();
            }
            Some('1'..='9') => {
                self.consume_digits();
            }
            _ => return Err(String::from("invalid number in config file")),
        }

        if self.peek_char() == Some('.') {
            self.next_char();
            self.consume_required_digits()?;
        }

        if matches!(self.peek_char(), Some('e' | 'E')) {
            self.next_char();
            if matches!(self.peek_char(), Some('+' | '-')) {
                self.next_char();
            }
            self.consume_required_digits()?;
        }

        let number = &self.input[start..self.position];
        number
            .parse::<f64>()
            .map_err(|_| String::from("invalid number in config file"))?;
        Ok(number.to_owned())
    }

    fn parse_keyword(&mut self, keyword: &str, value: JsonValue) -> Result<JsonValue, String> {
        if self.input[self.position..].starts_with(keyword) {
            self.position += keyword.len();
            return Ok(value);
        }
        Err(format!("invalid token in config file near `{keyword}`"))
    }

    fn expect_char(&mut self, expected: char) -> Result<(), String> {
        match self.next_char() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(format!("expected `{expected}` but found `{actual}`")),
            None => Err(format!("expected `{expected}` but found end of file")),
        }
    }

    fn consume_required_digits(&mut self) -> Result<(), String> {
        if !matches!(self.peek_char(), Some('0'..='9')) {
            return Err(String::from("invalid number in config file"));
        }
        self.consume_digits();
        Ok(())
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek_char(), Some('0'..='9')) {
            self.next_char();
        }
    }

    fn consume_whitespace(&mut self) {
        while matches!(self.peek_char(), Some(ch) if ch.is_whitespace()) {
            self.next_char();
        }
    }

    fn take_chars(&mut self, count: usize) -> Result<String, String> {
        let mut result = String::new();
        for _ in 0..count {
            result.push(
                self.next_char()
                    .ok_or_else(|| String::from("unexpected end of config file"))?,
            );
        }
        Ok(result)
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.position += ch.len_utf8();
        Some(ch)
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonParser, load_config};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_config_shape_used_by_example_file() {
        let config = r#"
        {
          "log_dir": "logs",
          "control": {
            "port": "/dev/ttyUSB0",
            "baud": 115200,
            "controller": "0",
            "format": "arm9",
            "raw": false,
            "display": {
              "default": "hex+utf8",
              "input": {
                "default": "utf8",
                "/dev/ttyUSB0": "utf8"
              },
              "output": {
                "default": "hex",
                "/dev/ttyUSB0": "hex"
              }
            },
            "monitor_ports": ["/dev/ttyUSB1"]
          },
          "monitor": {
            "ports": ["/dev/ttyUSB0", "/dev/ttyUSB1"],
            "baud": 115200,
            "raw": true,
            "display": {
              "default": "hex+utf8",
              "input": {
                "default": "hex",
                "/dev/ttyUSB1": "hex+ascii"
              }
            }
          },
          "route": {
            "baud": 115200,
            "raw": true,
            "inputs": [
              { "id": "in_a", "port": "/dev/ttyUSB0" },
              { "id": "in_b", "port": "/dev/ttyUSB1" }
            ],
            "outputs": [
              { "id": "out_main", "port": "/dev/ttyUSB2" }
            ],
            "pipelines": [
              {
                "id": "default",
                "inputs": ["in_a", "in_b"],
                "filter": { "module": "allow_all" },
                "transform": { "module": "identity" },
                "classify": { "module": "by_source" },
                "route": {
                  "module": "broadcast",
                  "outputs": ["out_main"]
                }
              }
            ]
          }
        }
        "#;

        assert!(JsonParser::new(config).parse().is_ok());
    }

    #[test]
    fn load_config_merges_json_files_from_directory() {
        let temp_dir = make_temp_dir("acs_config_merge");
        fs::create_dir_all(temp_dir.join("route")).unwrap();
        fs::write(
            temp_dir.join("00-common.json"),
            r#"
            {
              "log_dir": "logs",
              "monitor": {
                "ports": ["/dev/ttyUSB0"]
              }
            }
            "#,
        )
        .unwrap();
        fs::write(
            temp_dir.join("10-route.json"),
            r#"
            {
              "route": {
                "baud": 115200,
                "inputs": [
                  { "id": "in_a", "port": "/dev/ttyUSB0" }
                ],
                "outputs": [
                  { "id": "out_main", "port": "/dev/ttyUSB1" }
                ]
              }
            }
            "#,
        )
        .unwrap();
        fs::write(
            temp_dir.join("route/20-route-template.json"),
            r#"
            {
              "route": {
                "inputs": [
                  { "id": "in_a", "port": "/dev/ttyUSB9" },
                  { "id": "in_b", "port": "/dev/ttyUSB2" }
                ],
                "templates": {
                  "merge_pair": {
                    "description": "merge bytes to main output",
                    "pipelines": [
                      {
                        "id": "merge_pair",
                        "transform": {
                          "module": "identity"
                        },
                        "route": {
                          "module": "broadcast"
                        }
                      }
                    ]
                  }
                }
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&temp_dir).unwrap();

        assert_eq!(config.log_dir, Some(temp_dir.join("logs")));
        assert_eq!(config.monitor.ports, vec![String::from("/dev/ttyUSB0")]);
        assert_eq!(config.route.baud, Some(115200));
        assert_eq!(config.route.inputs.len(), 2);
        assert_eq!(config.route.inputs[0].port, "/dev/ttyUSB9");
        assert_eq!(config.route.inputs[1].id, "in_b");
        assert_eq!(config.route.outputs.len(), 1);
        assert_eq!(
            config
                .route
                .templates
                .get("merge_pair")
                .and_then(|template| template.description.as_deref()),
            Some("merge bytes to main output")
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), unique));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
