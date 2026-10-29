//! Ordered input validation preserving Python's defaults and constraint behavior.
use crate::{Value, json};
use std::{fmt, sync::Arc};

/// Types recognized by the JSON-compatible Python validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    /// Python int, including bool through subclass compatibility.
    Int,
    /// Python float; integer input is converted first.
    Float,
    /// Python bool.
    Bool,
    /// Python str.
    String,
    /// Python list.
    List,
    /// Python dict.
    Dict,
    /// Python NoneType.
    None,
    /// Python tuple of acceptable types.
    AnyOf(Vec<ValueType>),
}
impl ValueType {
    fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Int => value.is_i64() || value.is_u64() || value.is_boolean(),
            Self::Float => value.is_f64(),
            Self::Bool => value.is_boolean(),
            Self::String => value.is_string(),
            Self::List => value.is_array(),
            Self::Dict => value.is_object(),
            Self::None => value.is_null(),
            Self::AnyOf(types) => types.iter().any(|t| t.matches(value)),
        }
    }
    fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::None,
            Value::Bool(_) => Self::Bool,
            Value::String(_) => Self::String,
            Value::Array(_) => Self::List,
            Value::Object(_) => Self::Dict,
            Value::Number(n) if n.is_f64() => Self::Float,
            Value::Number(_) => Self::Int,
        }
    }
}
impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::String => "str",
            Self::List => "list",
            Self::Dict => "dict",
            Self::None => "NoneType",
            Self::AnyOf(types) => {
                return write!(
                    f,
                    "({}{})",
                    types
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    if types.len() == 1 { "," } else { "" }
                );
            }
        };
        write!(f, "<class '{name}'>")
    }
}
type Constraint = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
/// A validation rule; absent fields retain the Python schema-error behavior.
#[derive(Clone, Default)]
pub struct Rule {
    /// Allowed input type, or None to report a missing type.
    pub value_type: Option<ValueType>,
    /// Whether input is required; None reports a missing required declaration.
    pub required: Option<bool>,
    /// Fallback value for omitted optional input, including explicit JSON null.
    pub default: Option<Value>,
    constraint: Option<Constraint>,
}
impl fmt::Debug for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rule")
            .field("value_type", &self.value_type)
            .field("required", &self.required)
            .field("default", &self.default)
            .field("has_constraint", &self.constraint.is_some())
            .finish()
    }
}
impl Rule {
    /// Creates a required input rule.
    pub fn required(value_type: ValueType) -> Self {
        Self {
            value_type: Some(value_type),
            required: Some(true),
            ..Self::default()
        }
    }
    /// Creates an optional input rule with a fallback value.
    pub fn optional(value_type: ValueType, default: Value) -> Self {
        Self {
            value_type: Some(value_type),
            required: Some(false),
            default: Some(default),
            constraint: None,
        }
    }
    /// Adds a predicate for validating an input value or null for a missing field.
    pub fn with_constraint(
        mut self,
        constraint: impl Fn(&Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.constraint = Some(Arc::new(constraint));
        self
    }
}

/// Validates inputs against an ordered schema and returns errors or validated_input.
///
/// As in Python, matching the default value's type bypasses both the declared type
/// check and the constraint. The input object is copied, never modified in place.
pub fn validate(raw_input: &serde_json::Map<String, Value>, schema: &[(String, Rule)]) -> Value {
    let mut validated = raw_input.clone();
    let mut errors = Vec::new();
    for key in raw_input.keys() {
        if !schema.iter().any(|(k, _)| k == key) {
            errors.push(format!(
                "Unexpected input. {key} is not a valid input option."
            ));
        }
    }
    for (key, rule) in schema {
        if rule.value_type.is_none() {
            errors.push(format!("Schema error, missing type for {key}."));
        }
        match rule.required {
            None => errors.push(format!("{key} is a required input.")),
            Some(true) if !raw_input.contains_key(key) => {
                errors.push(format!("{key} is a required input."))
            }
            Some(false) if !raw_input.contains_key(key) => match &rule.default {
                Some(value) => {
                    validated.insert(key.clone(), value.clone());
                }
                None => errors.push(format!("Schema error, missing default value for {key}.")),
            },
            _ => {}
        }
    }
    for (key, rule) in schema {
        if let Some(value) = validated.get_mut(key) {
            if rule.value_type == Some(ValueType::Float)
                && value.is_number()
                && let Some(number) = value.as_f64()
            {
                *value = json!(number);
            }
            if rule
                .default
                .as_ref()
                .is_some_and(|default| ValueType::of(default).matches(value))
            {
                continue;
            }
            if let Some(kind) = &rule.value_type
                && !kind.matches(value)
            {
                errors.push(format!(
                    "{key} should be {kind} type, not {}.",
                    ValueType::of(value)
                ));
            }
        }
        if rule
            .constraint
            .as_ref()
            .is_some_and(|constraint| !constraint(validated.get(key).unwrap_or(&Value::Null)))
        {
            errors.push(format!("{key} does not meet the constraints."));
        }
    }
    if errors.is_empty() {
        json!({"validated_input":validated})
    } else {
        json!({"errors":errors})
    }
}
