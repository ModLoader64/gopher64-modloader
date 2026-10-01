use super::*;
use serde_json::{Map, Value};

#[path = "settings_schema.rs"]
mod schema;
use schema::{DESCRIPTIONS, Description};

pub(super) struct Settings {
    path: std::path::PathBuf,
    document: Value,
    values: Vec<CString>,
}

fn find(key: &str) -> Option<usize> {
    DESCRIPTIONS
        .iter()
        .position(|description| description.key.to_bytes() == key.as_bytes())
}

fn normalize(description: &Description, value: &str) -> Option<String> {
    if value.contains('\0') {
        return None;
    }
    let bounded = description.minimum < description.maximum;
    let in_range =
        |number: f64| !bounded || (number >= description.minimum && number <= description.maximum);
    match description.kind {
        SETTING_BOOL => matches!(value, "true" | "false").then(|| value.to_string()),
        SETTING_INT => value
            .parse::<i64>()
            .ok()
            .filter(|number| in_range(*number as f64))
            .map(|number| number.to_string()),
        SETTING_FLOAT => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && in_range(*number))
            .map(|number| number.to_string()),
        SETTING_CHOICE => description
            .choices
            .to_str()
            .unwrap_or("")
            .lines()
            .any(|line| line.split('\t').next() == Some(value))
            .then(|| value.to_string()),
        _ => Some(value.to_string()),
    }
}

fn stored_text(value: &Value) -> Option<String> {
    match value {
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn to_json(description: &Description, value: &str) -> Value {
    match description.kind {
        SETTING_BOOL => Value::Bool(value == "true"),
        SETTING_INT | SETTING_FLOAT | SETTING_CHOICE => {
            if let Ok(number) = value.parse::<i64>() {
                Value::from(number)
            } else if description.kind == SETTING_FLOAT
                && let Ok(number) = value.parse::<f64>()
            {
                Value::from(number)
            } else {
                Value::String(value.to_string())
            }
        }
        _ => Value::String(value.to_string()),
    }
}

fn store(document: &mut Value, key: &str, value: Value) {
    let mut object = document;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if !object.is_object() {
            *object = Value::Object(Map::new());
        }
        let map = object.as_object_mut().unwrap();
        if parts.peek().is_none() {
            map.insert(part.to_string(), value);
            return;
        }
        object = map
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

impl Settings {
    pub(super) fn load(path: std::path::PathBuf, problems: &mut Vec<String>) -> Settings {
        let mut document = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                problems.push(format!(
                    "{} is not JSON ({}); defaults replace it",
                    path.display(),
                    error
                ));
                Value::Object(Map::new())
            }),
            Err(_) => Value::Object(Map::new()),
        };
        let mut missing = document
            .get_mut("rt64")
            .and_then(|object| object.as_object_mut())
            .is_some_and(|object| {
                object.remove("resolution").is_some() | object.remove("filtering").is_some()
            });
        let mut values = Vec::new();
        for description in DESCRIPTIONS {
            let key = description.key.to_str().unwrap_or("");
            let stored_json = key
                .split('.')
                .try_fold(&document, |object, part| object.get(part));
            let stored = stored_json.and_then(stored_text);
            let value = match stored
                .as_deref()
                .and_then(|text| normalize(description, text))
            {
                Some(value) => value,
                None => {
                    if let Some(stored) = stored {
                        problems.push(format!("{}: {} cannot be {}", path.display(), key, stored));
                    }
                    description.default.to_string()
                }
            };
            let canonical = to_json(description, &value);
            if stored_json != Some(&canonical) {
                missing = true;
                store(&mut document, key, canonical);
            }
            values.push(CString::new(value).unwrap());
        }
        let settings = Settings {
            path,
            document,
            values,
        };
        if missing {
            settings.save(problems);
        }
        settings
    }

    fn save(&self, problems: &mut Vec<String>) {
        let written = serde_json::to_string_pretty(&self.document)
            .map(|text| std::fs::write(&self.path, text + "\n").is_ok())
            .unwrap_or(false);
        if !written {
            problems.push(format!("cannot write {}", self.path.display()));
        }
    }

    pub(super) fn value(&self, key: &str) -> &str {
        find(key)
            .and_then(|index| self.values[index].to_str().ok())
            .unwrap_or("")
    }

    pub(super) fn flag(&self, key: &str) -> bool {
        self.value(key) == "true"
    }

    pub(super) fn number(&self, key: &str) -> f64 {
        self.value(key).parse().unwrap_or(0.0)
    }

    pub(super) fn rt64(&self) -> CString {
        let mut rt64 = self
            .document
            .get("rt64")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if let Some(object) = rt64.as_object_mut() {
            let window = object
                .remove("windowSize")
                .and_then(|value| value.as_bool());
            let resolution = if window.unwrap_or(true) {
                "WindowIntegerScale"
            } else {
                "Manual"
            };
            object.insert("resolution".into(), Value::from(resolution));
            object.insert("filtering".into(), Value::from("AntiAliasedPixelScaling"));
        }
        CString::new(rt64.to_string()).unwrap_or_default()
    }

    fn hidden(&self, key: &str) -> bool {
        match key {
            "rt64.resolutionMultiplier" => self.flag("rt64.windowSize"),
            "rt64.aspectTarget" => self.value("rt64.aspectRatio") != "Manual",
            "rt64.refreshRateTarget" => self.value("rt64.refreshRate") != "Manual",
            _ => false,
        }
    }

    pub(super) fn count(&self) -> u32 {
        DESCRIPTIONS.len() as u32
    }

    pub(super) fn describe(&self, index: u32) -> Option<Setting> {
        let description = DESCRIPTIONS.get(index as usize)?;
        Some(Setting {
            key: description.key.as_ptr(),
            label: description.label.as_ptr(),
            page: description.page.as_ptr(),
            help: description.help.as_ptr(),
            choices: description.choices.as_ptr(),
            value: self.values[index as usize].as_ptr(),
            type_: description.kind as u32,
            flags: (description.flags
                | if self.hidden(description.key.to_str().unwrap_or("")) {
                    SETTING_HIDDEN
                } else {
                    0
                }) as u32,
            minimum: description.minimum,
            maximum: description.maximum,
        })
    }

    pub(super) fn set(
        &mut self,
        key: &str,
        value: &str,
        problems: &mut Vec<String>,
    ) -> Option<bool> {
        let index = find(key)?;
        let description = &DESCRIPTIONS[index];
        let value = normalize(description, value)?;
        let stored = CString::new(value.as_str()).ok()?;
        store(&mut self.document, key, to_json(description, &value));
        self.values[index] = stored;
        self.save(problems);
        Some(description.flags & SETTING_RESTART == 0)
    }
}
