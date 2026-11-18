//! `[native-editor]` vim mappings: key notation, defaults, and resolution.
//!
//! Keys are parsed into codemirror-vim's key names (`<C-w>`, `<A-x>`,
//! `<M-k>` for Cmd, `<S-Tab>`, `<Space>`, `<CR>`, single characters), which
//! the editor matches events against and replays to vim.

use std::collections::BTreeMap;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// The default `<leader>`.
pub const DEFAULT_LEADER: &str = "<Space>";
/// The default wait for more keys after a mapping that is also a prefix, in ms (Vim's `timeoutlen`).
pub const DEFAULT_TIMEOUT: u32 = 1000;

/// A vim mode a mapping applies in.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum VimMode {
    Normal,
    Visual,
    Insert,
}

const N: &[VimMode] = &[VimMode::Normal];
const V: &[VimMode] = &[VimMode::Visual];
const I: &[VimMode] = &[VimMode::Insert];
const NV: &[VimMode] = &[VimMode::Normal, VimMode::Visual];

/// Default mappings: Neovim's own (LSP, diagnostics), common community ones
/// (`gd`, `gD`, `gW`, `<leader>f`), and gitsigns.nvim's suggested ones.
const DEFAULT_VIM_KEYS: &[(&str, &str, &[VimMode])] = &[
    ("gd", "editor-go-to-definition", N),
    ("<C-]>", "editor-go-to-definition", N),
    ("gD", "editor-go-to-declaration", N),
    ("K", "editor-hover", N),
    ("grn", "editor-rename", N),
    ("gra", "editor-code-actions", NV),
    ("grr", "editor-find-references", N),
    ("gri", "editor-go-to-implementation", N),
    ("grt", "editor-go-to-type-definition", N),
    ("grx", "editor-run-code-lens", N),
    ("gO", "editor-document-symbols", N),
    ("gW", "editor-workspace-symbols", N),
    ("<C-s>", "editor-signature-help", I),
    ("an", "editor-expand-selection", V),
    ("in", "editor-shrink-selection", V),
    ("]d", "editor-next-diagnostic", N),
    ("[d", "editor-prev-diagnostic", N),
    ("]D", "editor-last-diagnostic", N),
    ("[D", "editor-first-diagnostic", N),
    ("<C-w>d", "editor-show-diagnostics", N),
    ("<C-w><C-d>", "editor-show-diagnostics", N),
    ("]c", "editor-next-hunk", N),
    ("[c", "editor-prev-hunk", N),
    ("<leader>hs", "editor-stage-hunk", NV),
    ("<leader>hr", "editor-reset-hunk", NV),
    ("<leader>hS", "editor-stage-buffer", N),
    ("<leader>hR", "editor-reset-buffer", N),
    ("<leader>hp", "editor-preview-hunk", N),
    ("<leader>hb", "editor-blame-line", N),
    ("<leader>tb", "editor-toggle-line-blame", N),
    ("<leader>f", "editor-format", NV),
    ("<C-o>", "editor-go-back", N),
    ("<C-i>", "editor-go-forward", N),
];

/// Default which-key group names.
const DEFAULT_VIM_GROUPS: &[(&str, &str)] = &[
    ("g", "goto"),
    ("gr", "language server"),
    ("]", "next"),
    ("[", "previous"),
    ("<C-w>", "window"),
    ("<leader>h", "hunk"),
    ("<leader>t", "toggle"),
];

/// What a `[native-editor.vim-keys]` entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum VimTarget {
    /// A btmux or `editor-*` action.
    Action(String),
    /// Keys vim gets instead.
    Keys(String),
    /// Removes the defaults for these keys.
    None,
}

/// A `[native-editor.vim-keys]` value: an action name, or a table.
#[derive(Clone, Debug, PartialEq)]
pub struct VimKey {
    pub target: VimTarget,
    /// Normal mode if unset.
    pub modes: Option<Vec<VimMode>>,
    pub desc: Option<String>,
}

