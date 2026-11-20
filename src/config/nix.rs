//! config.toml as Home Manager's `programs.btmux.settings`, which the module
//! turns back into the same TOML with `pkgs.formats.toml`.

use std::fmt::Write;

use toml_edit::{DocumentMut, Item, TableLike, Value};

const KEYWORDS: &[&str] = &[
    "assert", "else", "if", "in", "inherit", "let", "or", "rec", "then", "with",
];

/// `text`'s settings as a Nix assignment, in the file's order.
pub fn settings(text: &str) -> Result<String, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("Invalid TOML: {e}"))?;
    let mut out = String::from(
        "# All of config.toml: replace your programs.btmux.settings with it.\nprograms.btmux.settings = ",
    );
    table(&mut out, doc.as_table(), "")?;
    out.push_str(";\n");
    Ok(out)
}

fn table(out: &mut String, table: &dyn TableLike, indent: &str) -> Result<(), String> {
    let entries: Vec<_> = table.iter().filter(|(_, item)| !item.is_none()).collect();
    if entries.is_empty() {
        out.push_str("{ }");
        return Ok(());
    }
    let inner = format!("{indent}  ");
    out.push_str("{\n");
    for (key, item) in entries {
        let _ = write!(out, "{inner}{} = ", name(key));
        self::item(out, item, &inner)?;
        out.push_str(";\n");
    }
    out.push_str(indent);
    out.push('}');
    Ok(())
}

fn item(out: &mut String, item: &Item, indent: &str) -> Result<(), String> {
    match item {
        Item::None => Ok(()),
        Item::Value(value) => self::value(out, value, indent),
        Item::Table(t) => table(out, t, indent),
        Item::ArrayOfTables(tables) => {
            let inner = format!("{indent}  ");
            out.push_str("[\n");
            for t in tables.iter() {
                out.push_str(&inner);
                table(out, t, &inner)?;
                out.push('\n');
            }
            out.push_str(indent);
            out.push(']');
            Ok(())
        }
    }
}

fn value(out: &mut String, value: &Value, indent: &str) -> Result<(), String> {
    match value {
        Value::String(s) => out.push_str(&string(s.value())),
        Value::Integer(i) => out.push_str(&integer(*i.value())),
        Value::Float(f) => out.push_str(&float(*f.value())?),
        Value::Boolean(b) => out.push_str(if *b.value() { "true" } else { "false" }),
        // `pkgs.formats.toml` has no datetimes, and btmux takes none.
        Value::Datetime(d) => out.push_str(&string(&d.value().to_string())),
        Value::InlineTable(t) => table(out, t, indent)?,
        Value::Array(values) => {
            if values.is_empty() {
                out.push_str("[ ]");
                return Ok(());
            }
            let nested = values
                .iter()
                .any(|v| matches!(v, Value::Array(_) | Value::InlineTable(_)));
            let inner = format!("{indent}  ");
            out.push_str(if nested { "[\n" } else { "[ " });
            for v in values.iter() {
                if nested {
                    out.push_str(&inner);
                }
                let mut element = String::new();
                self::value(&mut element, v, &inner)?;
                // A leading minus would subtract from the previous element.
                if element.starts_with('-') {
                    element = format!("({element})");
                }
                out.push_str(&element);
                out.push(if nested { '\n' } else { ' ' });
            }
            if nested {
                out.push_str(indent);
            }
            out.push(']');
        }
    }
    Ok(())
}

fn name(key: &str) -> String {
    let mut chars = key.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || "_'-".contains(c));
    if identifier && !KEYWORDS.contains(&key) {
        key.to_string()
    } else {
        string(key)
    }
}

fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Nix reads `-9223372036854775808` as negating an out-of-range integer.
fn integer(value: i64) -> String {
    if value == i64::MIN {
        format!("({} - 1)", i64::MIN + 1)
    } else {
        value.to_string()
    }
}

