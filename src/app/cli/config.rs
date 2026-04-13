use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub(crate) struct AppConfig {
    pub log_dir: Option<PathBuf>,
    pub control: ControlConfig,
    pub monitor: MonitorConfig,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ControlConfig {
    pub port: Option<String>,
    pub baud: Option<u32>,
    pub controller: Option<String>,
    pub format: Option<String>,
    pub raw: Option<bool>,
    pub monitor_ports: Vec<String>,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MonitorConfig {
    pub ports: Vec<String>,
    pub baud: Option<u32>,
    pub raw: Option<bool>,
    pub log_dir: Option<PathBuf>,
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

pub(crate) fn load_config(path: &Path) -> Result<AppConfig, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("failed to read config file: {error}"))?;
    let root = JsonParser::new(&text).parse()?;
    let root = expect_object(&root, "root")?;
    // 相対パスは config ファイル基準で解決しておくと扱いやすい。
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let top_level_log_dir = optional_path(root, "log_dir", base_dir)?;
    let control = parse_control_config(root.get("control"), base_dir)?;
    let monitor = parse_monitor_config(root.get("monitor"), base_dir)?;

    Ok(AppConfig {
        log_dir: top_level_log_dir,
        control,
        monitor,
    })
}

fn parse_control_config(value: Option<&JsonValue>, base_dir: &Path) -> Result<ControlConfig, String> {
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
        monitor_ports: optional_string_list(object, "monitor_ports")?,
        log_dir: optional_path(object, "log_dir", base_dir)?,
    })
}

fn parse_monitor_config(value: Option<&JsonValue>, base_dir: &Path) -> Result<MonitorConfig, String> {
    let Some(value) = value else {
        return Ok(MonitorConfig::default());
    };
    let object = expect_object(value, "monitor")?;
    let mut ports = optional_string_list(object, "ports")?;
    if ports.is_empty() && let Some(port) = optional_string(object, "port")? {
        ports.push(port);
    }

    Ok(MonitorConfig {
        ports,
        baud: optional_u32(object, "baud")?,
        raw: optional_bool(object, "raw")?,
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

fn expect_object<'a>(value: &'a JsonValue, name: &str) -> Result<&'a JsonObject, String> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("`{name}` must be a JSON object")),
    }
}

fn type_error(key: &str, expected: &str) -> String {
    format!("`{key}` must be {expected}")
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
            return Err(String::from("unexpected trailing characters in config file"));
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
    use super::JsonParser;

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
            "monitor_ports": ["/dev/ttyUSB1"]
          },
          "monitor": {
            "ports": ["/dev/ttyUSB0", "/dev/ttyUSB1"],
            "baud": 115200,
            "raw": true
          }
        }
        "#;

        assert!(JsonParser::new(config).parse().is_ok());
    }
}
