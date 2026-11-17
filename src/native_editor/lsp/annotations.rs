//! Change annotations (`WorkspaceEdit.changeAnnotations`): changes marked
//! `needsConfirmation` are applied only once the user agrees.

use std::collections::HashSet;

use serde_json::Value;

/// Ids of the annotations that need confirmation.
fn confirm_ids(edit: &Value) -> HashSet<String> {
    edit["changeAnnotations"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, a)| a["needsConfirmation"] == true)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Every change that can carry an annotation: text edits and file operations.
fn changes(edit: &Value) -> impl Iterator<Item = &Value> {
    let documents = edit["documentChanges"].as_array().into_iter().flatten();
    let document_changes = documents.flat_map(|c| match c["edits"].as_array() {
        Some(edits) => edits.iter().collect::<Vec<_>>(),
        None => vec![c],
    });
    let plain = edit["changes"].as_object().into_iter().flatten();
    let plain = plain.flat_map(|(_, e)| e.as_array().into_iter().flatten());
    document_changes.chain(plain)
}

/// The labels of the annotations needing confirmation that `edit` uses.
pub fn confirmations(edit: &Value) -> Vec<String> {
    let ids = confirm_ids(edit);
    let mut labels = Vec::new();
    for change in changes(edit) {
        let Some(id) = change["annotationId"].as_str() else {
            continue;
        };
        if !ids.contains(id) {
            continue;
        }
        let label = edit["changeAnnotations"][id]["label"]
            .as_str()
            .unwrap_or(id)
            .to_owned();
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

/// Drop the changes needing confirmation from `edit`; false if none are left.
pub fn skip_confirmations(edit: &mut Value) -> bool {
    let ids = confirm_ids(edit);
    let keep = |change: &Value| {
        !change["annotationId"]
            .as_str()
            .is_some_and(|id| ids.contains(id))
    };
    let mut left = 0;
    if let Some(documents) = edit["documentChanges"].as_array_mut() {
        documents.retain_mut(|c| match c["edits"].as_array_mut() {
            Some(edits) => {
                edits.retain(keep);
                !edits.is_empty()
            }
            None => keep(c),
        });
        left += documents.len();
    } else if let Some(files) = edit["changes"].as_object_mut() {
        files.retain(|_, edits| {
            edits.as_array_mut().map_or(0, |e| {
                e.retain(keep);
                e.len()
            }) > 0
        });
        left += files.len();
    }
    left > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(id: &str) -> Value {
        json!({ "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 0 },
        }, "newText": "x", "annotationId": id })
    }

    #[test]
    fn confirmations_and_skipping() {
        let mut e = json!({
            "documentChanges": [
                { "textDocument": { "uri": "file:///a", "version": 1 },
                  "edits": [edit("ok"), edit("ask")] },
                { "textDocument": { "uri": "file:///b", "version": 1 }, "edits": [edit("ask")] },
                { "kind": "delete", "uri": "file:///c", "annotationId": "ask2" },
            ],
            "changeAnnotations": {
                "ok": { "label": "Rename" },
                "ask": { "label": "External file", "needsConfirmation": true },
                "ask2": { "label": "Delete", "needsConfirmation": true },
                "unused": { "label": "Unused", "needsConfirmation": true },
            },
        });
        assert_eq!(confirmations(&e), ["External file", "Delete"]);
        assert!(skip_confirmations(&mut e));
        assert!(confirmations(&e).is_empty());
        let documents = e["documentChanges"].as_array().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["edits"], json!([edit("ok")]));
    }

    #[test]
    fn nothing_left() {
        let mut e = json!({
            "changes": { "file:///a": [edit("ask")] },
            "changeAnnotations": { "ask": { "label": "Ask", "needsConfirmation": true } },
        });
        assert_eq!(confirmations(&e), ["Ask"]);
        assert!(!skip_confirmations(&mut e));
        assert!(confirmations(&json!({ "changes": { "file:///a": [edit("x")] } })).is_empty());
    }
}
