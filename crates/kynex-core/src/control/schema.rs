//! `knx api schema`: the control protocol as JSON, generated from the types the server and
//! the client use, so it can't drift from them. Needs no running app.

use schemars::schema_for;
use serde_json::{Value, json};

use super::protocol::{Answer, Event, Incoming};

/// Bumped when a change to the protocol breaks a client written against the last.
pub const VERSION: u32 = 1;

fn schema_of(schema: schemars::Schema) -> Value {
    serde_json::to_value(schema).unwrap_or(Value::Null)
}

/// The requests, answers and events, as JSON Schema where it applies.
pub fn schema() -> Value {
    let events: Vec<Value> =
        Event::ALL.iter().map(|e| json!({ "event": e.name(), "description": e.description() })).collect();
    json!({
        "version": VERSION,
        "transport": "A local socket (a named pipe on Windows; the address is in control.addr in the data dir): send one JSON request per line, read one JSON answer per line; after an `events` request the connection streams event lines until it closes.",
        "request": schema_of(schema_for!(Incoming)),
        "response": schema_of(schema_for!(Answer)),
        "events": events,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::protocol::tests::sample_requests;
    use super::*;

    #[test]
    fn the_schema_lists_every_command_and_event() {
        let schema = schema();
        // Valid JSON that survives a round trip through text.
        let text = serde_json::to_string_pretty(&schema).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), schema);
        let cmds: HashSet<String> = sample_requests()
            .iter()
            .map(|r| serde_json::to_value(r).unwrap()["cmd"].as_str().unwrap().to_string())
            .collect();
        for cmd in &cmds {
            assert!(text.contains(&format!("\"{cmd}\"")), "{cmd} is missing from the schema");
        }
        // And nothing the samples don't know of: a new request needs a sample.
        let in_schema = text.matches("\"cmd\": {").count();
        assert_eq!(in_schema, cmds.len(), "a request without a sample in protocol.rs, or the reverse");
        assert_eq!(schema["version"], 1);
        assert!(schema["transport"].as_str().is_some_and(|t| !t.is_empty()));
        assert!(schema["response"].to_string().contains("\"ok\""));
        let events = schema["events"].as_array().unwrap();
        assert_eq!(events.len(), Event::ALL.len());
        assert!(events.iter().any(|e| e["event"] == "turn_end"));
    }
}