const NONE: &str = "none";

impl<'de> Deserialize<'de> for VimKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Table {
            action: Option<String>,
            keys: Option<String>,
            modes: Option<Vec<VimMode>>,
            desc: Option<String>,
        }

        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = VimKey;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an action name, or a table with `action` or `keys`")
            }

            fn visit_str<E: de::Error>(self, action: &str) -> Result<VimKey, E> {
                Ok(VimKey {
                    target: action_target(action),
                    modes: None,
                    desc: None,
                })
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<VimKey, M::Error> {
                let table = Table::deserialize(de::value::MapAccessDeserializer::new(map))?;
                let target = match (table.action, table.keys) {
                    (Some(action), None) => action_target(&action),
                    (None, Some(keys)) => VimTarget::Keys(keys),
                    _ => return Err(de::Error::custom("set either `action` or `keys`")),
                };
                if table.modes.as_ref().is_some_and(|modes| modes.is_empty()) {
                    return Err(de::Error::custom("`modes` is empty"));
                }
                Ok(VimKey {
                    target,
                    modes: table.modes,
                    desc: table.desc,
                })
            }
        }

        deserializer.deserialize_any(KeyVisitor)
    }
}

fn action_target(action: &str) -> VimTarget {
    if action == NONE {
        VimTarget::None
    } else {
        VimTarget::Action(action.to_string())
    }
}

/// A resolved mapping in one mode, sent to the browser.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct VimBind {
    /// Key names, as codemirror-vim names them.
    pub keys: Vec<String>,
    pub mode: VimMode,
    /// The action run, or…
    pub action: Option<String>,
    /// …the keys vim gets instead.
    pub rhs: Option<Vec<String>>,
    /// Shown in the which-key popup; the action's label if unset.
    pub desc: Option<String>,
}

/// A which-key group: the name shown for a prefix.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct VimGroup {
    pub keys: Vec<String>,
    pub name: String,
}

/// Vim's names for special keys, by lowercase name, as codemirror-vim names them.
fn special_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "space" => "Space",
        "cr" | "enter" | "return" => "CR",
        "esc" | "escape" => "Esc",
        "tab" => "Tab",
        "bs" | "backspace" => "BS",
        "del" | "delete" => "Del",
        "ins" | "insert" => "Ins",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        _ => return None,
    })
}

/// One `<…>` key: `<C-w>`, `<S-Tab>`, `<lt>`, `<D-k>` (Cmd), `<M-x>` (Alt, as in Vim).
fn bracket_key(inner: &str) -> Result<String, String> {
    let (mut ctrl, mut alt, mut meta, mut shift) = (false, false, false, false);
    let mut rest = inner;
    // Modifiers are single letters followed by `-`; a key that is `-` itself ends it (`<C-->`).
    while rest.len() > 2 && rest.as_bytes()[1] == b'-' {
        match rest.as_bytes()[0].to_ascii_lowercase() {
            b'c' => ctrl = true,
            b'a' | b'm' => alt = true,
            b'd' => meta = true,
            b's' => shift = true,
            _ => return Err(format!("unknown modifier in <{inner}>")),
        }
        rest = &rest[2..];
    }
    let lower = rest.to_ascii_lowercase();
    let key = match lower.as_str() {
        "lt" => "<".to_string(),
        "bar" => "|".to_string(),
        "bslash" => "\\".to_string(),
        _ => match special_key(&lower) {
            Some(special) => special.to_string(),
            None if rest.chars().count() == 1 => rest.to_string(),
            None => return Err(format!("unknown key <{inner}>")),
        },
    };
    let single = key.chars().count() == 1;
    let letter = single && key.as_bytes()[0].is_ascii_alphabetic();
    let modified = ctrl || alt || meta;
    if single && !modified {
        // `<S-a>` is `A`; Shift with other characters types another one.
        return match (shift, letter) {
            (false, _) => Ok(key),
            (true, true) => Ok(key.to_ascii_uppercase()),
            (true, false) => Err(format!(
                "<{inner}>: Shift with `{key}` types another character"
            )),
        };
    }
    // With other modifiers, Shift is spelled out and letters follow it.
    let key = match (letter, shift) {
        (true, true) => key.to_ascii_uppercase(),
        (true, false) => key.to_ascii_lowercase(),
        _ => key,
    };
    let mut name = String::from("<");
    for (on, prefix) in [(ctrl, "C-"), (alt, "A-"), (meta, "M-"), (shift, "S-")] {
        if on {
            name.push_str(prefix);
        }
    }
    name.push_str(&key);
    name.push('>');
    Ok(name)
}

