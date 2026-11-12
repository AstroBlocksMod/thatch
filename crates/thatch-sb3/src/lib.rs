//! Reads Scratch 3 project files (`.sb3`) and bare `project.json` files into a typed model.
//!
//! The model keeps what it does not interpret: every struct has an `extra` map holding the fields
//! Thatch has no use for yet, and block inputs stay as the raw JSON arrays they are in the file, with
//! [`Input`] offering a parsed view. Assets are listed by their file names, not loaded.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a zip archive: {source}")]
    Zip {
        path: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("{path} has no project.json")]
    NoProjectJson { path: String },
    #[error("project.json is not valid: {0}")]
    Json(#[from] serde_json::Error),
}

/// A Scratch 3 project.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub targets: Vec<Target>,
    #[serde(default)]
    pub monitors: Vec<Value>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub meta: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The stage or a sprite.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    #[serde(default)]
    pub is_stage: bool,
    pub name: String,
    /// Variable id to `[name, value]` (a third element marks a cloud variable).
    #[serde(default)]
    pub variables: Map<String, Value>,
    /// List id to `[name, [items...]]`.
    #[serde(default)]
    pub lists: Map<String, Value>,
    /// Broadcast id to name. Only the stage has these.
    #[serde(default)]
    pub broadcasts: Map<String, Value>,
    #[serde(default)]
    pub blocks: Map<String, Value>,
    #[serde(default)]
    pub current_costume: i64,
    #[serde(default)]
    pub costumes: Vec<Value>,
    #[serde(default)]
    pub sounds: Vec<Value>,
    #[serde(default)]
    pub layer_order: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A block, as opposed to a top-level primitive (a variable or list reporter that sits on its own,
/// stored as an array).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub opcode: String,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub inputs: Map<String, Value>,
    #[serde(default)]
    pub fields: Map<String, Value>,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default)]
    pub top_level: bool,
    #[serde(default)]
    pub mutation: Option<Mutation>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The mutation of a procedure prototype or call.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutation {
    #[serde(default)]
    pub proccode: String,
    /// A JSON array of argument ids, as a string.
    #[serde(default)]
    pub argumentids: String,
    #[serde(default)]
    pub argumentnames: String,
    #[serde(default)]
    pub argumentdefaults: String,
    /// `"true"` or `"false"` as a string in most files, a boolean in some.
    #[serde(default)]
    pub warp: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Mutation {
    pub fn is_warp(&self) -> bool {
        match &self.warp {
            Value::Bool(b) => *b,
            Value::String(s) => s == "true",
            _ => false,
        }
    }

    fn string_list(json: &str) -> Vec<String> {
        serde_json::from_str::<Vec<Value>>(json)
            .map(|v| v.into_iter().map(|x| json_to_text(&x)).collect())
            .unwrap_or_default()
    }

    pub fn argument_ids(&self) -> Vec<String> {
        Self::string_list(&self.argumentids)
    }

    pub fn argument_names(&self) -> Vec<String> {
        Self::string_list(&self.argumentnames)
    }

    pub fn argument_defaults(&self) -> Vec<String> {
        Self::string_list(&self.argumentdefaults)
    }
}

/// A primitive stored inline in an input, or as a top-level entry in `blocks`.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    /// Number, positive number, positive integer, integer, angle, color or text shadows (types 4
    /// through 10). The text is the value as written in the file.
    Text(String),
    Broadcast {
        name: String,
        id: String,
    },
    Variable {
        name: String,
        id: String,
    },
    List {
        name: String,
        id: String,
    },
}

/// A parsed view of one block input.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Nothing is plugged in and there is no shadow: an empty boolean or substack slot.
    Empty,
    /// A block is plugged in (possibly covering a shadow).
    Block(String),
    /// Only a shadow, given inline as a primitive.
    Primitive(Primitive),
}

/// Decodes a `[kind, value, shadow?]` input array. The first element is 1 (shadow only), 2 (block,
/// no shadow) or 3 (block covering a shadow); the second is a block id or an inline primitive.
pub fn parse_input(raw: &Value) -> Input {
    let Some(parts) = raw.as_array() else {
        return Input::Empty;
    };
    match parts.get(1) {
        None | Some(Value::Null) => Input::Empty,
        Some(Value::String(id)) => Input::Block(id.clone()),
        Some(other) => match parse_primitive(other) {
            Some(p) => Input::Primitive(p),
            None => Input::Empty,
        },
    }
}

