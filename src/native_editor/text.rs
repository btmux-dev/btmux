//! Text changes in CodeMirror's terms: `ChangeSet` JSON over a rope, with
//! offsets in UTF-16 code units, as JavaScript strings count them.

use ropey::Rope;
use serde_json::Value;

/// A replacement of `from..to` (UTF-16 offsets in the document before any
/// change of its set) with `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

pub fn len_utf16(rope: &Rope) -> usize {
    rope.len_utf16_cu()
}

/// Char index of a UTF-16 offset.
pub fn char_at(rope: &Rope, offset: usize) -> usize {
    rope.utf16_cu_to_char(offset)
}

/// Parse `ChangeSet.toJSON()` for a document of `doc_len` code units: a
/// number keeps that many, `[n]` deletes `n`, `[n, ...lines]` replaces `n`
/// with the lines joined by `\n`.
pub fn parse(json: &Value, doc_len: usize) -> Result<Vec<Change>, String> {
    let parts = json.as_array().ok_or("changes must be an array")?;
    let mut changes = Vec::new();
    let mut pos = 0usize;
    for part in parts {
        if let Some(n) = part.as_u64() {
            pos += n as usize;
            continue;
        }
        let part = part.as_array().ok_or("invalid change")?;
        let len = part
            .first()
            .and_then(Value::as_u64)
            .ok_or("invalid change length")? as usize;
        let mut insert = String::new();
        for (i, line) in part[1..].iter().enumerate() {
            if i > 0 {
                insert.push('\n');
            }
            insert.push_str(line.as_str().ok_or("invalid inserted line")?);
        }
        changes.push(Change {
            from: pos,
            to: pos + len,
            insert,
        });
        pos += len;
    }
    if pos != doc_len {
        return Err(format!("changes cover {pos} of {doc_len} code units"));
    }
    Ok(changes)
}

/// `ChangeSet` JSON for sorted, non-overlapping `changes` to a document of
/// `doc_len` code units.
pub fn to_json(changes: &[Change], doc_len: usize) -> Value {
    let mut parts = Vec::new();
    let mut pos = 0;
    for change in changes {
        if change.from > pos {
            parts.push(Value::from(change.from - pos));
        }
        let mut part = vec![Value::from(change.to - change.from)];
        if !change.insert.is_empty() {
            part.extend(change.insert.split('\n').map(Value::from));
        }
        parts.push(Value::Array(part));
        pos = change.to;
    }
    if doc_len > pos {
        parts.push(Value::from(doc_len - pos));
    }
    Value::Array(parts)
}

/// Apply sorted changes; the last first, so earlier offsets stay valid.
pub fn apply(rope: &mut Rope, changes: &[Change]) {
    for change in changes.iter().rev() {
        let from = char_at(rope, change.from);
        let to = char_at(rope, change.to);
        rope.remove(from..to);
        rope.insert(from, &change.insert);
    }
}

/// The smallest single change turning `old` into `new`, if they differ, so a
/// reload keeps cursors outside the changed region.
pub fn diff(old: &Rope, new: &str) -> Option<Change> {
    let new_rope = Rope::from_str(new);
    let mut prefix = 0;
    let mut a = old.chars();
    let mut b = new_rope.chars();
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => prefix += 1,
            _ => break,
        }
    }
    let (old_len, new_len) = (old.len_chars(), new_rope.len_chars());
    if prefix == old_len && prefix == new_len {
        return None;
    }
    let mut suffix = 0;
    let max = old_len.min(new_len) - prefix;
    let mut a = old.chars_at(old_len).reversed();
    let mut b = new_rope.chars_at(new_len).reversed();
    while suffix < max {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => suffix += 1,
            _ => break,
        }
    }
    let from = old.char_to_utf16_cu(prefix);
    let to = old.char_to_utf16_cu(old_len - suffix);
    let insert = new_rope.slice(prefix..new_len - suffix).to_string();
    Some(Change { from, to, insert })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_applies_codemirror_changes() {
        let mut rope = Rope::from_str("hello world");
        // Replace "hello" with "hi\nthere", delete " ", keep "world".
        let changes = parse(&json!([[5, "hi", "there"], [1], 5]), 11).unwrap();
        apply(&mut rope, &changes);
        assert_eq!(rope.to_string(), "hi\nthereworld");
    }

    #[test]
    fn rejects_changes_for_another_length() {
        assert!(parse(&json!([3]), 4).is_err());
        assert!(parse(&json!([[1, 2]]), 1).is_err());
    }

    #[test]
    fn counts_utf16_code_units() {
        // "😀" is two UTF-16 code units and one char.
        let mut rope = Rope::from_str("a😀b");
        assert_eq!(len_utf16(&rope), 4);
        let changes = parse(&json!([3, [1, "c"]]), 4).unwrap();
        apply(&mut rope, &changes);
        assert_eq!(rope.to_string(), "a😀c");
    }

    #[test]
    fn round_trips_json() {
        let changes = vec![
            Change {
                from: 0,
                to: 0,
                insert: "x".into(),
            },
            Change {
                from: 2,
                to: 4,
                insert: "a\nb".into(),
            },
        ];
        let json = to_json(&changes, 6);
        assert_eq!(json, json!([[0, "x"], 2, [2, "a", "b"], 2]));
        assert_eq!(parse(&json, 6).unwrap(), changes);
    }

    #[test]
    fn diffs_to_the_changed_region() {
        let old = Rope::from_str("one\ntwo\nthree\n");
        let change = diff(&old, "one\n2\nthree\n").unwrap();
        assert_eq!(
            change,
            Change {
                from: 4,
                to: 7,
                insert: "2".into()
            }
        );
        assert_eq!(diff(&old, "one\ntwo\nthree\n"), None);
        // Repeated text: the prefix wins, and the suffix doesn't overlap it.
        let change = diff(&Rope::from_str("aa"), "aaa").unwrap();
        assert_eq!(
            (change.from, change.to, change.insert.as_str()),
            (2, 2, "a")
        );
        let mut rope = Rope::from_str("x😀y");
        let change = diff(&rope, "x😀😀y").unwrap();
        apply(&mut rope, &[change]);
        assert_eq!(rope.to_string(), "x😀😀y");
    }
}
