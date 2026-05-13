//! OpenAPI-3-flavored JSON Schema used by Gemini's `FunctionDeclaration` and
//! `Schema` types. Also used by the other providers (which accept raw JSON
//! Schema directly).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::SchemaError;

/// JSON-Schema primitive type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SchemaType {
    /// Reserved unspecified type.
    TypeUnspecified,
    /// JSON object (record).
    Object,
    /// JSON array.
    Array,
    /// JSON string.
    String,
    /// JSON number.
    Number,
    /// JSON integer.
    Integer,
    /// JSON boolean.
    Boolean,
}

/// OpenAPI-3 / Gemini schema node.
///
/// `properties` uses [`IndexMap`] to preserve insertion order — Gemini's
/// declarations are order-sensitive and Python's Pydantic models stay
/// ordered.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Schema {
    /// The primitive type.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub r#type: Option<SchemaType>,
    /// Human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// String format hint (`date-time`, `int32`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Whether `null` is allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    /// Allowed enum values.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    /// Object property schemas.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub properties: IndexMap<String, Schema>,
    /// Names of required object properties.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Schema for array items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    /// Minimum array length.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "minItems")]
    pub min_items: Option<u64>,
    /// Maximum array length.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxItems")]
    pub max_items: Option<u64>,
    /// Example values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// Default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// String pattern (regex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Minimum string length.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "minLength")]
    pub min_length: Option<u64>,
    /// Maximum string length.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxLength")]
    pub max_length: Option<u64>,
    /// Minimum numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Maximum numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Title (display name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Property ordering hint (Gemini-specific).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "propertyOrdering"
    )]
    pub property_ordering: Option<Vec<String>>,
}

impl Schema {
    /// Construct an object schema.
    pub fn object() -> Self {
        Self {
            r#type: Some(SchemaType::Object),
            ..Self::default()
        }
    }

    /// Construct a string schema.
    pub fn string() -> Self {
        Self {
            r#type: Some(SchemaType::String),
            ..Self::default()
        }
    }

    /// Construct a number schema.
    pub fn number() -> Self {
        Self {
            r#type: Some(SchemaType::Number),
            ..Self::default()
        }
    }

    /// Construct an integer schema.
    pub fn integer() -> Self {
        Self {
            r#type: Some(SchemaType::Integer),
            ..Self::default()
        }
    }

    /// Construct a boolean schema.
    pub fn boolean() -> Self {
        Self {
            r#type: Some(SchemaType::Boolean),
            ..Self::default()
        }
    }