/// A float literal Nix reads as one: always with a dot, which must come
/// before any exponent (`1.0e20`, not `1e20`).
fn float(value: f64) -> Result<String, String> {
    if !value.is_finite() {
        return Err(format!("Nix has no literal for {value}"));
    }
    let text = format!("{value:?}");
    Ok(match text.split_once('e') {
        Some((mantissa, exponent)) if !mantissa.contains('.') => format!("{mantissa}.0e{exponent}"),
        _ => text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_file_order() {
        let nix = settings(
            r#"
# a comment
prefix = "C-b"
repeat-time = 500
terminal.font-size = 16.0

[[wallpaper-shader]]
shader = "aurora"
params = { stops = [[1, -0.5], { a = true }], "a b" = 1 }

[[wallpaper-shader]]
shader = "grid"

[keys]
if = "x"
"#,
        )
        .unwrap();
        assert_eq!(
            nix,
            r#"# All of config.toml: replace your programs.btmux.settings with it.
programs.btmux.settings = {
  prefix = "C-b";
  repeat-time = 500;
  terminal = {
    font-size = 16.0;
  };
  wallpaper-shader = [
    {
      shader = "aurora";
      params = {
        stops = [
          [ 1 (-0.5) ]
          {
            a = true;
          }
        ];
        "a b" = 1;
      };
    }
    {
      shader = "grid";
    }
  ];
  keys = {
    "if" = "x";
  };
};
"#
        );
        assert!(settings("# only comments\n")
            .unwrap()
            .ends_with("\nprograms.btmux.settings = { };\n"));
    }

    #[test]
    fn literals_nix_accepts() {
        assert_eq!(
            string("a\"b\\c ${x} $${y} $ \n\t"),
            r#""a\"b\\c \${x} $\${y} $ \n\t""#
        );
        assert_eq!(float(1e20).unwrap(), "1.0e20");
        assert_eq!(float(1e21).unwrap(), "1.0e21");
        assert_eq!(float(-1.5e-7).unwrap(), "-1.5e-7");
        assert_eq!(float(0.5).unwrap(), "0.5");
        assert_eq!(float(18.0).unwrap(), "18.0");
        assert!(float(f64::INFINITY).is_err());
        assert_eq!(integer(i64::MIN), "(-9223372036854775807 - 1)");
        assert_eq!(name("font-size"), "font-size");
        assert_eq!(name("it's"), "it's");
        assert_eq!(name("1st"), "\"1st\"");
        assert_eq!(name(""), "\"\"");
    }

    /// Evaluates the Nix, when `nix-instantiate` is installed, against the TOML.
    #[test]
    fn evaluates_to_the_same_values() {
        let text = r#"
prefix = "C-b"
shell = "/bin/fish -c \"echo ${HOME} $${x} \\ \n\t\""
wallpaper-opacity = 0.50
min = -9223372036854775808
[terminal]
font-size = 1e20
smooth-scroll-duration = 1e21
[[wallpaper-shader]]
shader = "aurora"
params = { stops = [[1, -0.5, 1e-7], { "a b" = true, if = 2 }], "1st" = -2 }
"#;
        let nix = settings(text).unwrap();
        let expr = nix
            .split_once("programs.btmux.settings = ")
            .map(|(_, s)| s)
            .and_then(|s| s.strip_suffix(";\n"))
            .unwrap();
        let Ok(output) = std::process::Command::new("nix-instantiate")
            .args(["--eval", "--strict", "--json", "-E", expr])
            .output()
        else {
            return;
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Nix may print a whole float without its fraction; types are checked above.
        fn numbers(value: serde_json::Value) -> serde_json::Value {
            use serde_json::Value;
            match value {
                Value::Number(n) => n.as_f64().unwrap().into(),
                Value::Array(a) => a.into_iter().map(numbers).collect(),
                Value::Object(o) => o.into_iter().map(|(k, v)| (k, numbers(v))).collect(),
                v => v,
            }
        }
        let evaluated = serde_json::from_slice(&output.stdout).unwrap();
        let expected = serde_json::to_value(toml::from_str::<toml::Table>(text).unwrap()).unwrap();
        assert_eq!(numbers(evaluated), numbers(expected));
    }
}
