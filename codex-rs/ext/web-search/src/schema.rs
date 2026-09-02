use codex_api::SearchCommands;
use codex_api::SearchQuery;
use codex_api::SearchResponseLength;
use schemars::JsonSchema;
use schemars::r#gen::SchemaSettings;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by TinyFish dispatch in the next stacked change"
    )
)]
pub(crate) struct TinyFishCommands {
    #[schemars(length(min = 1, max = 4))]
    pub(crate) search_query: Vec<SearchQuery>,
    pub(crate) response_length: Option<SearchResponseLength>,
}

pub(crate) fn commands_schema() -> Value {
    schema_for::<SearchCommands>()
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by TinyFish dispatch in the next stacked change"
    )
)]
pub(crate) fn tinyfish_commands_schema() -> Value {
    let mut schema = schema_for::<TinyFishCommands>();
    let search_query = &mut schema["properties"]["search_query"];
    search_query["minItems"] = Value::from(1);
    search_query["maxItems"] = Value::from(4);
    schema
}

fn schema_for<T: JsonSchema>() -> Value {
    let schema = SchemaSettings::draft2019_09()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.option_add_null_type = false;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let schema = match serde_json::to_value(schema) {
        Ok(schema) => schema,
        Err(err) => panic!("search commands schema should serialize: {err}"),
    };
    let Value::Object(mut schema) = schema else {
        unreachable!("search commands schema must be an object");
    };

    let mut tool_schema = Map::new();
    for key in [
        "properties",
        "required",
        "type",
        "additionalProperties",
        "$defs",
        "definitions",
    ] {
        if let Some(value) = schema.remove(key) {
            tool_schema.insert(key.to_string(), value);
        }
    }
    Value::Object(tool_schema)
}