    /// Construct an array schema with the given item schema.
    pub fn array(items: Schema) -> Self {
        Self {
            r#type: Some(SchemaType::Array),
            items: Some(Box::new(items)),
            ..Self::default()
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    /// Add a property; the property is auto-tracked in `properties`. Use
    /// [`Self::require`] to mark it required.
    #[must_use]
    pub fn property(mut self, name: impl Into<String>, schema: Schema) -> Self {
        self.properties.insert(name.into(), schema);
        self
    }

    /// Mark a property as required.
    #[must_use]
    pub fn require(mut self, name: impl Into<String>) -> Self {
        self.required.push(name.into());
        self
    }

    /// Apply Gemini-compatibility constraints in place. The Python ADK
    /// (`utils/_gemini_schema_util.py`) strips a few constructs Gemini's
    /// `Schema` proto doesn't accept; we mirror those.
    ///
    /// Currently: drops `default`, `pattern`, `example`, `title` recursively;
    /// rejects nested `enum` on non-string types.
    pub fn sanitize_for_gemini(&mut self) -> Result<(), SchemaError> {
        self.default = None;
        self.pattern = None;
        self.example = None;
        self.title = None;
        if let Some(items) = self.items.as_deref_mut() {
            items.sanitize_for_gemini()?;
        }
        for v in self.properties.values_mut() {
            v.sanitize_for_gemini()?;
        }
        if matches!(
            self.r#type,
            Some(SchemaType::Number | SchemaType::Integer | SchemaType::Boolean)
        ) && self.enum_values.as_ref().is_some_and(|v| !v.is_empty())
        {
            return Err(SchemaError::Sanitize(
                "Gemini schema: enum only supported on string types".into(),
            ));
        }
        Ok(())
    }

    /// Convert from a `schemars`-emitted JSON Schema. The conversion is
    /// best-effort: unsupported constructs (`oneOf`, `$ref`, etc.) become
    /// validation errors.
    pub fn from_schemars(schema: &schemars::schema::RootSchema) -> Result<Self, SchemaError> {
        let root_obj = serde_json::to_value(&schema.schema)
            .map_err(|e| SchemaError::Invalid(e.to_string()))?;
        let mut definitions: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for (k, v) in &schema.definitions {
            definitions.insert(
                k.clone(),
                serde_json::to_value(v).map_err(|e| SchemaError::Invalid(e.to_string()))?,
            );
        }
        from_json_schema(&root_obj, &definitions)
    }
}

fn from_json_schema(
    v: &serde_json::Value,
    defs: &BTreeMap<String, serde_json::Value>,
) -> Result<Schema, SchemaError> {
    let obj = v
        .as_object()
        .ok_or_else(|| SchemaError::Invalid("expected object".into()))?;

    // Handle $ref.
    if let Some(r) = obj.get("$ref").and_then(|x| x.as_str()) {
        let name = r
            .strip_prefix("#/definitions/")
            .or_else(|| r.strip_prefix("#/$defs/"))
            .ok_or_else(|| SchemaError::Invalid(format!("unsupported $ref: {r}")))?;
        let target = defs
            .get(name)
            .ok_or_else(|| SchemaError::Invalid(format!("dangling $ref: {r}")))?;
        return from_json_schema(target, defs);
    }

    let mut out = Schema::default();

    if let Some(t) = obj.get("type") {
        out.r#type = Some(parse_schema_type(t)?);
    } else if obj.get("properties").is_some() {
        out.r#type = Some(SchemaType::Object);
    }

    if let Some(d) = obj.get("description").and_then(|x| x.as_str()) {
        out.description = Some(d.to_string());
    }
    if let Some(f) = obj.get("format").and_then(|x| x.as_str()) {
        out.format = Some(f.to_string());
    }
    if let Some(n) = obj.get("nullable").and_then(serde_json::Value::as_bool) {
        out.nullable = Some(n);
    }
    if let Some(e) = obj.get("enum").and_then(|x| x.as_array()) {
        out.enum_values = Some(
            e.iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect(),
        );
    }
    if let Some(it) = obj.get("items") {
        out.items = Some(Box::new(from_json_schema(it, defs)?));
    }
    if let Some(props) = obj.get("properties").and_then(|x| x.as_object()) {
        for (k, v) in props {
            out.properties.insert(k.clone(), from_json_schema(v, defs)?);
        }
    }
    if let Some(req) = obj.get("required").and_then(|x| x.as_array()) {
        for r in req {
            if let Some(s) = r.as_str() {
                out.required.push(s.to_string());
            }
        }
    }
    if let Some(min) = obj.get("minItems").and_then(serde_json::Value::as_u64) {
        out.min_items = Some(min);
    }
    if let Some(max) = obj.get("maxItems").and_then(serde_json::Value::as_u64) {
        out.max_items = Some(max);
    }
    if let Some(min) = obj.get("minLength").and_then(serde_json::Value::as_u64) {
        out.min_length = Some(min);
    }
    if let Some(max) = obj.get("maxLength").and_then(serde_json::Value::as_u64) {
        out.max_length = Some(max);
    }
    if let Some(min) = obj.get("minimum").and_then(serde_json::Value::as_f64) {
        out.minimum = Some(min);
    }
    if let Some(max) = obj.get("maximum").and_then(serde_json::Value::as_f64) {
        out.maximum = Some(max);
    }
    if let Some(p) = obj.get("pattern").and_then(|x| x.as_str()) {
        out.pattern = Some(p.to_string());
    }
    if let Some(t) = obj.get("title").and_then(|x| x.as_str()) {
        out.title = Some(t.to_string());
    }
    Ok(out)
}

fn parse_schema_type(v: &serde_json::Value) -> Result<SchemaType, SchemaError> {
    // schemars sometimes emits arrays like `["string", "null"]` for nullable;
    // pick the non-null type.
    if let Some(s) = v.as_str() {
        return Ok(match s {
            "object" => SchemaType::Object,
            "array" => SchemaType::Array,
            "string" => SchemaType::String,
            "number" => SchemaType::Number,
            "integer" => SchemaType::Integer,
            "boolean" => SchemaType::Boolean,
            other => {
                return Err(SchemaError::Invalid(format!(
                    "unknown JSON Schema type: {other}"
                )));
            }
        });
    }
    if let Some(arr) = v.as_array() {
        for it in arr {
            if let Some(s) = it.as_str() {
                if s != "null" {
                    return parse_schema_type(&serde_json::Value::String(s.to_string()));
                }
            }
        }
    }
    Err(SchemaError::Invalid(format!(
        "unrecognised JSON Schema type: {v}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_object_schema() {
        let s = Schema::object()
            .property("name", Schema::string().with_description("user name"))
            .property("age", Schema::integer())
            .require("name");
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["type"], "OBJECT");
        assert_eq!(j["properties"]["name"]["type"], "STRING");
        assert_eq!(j["required"][0], "name");
    }

    #[test]
    fn schema_round_trips() {
        let s = Schema::array(Schema::string());
        let j = serde_json::to_value(&s).unwrap();
        let back: Schema = serde_json::from_value(j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn sanitize_strips_default_and_pattern() {
        let mut s = Schema::string();
        s.default = Some(json!("x"));
        s.pattern = Some("[a-z]".into());
        s.example = Some(json!("y"));
        s.sanitize_for_gemini().unwrap();
        assert!(s.default.is_none());
        assert!(s.pattern.is_none());
        assert!(s.example.is_none());
    }

    #[test]
    fn sanitize_rejects_enum_on_number() {
        let mut s = Schema::integer();
        s.enum_values = Some(vec!["1".into(), "2".into()]);
        assert!(s.sanitize_for_gemini().is_err());
    }

    #[test]
    fn from_schemars_handles_simple_struct() {
        use schemars::JsonSchema;
        #[derive(JsonSchema)]
        struct Args {
            /// the user's name
            name: String,
            age: u32,
        }
        let args = Args {
            name: "Ada".into(),
            age: 42,
        };
        assert_eq!(args.name, "Ada");
        assert_eq!(args.age, 42);
        let root = schemars::schema_for!(Args);
        let s = Schema::from_schemars(&root).unwrap();
        assert_eq!(s.r#type, Some(SchemaType::Object));
        assert!(s.properties.contains_key("name"));
        assert!(s.properties.contains_key("age"));
    }
}