/// Decodes an inline primitive array such as `[10, "hello"]` or `[12, "score", "varid"]`.
pub fn parse_primitive(raw: &Value) -> Option<Primitive> {
    let parts = raw.as_array()?;
    let kind = parts.first()?.as_i64()?;
    let text = parts.get(1).map(json_to_text).unwrap_or_default();
    let id = parts.get(2).map(json_to_text).unwrap_or_default();
    Some(match kind {
        4..=10 => Primitive::Text(text),
        11 => Primitive::Broadcast { name: text, id },
        12 => Primitive::Variable { name: text, id },
        13 => Primitive::List { name: text, id },
        _ => return None,
    })
}

/// The value of a field: `[value, id]` where the id is null for menus.
pub fn field_value(block: &Block, name: &str) -> Option<String> {
    let raw = block.fields.get(name)?;
    match raw {
        Value::Array(parts) => parts.first().map(json_to_text),
        other => Some(json_to_text(other)),
    }
}

/// The id stored with a field, for variables, lists and broadcasts.
pub fn field_id(block: &Block, name: &str) -> Option<String> {
    let parts = block.fields.get(name)?.as_array()?;
    match parts.get(1) {
        Some(Value::String(id)) => Some(id.clone()),
        _ => None,
    }
}

/// A JSON scalar as the text Scratch would show for it. Numbers in `project.json` are written by
/// JavaScript, so the shortest round-trip form matches `String(n)` closely enough for ids and names.
pub fn json_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

impl Target {
    /// The block with the given id, or None when the id is a top-level primitive or unknown.
    pub fn block(&self, id: &str) -> Option<Block> {
        let raw = self.blocks.get(id)?;
        if raw.is_array() {
            return None;
        }
        serde_json::from_value(raw.clone()).ok()
    }

    /// All real blocks (not top-level primitives), keyed by id and in file order.
    pub fn all_blocks(&self) -> Vec<(String, Block)> {
        self.blocks
            .iter()
            .filter_map(|(id, raw)| {
                if raw.is_array() {
                    return None;
                }
                serde_json::from_value::<Block>(raw.clone()).ok().map(|b| (id.clone(), b))
            })
            .collect()
    }

    /// Variables as (id, name, initial value), in file order.
    pub fn variable_list(&self) -> Vec<(String, String, Value)> {
        self.variables
            .iter()
            .filter_map(|(id, raw)| {
                let parts = raw.as_array()?;
                let name = parts.first().map(json_to_text)?;
                let value = parts.get(1).cloned().unwrap_or(Value::Number(0.into()));
                Some((id.clone(), name, value))
            })
            .collect()
    }

    /// Broadcast names by id.
    pub fn broadcast_names(&self) -> BTreeMap<String, String> {
        self.broadcasts.iter().map(|(id, v)| (id.clone(), json_to_text(v))).collect()
    }
}

impl Project {
    /// Parses a `project.json`.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        Ok(serde_json::from_str(json)?)
    }

    /// Reads an `.sb3` archive or a bare `project.json` file.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let display = path.display().to_string();
        let io = |source| Error::Io { path: display.clone(), source };
        let bytes = std::fs::read(path).map_err(io)?;
        if bytes.first() == Some(&b'{') {
            let text = String::from_utf8_lossy(&bytes);
            return Self::from_json(&text);
        }
        let cursor = std::io::Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|source| Error::Zip { path: display.clone(), source })?;
        let mut file = archive.by_name("project.json").map_err(|_| Error::NoProjectJson { path: display.clone() })?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(io)?;
        Self::from_json(&text)
    }

    pub fn stage(&self) -> Option<&Target> {
        self.targets.iter().find(|t| t.is_stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inputs_and_fields() {
        let json = r#"{"targets":[{"isStage":true,"name":"Stage","variables":{"v1":["score",0]},
            "blocks":{"a":{"opcode":"looks_say","next":null,"parent":null,
            "inputs":{"MESSAGE":[1,[10,"hi"]],"OTHER":[3,"b",[10,""]],"COND":[2,"c"]},
            "fields":{"VARIABLE":["score","v1"],"OP":["abs",null]},"shadow":false,"topLevel":true},
            "p":[12,"score","v1",10,20]}}]}"#;
        let project = Project::from_json(json).unwrap();
        let stage = project.stage().unwrap();
        let block = stage.block("a").unwrap();
        assert_eq!(parse_input(&block.inputs["MESSAGE"]), Input::Primitive(Primitive::Text("hi".into())));
        assert_eq!(parse_input(&block.inputs["OTHER"]), Input::Block("b".into()));
        assert_eq!(parse_input(&block.inputs["COND"]), Input::Block("c".into()));
        assert_eq!(field_value(&block, "VARIABLE").as_deref(), Some("score"));
        assert_eq!(field_id(&block, "VARIABLE").as_deref(), Some("v1"));
        assert_eq!(field_id(&block, "OP"), None);
        assert!(stage.block("p").is_none());
        assert_eq!(stage.all_blocks().len(), 1);
        assert_eq!(stage.variable_list()[0].1, "score");
    }
}