/// Parse key notation (`<leader>hs`, `<C-w>d`, `gra`) into key names.
pub fn parse_keys(text: &str, leader: Option<&str>) -> Result<Vec<String>, String> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if ch == '<' {
            if let Some(end) = rest[1..].find('>').map(|i| i + 1) {
                let inner = &rest[1..end];
                // `<` alone, or `<` before other text, is the character.
                if !inner.is_empty() && !inner.contains('<') && !inner.contains(' ') {
                    if inner.eq_ignore_ascii_case("leader") {
                        match leader {
                            Some(leader) => keys.push(leader.to_string()),
                            None => return Err("<leader> can't name itself".into()),
                        }
                    } else {
                        keys.push(bracket_key(inner)?);
                    }
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        keys.push(if ch == ' ' {
            "<Space>".into()
        } else {
            ch.to_string()
        });
        rest = &rest[ch.len_utf8()..];
    }
    if keys.is_empty() {
        return Err("no keys".into());
    }
    Ok(keys)
}

/// `leader` as one key name.
pub fn parse_leader(text: &str) -> Result<String, String> {
    match parse_keys(text, None)?.as_slice() {
        [key] => Ok(key.clone()),
        _ => Err(format!("leader `{text}` must be one key")),
    }
}

/// `<C-s>`, `<F2>`: keys typed in insert mode that don't insert text.
fn is_command_key(key: &str) -> bool {
    key.len() > 1 && key.starts_with('<') && key != "<Space>"
}

/// The vim mappings: defaults, with the user's replacing those on the same keys and modes.
pub fn resolve(leader: &str, user: &BTreeMap<String, VimKey>) -> Result<Vec<VimBind>, String> {
    let mut binds: Vec<VimBind> = Vec::new();
    for (keys, action, modes) in DEFAULT_VIM_KEYS {
        let keys = parse_keys(keys, Some(leader)).expect("default vim keys parse");
        for &mode in *modes {
            binds.push(VimBind {
                keys: keys.clone(),
                mode,
                action: Some((*action).to_string()),
                rhs: None,
                desc: None,
            });
        }
    }
    for (text, key) in user {
        let keys = parse_keys(text, Some(leader)).map_err(|e| format!("vim-keys `{text}`: {e}"))?;
        if keys.iter().any(|k| k == "<Esc>") {
            return Err(format!(
                "vim-keys `{text}`: Esc drops pending keys and hides the editor, so it can't be mapped"
            ));
        }
        let modes = key.modes.clone().unwrap_or_else(|| vec![VimMode::Normal]);
        binds.retain(|b| !(b.keys == keys && modes.contains(&b.mode)));
        let (action, rhs) = match &key.target {
            VimTarget::None => continue,
            VimTarget::Action(action) => {
                if modes.contains(&VimMode::Insert)
                    && !(keys.len() == 1 && is_command_key(&keys[0]))
                {
                    return Err(format!(
                        "vim-keys `{text}`: an insert-mode action needs one key like <C-s>; \
                         map typed keys with `keys` instead"
                    ));
                }
                (Some(action.clone()), None)
            }
            VimTarget::Keys(rhs) => (
                None,
                Some(parse_keys(rhs, Some(leader)).map_err(|e| format!("vim-keys `{text}`: {e}"))?),
            ),
        };
        for mode in modes {
            binds.push(VimBind {
                keys: keys.clone(),
                mode,
                action: action.clone(),
                rhs: rhs.clone(),
                desc: key.desc.clone(),
            });
        }
    }
    Ok(binds)
}

/// Which-key group names: defaults, with the user's replacing those on the same keys.
pub fn resolve_groups(
    leader: &str,
    user: &BTreeMap<String, String>,
) -> Result<Vec<VimGroup>, String> {
    let mut groups: Vec<VimGroup> = DEFAULT_VIM_GROUPS
        .iter()
        .map(|(keys, name)| VimGroup {
            keys: parse_keys(keys, Some(leader)).expect("default vim groups parse"),
            name: (*name).to_string(),
        })
        .collect();
    for (text, name) in user {
        let keys =
            parse_keys(text, Some(leader)).map_err(|e| format!("vim-groups `{text}`: {e}"))?;
        groups.retain(|g| g.keys != keys);
        groups.push(VimGroup {
            keys,
            name: name.clone(),
        });
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        parse_keys(text, Some("<Space>")).unwrap()
    }

    #[test]
    fn notation_parses_to_vim_key_names() {
        assert_eq!(keys("gra"), ["g", "r", "a"]);
        assert_eq!(keys("<leader>hs"), ["<Space>", "h", "s"]);
        assert_eq!(keys("<Leader>f"), ["<Space>", "f"]);
        assert_eq!(keys("<C-w>d"), ["<C-w>", "d"]);
        assert_eq!(keys("<c-W><C-D>"), ["<C-w>", "<C-d>"]);
        assert_eq!(keys("<C-S-x>"), ["<C-S-X>"]);
        assert_eq!(keys("<C-]>"), ["<C-]>"]);
        assert_eq!(keys("<M-x><A-y>"), ["<A-x>", "<A-y>"]);
        assert_eq!(keys("<D-k><D-i>"), ["<M-k>", "<M-i>"]);
        assert_eq!(
            keys("<S-Tab><cr><esc><BS>"),
            ["<S-Tab>", "<CR>", "<Esc>", "<BS>"]
        );
        assert_eq!(keys("<S-a>"), ["A"]);
        assert_eq!(keys("<lt>x<bar>"), ["<", "x", "|"]);
        assert_eq!(keys("<f12>"), ["<F12>"]);
        assert_eq!(keys(" x"), ["<Space>", "x"]);
        assert_eq!(keys("<"), ["<"]);
        assert_eq!(keys("<>"), ["<", ">"]);
        assert_eq!(keys("<C-->"), ["<C-->"]);
        assert!(parse_keys("<X-a>", None).is_err());
        assert!(parse_keys("<nope>", None).is_err());
        assert!(parse_keys("", None).is_err());
        assert!(parse_keys("<leader>", None).is_err());
    }

    #[test]
    fn leaders_are_one_key() {
        assert_eq!(parse_leader(" ").unwrap(), "<Space>");
        assert_eq!(parse_leader("<Space>").unwrap(), "<Space>");
        assert_eq!(parse_leader(",").unwrap(), ",");
        assert_eq!(parse_leader("\\").unwrap(), "\\");
        assert!(parse_leader(",,").is_err());
    }

    fn user(entries: &[(&str, VimKey)]) -> BTreeMap<String, VimKey> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn action(name: &str, modes: Option<Vec<VimMode>>) -> VimKey {
        VimKey {
            target: action_target(name),
            modes,
            desc: None,
        }
    }

    fn find<'a>(binds: &'a [VimBind], text: &str, mode: VimMode) -> Vec<&'a VimBind> {
        let keys = keys(text);
        binds
            .iter()
            .filter(|b| b.keys == keys && b.mode == mode)
            .collect()
    }

    #[test]
    fn user_mappings_replace_defaults_on_the_same_keys_and_modes() {
        let binds = resolve(
            "<Space>",
            &user(&[
                ("gd", action("none", None)),
                (
                    "<leader>f",
                    action("editor-hover", Some(vec![VimMode::Visual])),
                ),
                (
                    "jk",
                    VimKey {
                        target: VimTarget::Keys("<Esc>".into()),
                        modes: Some(vec![VimMode::Insert]),
                        desc: Some("leave insert mode".into()),
                    },
                ),
            ]),
        )
        .unwrap();
        assert!(find(&binds, "gd", VimMode::Normal).is_empty());
        assert_eq!(find(&binds, "<C-]>", VimMode::Normal).len(), 1);
        let normal = find(&binds, "<leader>f", VimMode::Normal);
        assert_eq!(normal[0].action.as_deref(), Some("editor-format"));
        let visual = find(&binds, "<leader>f", VimMode::Visual);
        assert_eq!(visual.len(), 1);
        assert_eq!(visual[0].action.as_deref(), Some("editor-hover"));
        let jk = find(&binds, "jk", VimMode::Insert);
        assert_eq!(jk[0].rhs.as_deref(), Some(&["<Esc>".to_string()][..]));
        assert_eq!(jk[0].desc.as_deref(), Some("leave insert mode"));
    }

    #[test]
    fn the_leader_moves_default_mappings() {
        let binds = resolve(",", &BTreeMap::new()).unwrap();
        assert_eq!(find(&binds, ",hs", VimMode::Normal).len(), 1);
        assert_eq!(find(&binds, ",hs", VimMode::Visual).len(), 1);
    }

    #[test]
    fn insert_mode_actions_need_one_command_key() {
        assert!(resolve(
            "<Space>",
            &user(&[("<C-k>", action("editor-hover", Some(vec![VimMode::Insert])))])
        )
        .is_ok());
        assert!(resolve(
            "<Space>",
            &user(&[("jk", action("editor-hover", Some(vec![VimMode::Insert])))])
        )
        .is_err());
        assert!(resolve("<Space>", &user(&[("<bad>", action("editor-hover", None))])).is_err());
        assert!(resolve(
            "<Space>",
            &user(&[("<Esc>", action("editor-problems", None))])
        )
        .is_err());
        assert!(resolve(
            "<Space>",
            &user(&[("<leader><esc>", action("editor-problems", None))])
        )
        .is_err());
    }

    #[test]
    fn groups_merge_by_keys() {
        let groups = resolve_groups(
            ",",
            &[(String::from("<leader>h"), String::from("git"))].into(),
        )
        .unwrap();
        let hunk: Vec<_> = groups.iter().filter(|g| g.keys == [",", "h"]).collect();
        assert_eq!(hunk.len(), 1);
        assert_eq!(hunk[0].name, "git");
    }

    #[test]
    fn values_parse_from_toml() {
        let map: BTreeMap<String, VimKey> = toml::from_str(
            r#"
            "<leader>ff" = "editor-quick-open"
            "gd" = "none"
            "jk" = { keys = "<Esc>", modes = ["insert"] }
            "<leader>x" = { action = "editor-problems", modes = ["normal", "visual"], desc = "problems" }
            "#,
        )
        .unwrap();
        assert_eq!(map["gd"].target, VimTarget::None);
        assert_eq!(map["jk"].target, VimTarget::Keys("<Esc>".into()));
        assert_eq!(
            map["<leader>x"].modes,
            Some(vec![VimMode::Normal, VimMode::Visual])
        );
        for bad in [
            r#""x" = { action = "a", keys = "b" }"#,
            r#""x" = { desc = "nothing" }"#,
            r#""x" = { action = "a", modes = [] }"#,
            r#""x" = { action = "a", modes = ["replace"] }"#,
            r#""x" = { action = "a", mode = "normal" }"#,
            r#""x" = 1"#,
        ] {
            assert!(
                toml::from_str::<BTreeMap<String, VimKey>>(bad).is_err(),
                "{bad}"
            );
        }
    }
}
