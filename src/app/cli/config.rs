use super::common::{PortSpec, merge_port_specs};
use super::paths::{self, ConfigLookup};
use crate::output::OutputFormat;
use crate::pipeline::{
    ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec, RouterModuleConfig,
    TagRoutingRule, TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{
    LineBreakMode, PortDisplayConfig, PortDisplayMode, PortDisplayStream, parse_display_value,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) use super::paths::LOCAL_CONFIG_DIR_NAME as DEFAULT_CONFIG_DIR_NAME;

#[derive(Debug, Default, Clone)]
pub(crate) struct AppConfig {
    pub log_dir: Option<PathBuf>,
    pub control: ControlConfig,
    pub send: SendConfig,
    pub xbee_test: XbeeTestConfig,
    pub monitor: MonitorConfig,
    pub route: RouteConfig,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ControlConfig {
    pub port: Option<PortSpec>,
    pub baud: Option<u32>,
    pub controller: Option<String>,
    pub format: Option<String>,
    pub display: PortDisplayConfig,
    pub monitor_ports: Vec<PortSpec>,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MonitorConfig {
    pub ports: Vec<PortSpec>,
    pub baud: Option<u32>,
    pub display: PortDisplayConfig,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SendConfig {
    pub port: Option<PortSpec>,
    pub outputs: Vec<SendOutputConfig>,
    pub baud: Option<u32>,
    pub format: Option<String>,
    pub display: PortDisplayConfig,
    pub monitor_ports: Vec<SendMonitorConfig>,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct XbeeTestConfig {
    pub ports: Vec<XbeeTestPortConfig>,
    pub mode: Option<String>,
    pub ac_rate_hz: Option<u32>,
    pub jf_rate_hz: Option<u32>,
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
    pub display: PortDisplayConfig,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct SendOutputConfig {
    pub id: String,
    pub port: String,
    pub baud: Option<u32>,
    pub format: Option<String>,
    pub display_mode: Option<PortDisplayMode>,
    pub line_break_mode: Option<LineBreakMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendMonitorConfig {
    pub port: String,
    pub baud: Option<u32>,
    pub format: Option<String>,
    pub display_mode: Option<PortDisplayMode>,
    pub line_break_mode: Option<LineBreakMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XbeeTestPortConfig {
    pub id: String,
    pub port: String,
    pub baud: Option<u32>,
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
    pub display_mode: Option<PortDisplayMode>,
    pub line_break_mode: Option<LineBreakMode>,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteOutputConfig {
    pub id: String,
    pub port: String,
    pub baud: Option<u32>,
    pub display_mode: Option<PortDisplayMode>,
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

#[derive(Debug, Clone)]
pub(crate) struct LoadedConfig {
    pub config: AppConfig,
    pub lookup: ConfigLookup,
}

impl AppConfig {
    fn merge_from(&mut self, other: AppConfig) {
        merge_option(&mut self.log_dir, other.log_dir);
        self.control.merge_from(other.control);
        self.send.merge_from(other.send);
        self.xbee_test.merge_from(other.xbee_test);
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
        self.display.merge_from(other.display);
        merge_port_specs(&mut self.monitor_ports, other.monitor_ports);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl MonitorConfig {
    fn merge_from(&mut self, other: MonitorConfig) {
        merge_port_specs(&mut self.ports, other.ports);
        merge_option(&mut self.baud, other.baud);
        self.display.merge_from(other.display);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl SendConfig {
    fn merge_from(&mut self, other: SendConfig) {
        merge_option(&mut self.port, other.port);
        merge_send_outputs(&mut self.outputs, other.outputs);
        merge_option(&mut self.baud, other.baud);
        merge_option(&mut self.format, other.format);
        self.display.merge_from(other.display);
        merge_send_monitors(&mut self.monitor_ports, other.monitor_ports);
        merge_option(&mut self.log_dir, other.log_dir);
    }
}

impl XbeeTestConfig {
    fn merge_from(&mut self, other: XbeeTestConfig) {
        merge_xbee_test_ports(&mut self.ports, other.ports);
        merge_option(&mut self.mode, other.mode);
        merge_option(&mut self.ac_rate_hz, other.ac_rate_hz);
        merge_option(&mut self.jf_rate_hz, other.jf_rate_hz);
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

fn merge_send_outputs(target: &mut Vec<SendOutputConfig>, outputs: Vec<SendOutputConfig>) {
    for output in outputs {
        if let Some(existing) = target.iter_mut().find(|existing| existing.id == output.id) {
            *existing = output;
        } else {
            target.push(output);
        }
    }
}

fn merge_send_monitors(target: &mut Vec<SendMonitorConfig>, monitors: Vec<SendMonitorConfig>) {
    for monitor in monitors {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.port == monitor.port)
        {
            *existing = monitor;
        } else {
            target.push(monitor);
        }
    }
}

fn merge_xbee_test_ports(target: &mut Vec<XbeeTestPortConfig>, ports: Vec<XbeeTestPortConfig>) {
    for port in ports {
        if let Some(existing) = target.iter_mut().find(|existing| existing.id == port.id) {
            *existing = port;
        } else {
            target.push(port);
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
    let send = parse_send_config(root.get("send"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let xbee_test = parse_xbee_test_config(root.get("xbee_test"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let monitor = parse_monitor_config(root.get("monitor"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let route = parse_route_config(root.get("route"), base_dir)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    Ok(AppConfig {
        log_dir: top_level_log_dir,
        control,
        send,
        xbee_test,
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

pub(crate) fn load_config_or_default(explicit_path: Option<&Path>) -> Result<LoadedConfig, String> {
    let lookup = match explicit_path {
        Some(path) => ConfigLookup::Explicit(path.to_path_buf()),
        None => paths::find_default_config_lookup(),
    };

    let config = match lookup.path() {
        Some(path) => load_config(path)?,
        None => AppConfig::default(),
    };

    Ok(LoadedConfig { config, lookup })
}

fn parse_control_config(
    value: Option<&JsonValue>,
    base_dir: &Path,
) -> Result<ControlConfig, String> {
    let Some(value) = value else {
        return Ok(ControlConfig::default());
    };
    let object = expect_object(value, "control")?;
    let mut display = legacy_input_display_config(optional_bool(object, "raw")?);
    display.merge_from(optional_display_config(object, "display")?);

    Ok(ControlConfig {
        port: optional_port_spec(object, "port")?,
        baud: optional_u32(object, "baud")?,
        controller: optional_string(object, "controller")?,
        format: optional_string(object, "format")?,
        display,
        monitor_ports: optional_port_spec_list(object, "monitor_ports")?,
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
    let mut display = legacy_input_display_config(optional_bool(object, "raw")?);
    display.merge_from(optional_display_config(object, "display")?);
    let mut ports = optional_port_spec_list(object, "ports")?;
    if ports.is_empty()
        && let Some(port) = optional_port_spec(object, "port")?
    {
        ports.push(port);
    }

    Ok(MonitorConfig {
        ports,
        baud: optional_u32(object, "baud")?,
        display,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_send_config(value: Option<&JsonValue>, base_dir: &Path) -> Result<SendConfig, String> {
    let Some(value) = value else {
        return Ok(SendConfig::default());
    };
    let object = expect_object(value, "send")?;

    Ok(SendConfig {
        port: optional_port_spec(object, "port")?,
        outputs: optional_send_outputs(object, "outputs")?,
        baud: optional_u32(object, "baud")?,
        format: optional_string(object, "format")?,
        display: optional_display_config(object, "display")?,
        monitor_ports: optional_send_monitors(object, "monitor_ports")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_xbee_test_config(
    value: Option<&JsonValue>,
    base_dir: &Path,
) -> Result<XbeeTestConfig, String> {
    let Some(value) = value else {
        return Ok(XbeeTestConfig::default());
    };
    let object = expect_object(value, "xbee_test")?;

    Ok(XbeeTestConfig {
        ports: optional_xbee_test_ports(object, "ports")?,
        mode: optional_string(object, "mode")?,
        ac_rate_hz: optional_u32(object, "ac_rate")?,
        jf_rate_hz: optional_u32(object, "jf_rate")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_route_config(value: Option<&JsonValue>, base_dir: &Path) -> Result<RouteConfig, String> {
    let Some(value) = value else {
        return Ok(RouteConfig::default());
    };
    let object = expect_object(value, "route")?;
    let mut display = legacy_input_display_config(optional_bool(object, "raw")?);
    display.merge_from(optional_display_config(object, "display")?);

    Ok(RouteConfig {
        inputs: optional_route_inputs(object, "inputs")?,
        outputs: optional_route_outputs(object, "outputs")?,
        pipelines: optional_pipeline_spec(object, "pipelines")?,
        template: optional_string(object, "template")?,
        templates: optional_route_templates(object.get("templates"))?,
        baud: optional_u32(object, "baud")?,
        display,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn legacy_input_display_config(raw: Option<bool>) -> PortDisplayConfig {
    let mut display = PortDisplayConfig::default();
    if let Some(raw) = raw {
        display.set_line_break_for_stream(
            Some(PortDisplayStream::Input),
            "default",
            if raw {
                LineBreakMode::Packet
            } else {
                LineBreakMode::Line
            },
        );
    }
    display
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
                    apply_display_value(
                        &mut config,
                        Some(PortDisplayStream::Input),
                        scope_target,
                        mode,
                        &format!("{key}.{target}.{scope_target}"),
                    )?;
                }
            }
            ("output" | "tx", JsonValue::Object(scope)) => {
                for (scope_target, scope_value) in scope {
                    let JsonValue::String(mode) = scope_value else {
                        return Err(format!("`{key}.{target}.{scope_target}` must be string"));
                    };
                    apply_display_value(
                        &mut config,
                        Some(PortDisplayStream::Output),
                        scope_target,
                        mode,
                        &format!("{key}.{target}.{scope_target}"),
                    )?;
                }
            }
            (_, JsonValue::String(mode)) => {
                apply_display_value(&mut config, None, target, mode, &format!("{key}.{target}"))?;
            }
            _ => return Err(format!("`{key}.{target}` must be string or object")),
        }
    }

    Ok(config)
}

fn apply_display_value(
    config: &mut PortDisplayConfig,
    stream: Option<PortDisplayStream>,
    target: &str,
    mode: &str,
    key_path: &str,
) -> Result<(), String> {
    let (display_mode, line_break_mode) =
        parse_display_value(mode).map_err(|error| format!("`{key_path}`: {error}"))?;
    if display_mode.is_none() && line_break_mode.is_none() {
        return Err(format!(
            "`{key_path}` must be one of hex/ascii/utf8/hex+ascii/hex+utf8/line/packet"
        ));
    }
    if let Some(display_mode) = display_mode {
        config.set_for_stream(stream, target.to_owned(), display_mode);
    }
    if let Some(line_break_mode) = line_break_mode {
        config.set_line_break_for_stream(stream, target.to_owned(), line_break_mode);
    }
    Ok(())
}

fn optional_port_spec(object: &JsonObject, key: &str) -> Result<Option<PortSpec>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };

    parse_port_spec_value(value, key)
}

fn optional_port_spec_list(object: &JsonObject, key: &str) -> Result<Vec<PortSpec>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::String(_)) | Some(JsonValue::Object(_)) => {
            parse_port_spec_value(object.get(key).expect("value exists"), key)
                .map(|value| value.into_iter().collect())
        }
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(|value| match parse_port_spec_value(value, key) {
                Ok(Some(spec)) => Some(Ok(spec)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect(),
        Some(_) => Err(type_error(key, "string, object, or array")),
    }
}

fn parse_port_spec_value(value: &JsonValue, key: &str) -> Result<Option<PortSpec>, String> {
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(parse_port_spec_text(trimmed, key)?))
        }
        JsonValue::Object(object) => parse_port_spec_object(object, key),
        _ => Err(type_error(key, "string or object")),
    }
}

fn parse_port_spec_text(value: &str, key: &str) -> Result<PortSpec, String> {
    let (port_and_baud, display_mode, line_break_mode) = match value.rsplit_once(',') {
        Some((port_and_baud, mode)) => match parse_display_value(mode) {
            Ok((display_mode, line_break_mode))
                if display_mode.is_some() || line_break_mode.is_some() =>
            {
                (port_and_baud, display_mode, line_break_mode)
            }
            _ => (value, None, None),
        },
        None => (value, None, None),
    };

    let (port, baud) = match port_and_baud.rsplit_once('@') {
        Some((port, baud)) => (
            port.trim(),
            Some(
                baud.parse::<u32>()
                    .map_err(|_| format!("invalid value for `{key}`: {value}"))?,
            ),
        ),
        None => (port_and_baud.trim(), None),
    };

    if port.is_empty() {
        return Err(format!("`{key}` must not be empty"));
    }

    Ok(PortSpec {
        port: port.to_owned(),
        baud,
        display_mode,
        line_break_mode,
    })
}

fn parse_port_spec_object(object: &JsonObject, _key: &str) -> Result<Option<PortSpec>, String> {
    let port = optional_string(object, "path")?
        .or(optional_string(object, "port")?)
        .unwrap_or_default();
    let trimmed = port.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    Ok(Some(PortSpec {
        port: trimmed.to_owned(),
        baud: optional_u32(object, "baud")?,
        display_mode: optional_string(object, "display")?
            .map(|mode| parse_display_value(&mode).map(|(display_mode, _)| display_mode))
            .transpose()?
            .flatten(),
        line_break_mode: optional_string(object, "display")?
            .map(|mode| parse_display_value(&mode).map(|(_, line_break_mode)| line_break_mode))
            .transpose()?
            .flatten(),
    }))
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
        let display = optional_string(entry, "display")?
            .map(|mode| parse_display_value(&mode))
            .transpose()?;
        inputs.push(RouteInputConfig {
            id: required_string(entry, "id")?,
            port: required_string(entry, "port")?,
            baud: optional_u32(entry, "baud")?,
            display_mode: display.and_then(|(display_mode, _)| display_mode),
            line_break_mode: display.and_then(|(_, line_break_mode)| line_break_mode),
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
        let display = optional_string(entry, "display")?
            .map(|mode| parse_display_value(&mode))
            .transpose()?;
        outputs.push(RouteOutputConfig {
            id: required_string(entry, "id")?,
            port: required_string(entry, "port")?,
            baud: optional_u32(entry, "baud")?,
            display_mode: display.and_then(|(display_mode, _)| display_mode),
        });
    }

    Ok(outputs)
}

fn optional_send_outputs(object: &JsonObject, key: &str) -> Result<Vec<SendOutputConfig>, String> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let values = expect_array(value, key)?;
    let mut outputs = Vec::new();

    for value in values {
        let entry = expect_object(value, key)?;
        let display = optional_string(entry, "display")?
            .map(|mode| parse_display_value(&mode))
            .transpose()?;
        outputs.push(SendOutputConfig {
            id: optional_string(entry, "id")?
                .unwrap_or_else(|| required_string(entry, "port").expect("port exists")),
            port: required_string(entry, "port")?,
            baud: optional_u32(entry, "baud")?,
            format: optional_string(entry, "format")?,
            display_mode: display.and_then(|(display_mode, _)| display_mode),
            line_break_mode: display.and_then(|(_, line_break_mode)| line_break_mode),
        });
    }

    Ok(outputs)
}

fn optional_send_monitors(
    object: &JsonObject,
    key: &str,
) -> Result<Vec<SendMonitorConfig>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::String(_)) | Some(JsonValue::Object(_)) => {
            parse_send_monitor_value(object.get(key).expect("value exists"), key)
                .map(|value| value.into_iter().collect())
        }
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(|value| match parse_send_monitor_value(value, key) {
                Ok(Some(spec)) => Some(Ok(spec)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect(),
        Some(_) => Err(type_error(key, "string, object, or array")),
    }
}

fn parse_send_monitor_value(
    value: &JsonValue,
    key: &str,
) -> Result<Option<SendMonitorConfig>, String> {
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(parse_send_monitor_text(trimmed, key)?))
        }
        JsonValue::Object(object) => parse_send_monitor_object(object, key),
        _ => Err(type_error(key, "string or object")),
    }
}

fn parse_send_monitor_text(value: &str, key: &str) -> Result<SendMonitorConfig, String> {
    let (port_text, format) = if let Some((port_text, format_name)) = value.rsplit_once(',') {
        if OutputFormat::parse(format_name).is_ok() {
            (port_text, Some(format_name.to_owned()))
        } else {
            (value, None)
        }
    } else {
        (value, None)
    };

    let port_spec = parse_port_spec_text(port_text, key)?;
    Ok(SendMonitorConfig {
        port: port_spec.port,
        baud: port_spec.baud,
        format,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

fn parse_send_monitor_object(
    object: &JsonObject,
    key: &str,
) -> Result<Option<SendMonitorConfig>, String> {
    let Some(port_spec) = parse_port_spec_object(object, key)? else {
        return Ok(None);
    };

    Ok(Some(SendMonitorConfig {
        port: port_spec.port,
        baud: port_spec.baud,
        format: optional_string(object, "format")?,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    }))
}

fn optional_xbee_test_ports(
    object: &JsonObject,
    key: &str,
) -> Result<Vec<XbeeTestPortConfig>, String> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::String(_)) | Some(JsonValue::Object(_)) => {
            parse_xbee_test_port_value(object.get(key).expect("value exists"), key)
                .map(|value| value.into_iter().collect())
        }
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(|value| match parse_xbee_test_port_value(value, key) {
                Ok(Some(spec)) => Some(Ok(spec)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect(),
        Some(_) => Err(type_error(key, "string, object, or array")),
    }
}

fn parse_xbee_test_port_value(
    value: &JsonValue,
    key: &str,
) -> Result<Option<XbeeTestPortConfig>, String> {
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(parse_xbee_test_port_text(trimmed, key)?))
        }
        JsonValue::Object(object) => parse_xbee_test_port_object(object, key),
        _ => Err(type_error(key, "string or object")),
    }
}

fn parse_xbee_test_port_text(value: &str, key: &str) -> Result<XbeeTestPortConfig, String> {
    let Some((id, port_text)) = value.split_once('=') else {
        return Err(format!(
            "`{key}` entry must be `base=PORT[@BAUD]` or `rover=PORT[@BAUD]`"
        ));
    };

    let port_spec = parse_port_spec_text(port_text, key)?;
    if port_spec.display_mode.is_some() || port_spec.line_break_mode.is_some() {
        return Err(format!(
            "`{key}` entry must not specify display mode; xbee_test uses fixed hex packet display"
        ));
    }

    Ok(XbeeTestPortConfig {
        id: normalize_xbee_test_port_id(id)?,
        port: port_spec.port,
        baud: port_spec.baud,
    })
}

fn parse_xbee_test_port_object(
    object: &JsonObject,
    key: &str,
) -> Result<Option<XbeeTestPortConfig>, String> {
    let id = optional_string(object, "id")?.unwrap_or_default();
    let id = id.trim();
    if id.is_empty() {
        return Ok(None);
    }

    let port = optional_string(object, "path")?
        .or(optional_string(object, "port")?)
        .unwrap_or_default();
    let port = port.trim();
    if port.is_empty() {
        return Err(format!("`{key}.port` is required"));
    }

    Ok(Some(XbeeTestPortConfig {
        id: normalize_xbee_test_port_id(id)?,
        port: port.to_owned(),
        baud: optional_u32(object, "baud")?,
    }))
}

fn normalize_xbee_test_port_id(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "base" | "rover" => Ok(value),
        _ => Err(format!(
            "xbee_test port id must be `base` or `rover`, got `{value}`"
        )),
    }
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
        "output_encode" => Ok(TransformModuleConfig::OutputEncode {
            format: OutputFormat::parse(&required_string(object, "format")?)?,
        }),
        "packetacv6_encode" => Ok(TransformModuleConfig::OutputEncode {
            format: OutputFormat::PacketAcV6,
        }),
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
    use super::{JsonParser, SendMonitorConfig, XbeeTestPortConfig, load_config};
    use crate::app::cli::common::PortSpec;
    use crate::output::OutputFormat;
    use crate::pipeline::TransformModuleConfig;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
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
            "format": "packetacv6",
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
          "send": {
            "port": "/dev/ttyUSB2",
            "baud": 115200,
            "format": "packetjfv1",
            "monitor_ports": ["/dev/ttyUSB3"],
            "display": {
              "output": {
                "default": "hex"
              }
            }
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
              "send": {
                "format": "packetjfv1",
                "monitor_ports": ["/dev/ttyUSB2"]
              },
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
        assert_eq!(config.send.format, Some(String::from("packetjfv1")));
        assert!(config.xbee_test.ports.is_empty());
        assert_eq!(
            config.send.monitor_ports,
            vec![SendMonitorConfig {
                port: String::from("/dev/ttyUSB2"),
                baud: None,
                format: None,
                display_mode: None,
                line_break_mode: None,
            }]
        );
        assert_eq!(
            config.monitor.ports,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: None,
                display_mode: None,
                line_break_mode: None,
            }]
        );
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

    #[test]
    fn load_config_accepts_port_objects_with_baud_and_display() {
        let temp_dir = make_temp_dir("acs_config_port_objects");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "control": {
                "port": { "path": "/dev/ttyUSB0", "baud": 921600, "display": "hex" },
                "monitor_ports": [
                  { "port": "/dev/ttyUSB1", "baud": 115200, "display": "utf8" }
                ]
              },
              "monitor": {
                "ports": [
                  { "path": "/dev/ttyUSB2", "baud": 460800, "display": "hex+ascii" }
                ]
              },
              "route": {
                "inputs": [
                  { "id": "in_a", "port": "/dev/ttyUSB3", "baud": 230400, "display": "utf8" }
                ],
                "outputs": [
                  { "id": "out_main", "port": "/dev/ttyUSB4", "baud": 460800, "display": "hex" }
                ]
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(
            config.control.port,
            Some(PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: None,
            })
        );
        assert_eq!(
            config.control.monitor_ports,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB1"),
                baud: Some(115_200),
                display_mode: Some(PortDisplayMode::Utf8),
                line_break_mode: None,
            }]
        );
        assert_eq!(
            config.monitor.ports,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB2"),
                baud: Some(460_800),
                display_mode: Some(PortDisplayMode::HexAscii),
                line_break_mode: None,
            }]
        );
        assert_eq!(config.route.inputs[0].baud, Some(230_400));
        assert_eq!(
            config.route.inputs[0].display_mode,
            Some(PortDisplayMode::Utf8)
        );
        assert_eq!(config.route.outputs[0].baud, Some(460_800));
        assert_eq!(
            config.route.outputs[0].display_mode,
            Some(PortDisplayMode::Hex)
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_accepts_send_outputs_with_per_output_settings() {
        let temp_dir = make_temp_dir("acs_config_send_outputs");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "send": {
                "outputs": [
                  {
                    "id": "main",
                    "port": "/dev/ttyUSB0",
                    "baud": 921600,
                    "format": "packetacv6",
                    "display": "hex+packet"
                  },
                  {
                    "id": "sub",
                    "port": "/dev/ttyUSB1",
                    "baud": 115200,
                    "format": "packetjfv1",
                    "display": "utf8+line"
                  }
                ]
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(config.send.outputs.len(), 2);
        assert_eq!(config.send.outputs[0].id, "main");
        assert_eq!(config.send.outputs[0].format.as_deref(), Some("packetacv6"));
        assert_eq!(
            config.send.outputs[0].display_mode,
            Some(PortDisplayMode::Hex)
        );
        assert_eq!(
            config.send.outputs[0].line_break_mode,
            Some(LineBreakMode::Packet)
        );
        assert_eq!(config.send.outputs[1].id, "sub");
        assert_eq!(config.send.outputs[1].format.as_deref(), Some("packetjfv1"));
        assert_eq!(
            config.send.outputs[1].display_mode,
            Some(PortDisplayMode::Utf8)
        );
        assert_eq!(
            config.send.outputs[1].line_break_mode,
            Some(LineBreakMode::Line)
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_accepts_send_monitor_formats() {
        let temp_dir = make_temp_dir("acs_config_send_monitors");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "send": {
                "monitor_ports": [
                  "/dev/ttyUSB9@115200,utf8+line,packetjfv1",
                  { "port": "/dev/ttyUSB8", "baud": 921600, "display": "hex+packet", "format": "packetacv6" }
                ]
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(config.send.monitor_ports.len(), 2);
        assert_eq!(config.send.monitor_ports[0].port, "/dev/ttyUSB9");
        assert_eq!(
            config.send.monitor_ports[0].format.as_deref(),
            Some("packetjfv1")
        );
        assert_eq!(
            config.send.monitor_ports[0].display_mode,
            Some(PortDisplayMode::Utf8)
        );
        assert_eq!(
            config.send.monitor_ports[0].line_break_mode,
            Some(LineBreakMode::Line)
        );
        assert_eq!(config.send.monitor_ports[1].port, "/dev/ttyUSB8");
        assert_eq!(
            config.send.monitor_ports[1].format.as_deref(),
            Some("packetacv6")
        );
        assert_eq!(
            config.send.monitor_ports[1].display_mode,
            Some(PortDisplayMode::Hex)
        );
        assert_eq!(
            config.send.monitor_ports[1].line_break_mode,
            Some(LineBreakMode::Packet)
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_accepts_xbee_test_ports_and_rates() {
        let temp_dir = make_temp_dir("acs_config_xbee_test");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "xbee_test": {
                "ports": [
                  "base=/dev/ttyUSB0@921600",
                  { "id": "rover", "port": "/dev/ttyUSB1", "baud": 115200 }
                ],
                "mode": "ping-pong",
                "ac_rate": 100,
                "jf_rate": 80
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(
            config.xbee_test.ports,
            vec![
                XbeeTestPortConfig {
                    id: String::from("base"),
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(921_600),
                },
                XbeeTestPortConfig {
                    id: String::from("rover"),
                    port: String::from("/dev/ttyUSB1"),
                    baud: Some(115_200),
                }
            ]
        );
        assert_eq!(config.xbee_test.mode.as_deref(), Some("ping-pong"));
        assert_eq!(config.xbee_test.ac_rate_hz, Some(100));
        assert_eq!(config.xbee_test.jf_rate_hz, Some(80));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_maps_legacy_raw_to_packet_line_break() {
        let temp_dir = make_temp_dir("acs_config_legacy_raw");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "monitor": {
                "raw": true
              },
              "control": {
                "raw": false
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();

        assert_eq!(
            config
                .monitor
                .display
                .resolve_line_break_input("/dev/ttyUSB0"),
            LineBreakMode::Packet
        );
        assert_eq!(
            config
                .control
                .display
                .resolve_line_break_input("/dev/ttyUSB0"),
            LineBreakMode::Line
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_rejects_legacy_arm9_transform_module_name() {
        let temp_dir = make_temp_dir("acs_config_reject_arm9_transform");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "route": {
                "inputs": [
                  { "id": "in_a", "port": "/dev/ttyUSB0" }
                ],
                "outputs": [
                  { "id": "out_main", "port": "/dev/ttyUSB1" }
                ],
                "pipelines": [
                  {
                    "id": "legacy_transform",
                    "inputs": ["in_a"],
                    "transform": {
                      "module": "arm9_encode"
                    },
                    "route": {
                      "module": "broadcast",
                      "outputs": ["out_main"]
                    }
                  }
                ]
              }
            }
            "#,
        )
        .unwrap();

        let error = load_config(&config_path).expect_err("legacy transform should fail");
        assert!(error.contains("unsupported transform module: arm9_encode"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_config_accepts_generic_output_encode_transform_module() {
        let temp_dir = make_temp_dir("acs_config_output_encode");
        let config_path = temp_dir.join("config.json");
        fs::write(
            &config_path,
            r#"
            {
              "route": {
                "inputs": [
                  { "id": "in_a", "port": "/dev/ttyUSB0" }
                ],
                "outputs": [
                  { "id": "out_main", "port": "/dev/ttyUSB1" }
                ],
                "pipelines": [
                  {
                    "id": "encode_packet",
                    "inputs": ["in_a"],
                    "transform": {
                      "module": "output_encode",
                      "format": "packetacv6"
                    },
                    "route": {
                      "module": "broadcast",
                      "outputs": ["out_main"]
                    }
                  }
                ]
              }
            }
            "#,
        )
        .unwrap();

        let config = load_config(&config_path).expect("generic output encode should parse");
        match &config.route.pipelines.pipelines[0].transform.modules[0] {
            TransformModuleConfig::OutputEncode { format } => {
                assert_eq!(*format, OutputFormat::PacketAcV6);
            }
            other => panic!("unexpected transform module: {other:?}"),
        }

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
