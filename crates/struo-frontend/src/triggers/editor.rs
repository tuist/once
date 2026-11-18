use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use super::{load_triggers_toml_str, parse_diagnostic, TriggerEditOperation, TriggerSpec};
use crate::Diagnostic;

/// Apply a batch to root trigger declarations, validating the complete final list.
/// No manifest changes are returned if any operation or final validation fails.
pub fn apply_trigger_operations(
    src: &str,
    ops: &[TriggerEditOperation],
) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|error: toml_edit::TomlError| vec![parse_diagnostic("struo.toml", error)])?;
    if ops.is_empty() {
        load_triggers_toml_str("struo.toml", src)?;
        return Ok(src.to_string());
    }
    ensure_array(&mut doc)?;
    for op in ops {
        match op {
            TriggerEditOperation::Create { trigger } => {
                if index_of(&doc, &trigger.id).is_some() {
                    return Err(vec![Diagnostic::new(
                        "trigger_already_exists",
                        format!("a trigger named `{}` already exists", trigger.id),
                    )
                    .with_target(&trigger.id)
                    .with_attribute("id")
                    .with_repair(
                        "Use update to replace the trigger or choose a distinct id",
                    )]);
                }
                tables_mut(&mut doc).push(trigger_table(trigger));
            }
            TriggerEditOperation::Update { id, trigger } => {
                let index = find_index(&doc, id)?;
                *tables_mut(&mut doc)
                    .get_mut(index)
                    .expect("existing trigger") = trigger_table(trigger);
            }
            TriggerEditOperation::Delete { id } => {
                let index = find_index(&doc, id)?;
                tables_mut(&mut doc).remove(index);
            }
        }
    }
    let edited = doc.to_string();
    load_triggers_toml_str("struo.toml", &edited)?;
    Ok(edited)
}

fn index_of(doc: &DocumentMut, id: &str) -> Option<usize> {
    doc.get("trigger")?
        .as_array_of_tables()?
        .iter()
        .position(|table| table.get("id").and_then(Item::as_str) == Some(id))
}

fn find_index(doc: &DocumentMut, id: &str) -> Result<usize, Vec<Diagnostic>> {
    index_of(doc, id).ok_or_else(|| {
        vec![Diagnostic::new(
            "trigger_not_found",
            format!("no trigger named `{id}` in the root manifest"),
        )
        .with_target(id)
        .with_attribute("id")
        .with_repair(format!(
            "Use an existing id ({}) or create a trigger first",
            doc["trigger"]
                .as_array_of_tables()
                .expect("checked trigger array")
                .iter()
                .filter_map(|table| table.get("id").and_then(Item::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        ))]
    })
}

fn ensure_array(doc: &mut DocumentMut) -> Result<(), Vec<Diagnostic>> {
    match doc.get("trigger") {
        None => {}
        Some(Item::ArrayOfTables(_)) => return Ok(()),
        Some(Item::Value(Value::Array(array))) if array.is_empty() => {}
        Some(_) => {
            return Err(vec![Diagnostic::new(
                "invalid_trigger_section",
                "the top-level trigger key must be an array of tables",
            )
            .with_attribute("trigger")
            .with_repair("Declare triggers with [[trigger]]")]);
        }
    }
    doc.insert("trigger", Item::ArrayOfTables(ArrayOfTables::new()));
    Ok(())
}

fn tables_mut(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    doc.get_mut("trigger")
        .and_then(Item::as_array_of_tables_mut)
        .expect("checked trigger array")
}

fn trigger_table(trigger: &TriggerSpec) -> Table {
    let mut on = InlineTable::new();
    on.insert("event", Value::from(trigger.on.event.as_str()));
    on.insert("refs", Value::Array(strings(&trigger.on.refs)));
    let mut invoke = Array::new();
    for invocation in &trigger.invoke {
        let mut configuration = InlineTable::new();
        configuration.insert("os", Value::from(invocation.configuration.os.as_str()));
        configuration.insert("arch", Value::from(invocation.configuration.arch.as_str()));
        configuration.insert(
            "tokens",
            Value::Array(strings(&invocation.configuration.tokens)),
        );
        let mut member = InlineTable::new();
        member.insert("target", Value::from(invocation.target.as_str()));
        member.insert("capability", Value::from(invocation.capability.as_str()));
        member.insert("configuration", Value::InlineTable(configuration));
        invoke.push(Value::InlineTable(member));
    }
    let mut table = Table::new();
    table.insert("id", toml_edit::value(trigger.id.as_str()));
    table.insert("on", Item::Value(Value::InlineTable(on)));
    table.insert("invoke", Item::Value(Value::Array(invoke)));
    table
}

fn strings(values: &[String]) -> Array {
    values.iter().map(String::as_str).collect()
}
