//! Hermes agent configuration: locating `config.yaml` and adding the workspace's
//! shared skills directory to its `skills.external_dirs` key.
//!
//! See `docs/specs/spec-002-hermes.md`. Two properties of this module matter:
//!
//! - **It is pure.** Nothing here reads the environment or the filesystem.
//!   `config_path` takes the home directory; the editing functions take the
//!   document text. Reading, backing up, and writing live in `commands.rs`,
//!   which is also where `--dry-run` is honored — the same split `git.rs` uses.
//! - **It edits text, not a parse tree** (spec §1.3.1). A Hermes configuration
//!   is machine-serialized but hand-annotated: deserializing it to a `Value` and
//!   re-serializing would set the key correctly and destroy every comment in the
//!   file. So only the lines that must change are rewritten, and [`verify`]
//!   proves the result differs from the original by exactly one key.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde_yaml_ng::{Mapping, Value};

/// Directory Hermes keeps its configuration in, under the user's home (spec §2).
const CONFIG_DIR: &str = ".hermes";

/// The configuration file itself.
const CONFIG_FILE: &str = "config.yaml";

/// The top-level mapping holding skills settings.
const SKILLS_KEY: &str = "skills";

/// The sequence within it that lists directories Hermes loads skills from. This
/// is the only key `zpr-dev` ever writes in this file (spec §8).
const EXTERNAL_DIRS_KEY: &str = "external_dirs";

/// One level of indentation for lines this module inserts. Existing lines keep
/// whatever indentation they already have; see [`child_indent`].
const IND: &str = "  ";

/// Where Hermes keeps its configuration, given the user's home directory. Takes
/// `home` rather than reading `$HOME` so the caller owns that failure (spec §6).
pub fn config_path(home: &Path) -> PathBuf {
    home.join(CONFIG_DIR).join(CONFIG_FILE)
}

/// The directories currently listed in `skills.external_dirs`.
///
/// An absent `skills` block, an absent `external_dirs` key, and an explicit null
/// all mean "none listed" — they are states to report, not problems. `Err` is
/// reserved for a document whose shape says this module must not touch it
/// (spec §4.3), so `status` can print the reason and `configure` can refuse.
pub fn external_dirs(text: &str) -> Result<Vec<String>> {
    let root = parse_root(text)?;

    let Some(skills) = root.get(key(SKILLS_KEY)) else {
        return Ok(Vec::new());
    };
    let Some(skills) = skills.as_mapping() else {
        bail!("`{SKILLS_KEY}` is not a mapping");
    };
    let dirs = match skills.get(key(EXTERNAL_DIRS_KEY)) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(value) => value,
    };
    let Some(items) = dirs.as_sequence() else {
        bail!("`{SKILLS_KEY}.{EXTERNAL_DIRS_KEY}` is not a sequence");
    };

    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                anyhow!("`{SKILLS_KEY}.{EXTERNAL_DIRS_KEY}` contains a non-string entry")
            })
        })
        .collect()
}

/// Adds `path` to `skills.external_dirs`, returning the edited document — or
/// `None` when `path` is already listed, which is the idempotent no-op every
/// second run takes.
///
/// `Err` is a refusal to edit (spec §4.3), never an I/O failure. Every refusal
/// message ends with the remedy, because the remedy is always the same: the
/// developer adds two lines by hand.
///
/// The result is always passed through [`verify`] before it is returned, so no
/// caller can skip that check.
pub fn add_external_dir(text: &str, path: &str) -> Result<Option<String>> {
    check_textual_guards(text)?;

    if external_dirs(text)?.iter().any(|dir| dir == path) {
        return Ok(None);
    }

    let edited = insert(text, path)?;
    verify(text, &edited, path)?;
    Ok(Some(edited))
}

/// A `Value` key, spelled once so the accessors below read as prose.
fn key(name: &str) -> Value {
    Value::String(name.to_string())
}

/// Parses the document and requires a mapping at its root. An empty document
/// parses as null and is treated as an empty mapping, so an empty configuration
/// file gains a `skills` block rather than being refused.
fn parse_root(text: &str) -> Result<Mapping> {
    let value: Value =
        serde_yaml_ng::from_str(text).map_err(|err| anyhow!("not valid YAML: {err}"))?;
    match value {
        Value::Mapping(map) => Ok(map),
        Value::Null => Ok(Mapping::new()),
        _ => bail!("the document root is not a mapping"),
    }
}

/// The two guards that are questions about the text rather than the parse tree
/// (spec §4.3). The type guards live in [`external_dirs`], which every caller
/// goes through first.
fn check_textual_guards(text: &str) -> Result<()> {
    if text.contains('\t') {
        bail!("the file contains a tab character; edit the file by hand");
    }
    // A separator on the first line is the ordinary single-document form. One
    // anywhere else means the line scanner below would be reading the wrong
    // document.
    if text.lines().skip(1).any(|line| line.trim_end() == "---") {
        bail!("the file holds more than one YAML document; edit the file by hand");
    }
    Ok(())
}

/// The extent of a top-level key's block: the key's own line, and the exclusive
/// end of the lines beneath it.
struct Block {
    key: usize,
    end: usize,
}

/// True when `line` is the top-level key `name` with nothing but an optional
/// comment after the colon. Matching against the raw line is what enforces
/// column zero, and refusing a line with a value after the colon is what keeps
/// a flow mapping (`skills: {a: 1}`) out of the line scanner.
fn is_top_level_key(line: &str, name: &str) -> bool {
    let Some(rest) = line.strip_prefix(name).and_then(|r| r.strip_prefix(':')) else {
        return false;
    };
    let rest = rest.trim();
    rest.is_empty() || rest.starts_with('#')
}

/// Locates a top-level key and the block beneath it. The block ends at the next
/// line that is non-empty, is not a comment, and has zero indentation — so
/// trailing comments before the next top-level key stay inside the range, which
/// is harmless: a comment is never mistaken for the key being searched for.
fn find_top_level_block(lines: &[&str], name: &str) -> Option<Block> {
    let head = lines.iter().position(|line| is_top_level_key(line, name))?;

    let mut end = head + 1;
    while end < lines.len() {
        let line = lines[end];
        let trimmed = line.trim_start();
        let is_top_level = !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !line.starts_with(char::is_whitespace);
        if is_top_level {
            break;
        }
        end += 1;
    }
    Some(Block { key: head, end })
}

/// The indentation of a block's direct children, taken from the first line that
/// has any rather than assumed to be two spaces (spec §4.2).
fn child_indent(lines: &[&str], block: &Block) -> usize {
    for line in &lines[block.key + 1..block.end] {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return line.len() - trimmed.len();
        }
    }
    IND.len()
}

/// The line index of a direct child key of `block`. The indentation must match
/// exactly, so a key of the same name nested deeper is not mistaken for it.
fn find_child_key(lines: &[&str], block: &Block, indent: usize, name: &str) -> Option<usize> {
    (block.key + 1..block.end).find(|&at| {
        let line = lines[at];
        let trimmed = line.trim_start();
        line.len() - trimmed.len() == indent
            && trimmed
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with(':'))
    })
}

/// Whatever follows `name:` on its line, comment included.
fn value_after_key<'a>(line: &'a str, name: &str) -> &'a str {
    line.trim_start()
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or("")
}

/// Splits a value from its trailing comment. Sound for the values this module
/// inspects — an empty value or a flow sequence — neither of which can contain a
/// `#` inside a string.
fn split_comment(value: &str) -> (&str, &str) {
    match value.find('#') {
        Some(at) => (&value[..at], &value[at..]),
        None => (value, ""),
    }
}

/// The index of the last content line belonging to the block sequence under the
/// key at `at`, and the indentation of its items. `None` when the key has no
/// items beneath it. Blank lines and comments are skipped rather than counted,
/// so an insertion lands after the last real item.
fn last_item(lines: &[&str], at: usize, key_indent: usize) -> Option<(usize, usize)> {
    let mut last = None;
    let mut item_indent = None;

    for (offset, line) in lines.iter().enumerate().skip(at + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= key_indent {
            break;
        }
        if item_indent.is_none() && trimmed.starts_with('-') {
            item_indent = Some(indent);
        }
        last = Some(offset);
    }
    Some((last?, item_indent?))
}

/// One sequence item line, indented by `indent` spaces.
fn item(indent: usize, path: &str) -> String {
    format!("{blank:indent$}- {path}", blank = "")
}

/// Reassembles edited lines, preserving whether the original ended in a newline.
fn join(lines: &[String], trailing_newline: bool) -> String {
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

/// Produces the edited document. The caller has established that `path` is
/// absent and that the textual guards pass.
fn insert(text: &str, path: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let has_skills = parse_root(text)?.contains_key(key(SKILLS_KEY));

    match (has_skills, find_top_level_block(&lines, SKILLS_KEY)) {
        // No `skills:` anywhere: append the whole block.
        (false, None) => Ok(append_block(text, path)),
        (true, Some(block)) => insert_into_block(&lines, &block, path, text.ends_with('\n')),
        // The key is in the document but not as a plain top-level block: a flow
        // mapping, a quoted key, an alias. Adding two lines by hand is a small
        // job; guessing at the shape here is not.
        _ => {
            bail!("`{SKILLS_KEY}` is not written as a plain top-level block; edit the file by hand")
        }
    }
}

/// Appends a whole `skills` block. It lands after any trailing comment block the
/// developer left at the end of the file — harmless, and honest about who wrote
/// it. The blank line keeps it from reading as part of that comment.
fn append_block(text: &str, path: &str) -> String {
    let mut out = text.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&format!("{SKILLS_KEY}:\n{IND}{EXTERNAL_DIRS_KEY}:\n"));
    out.push_str(&item(IND.len() * 2, path));
    out.push('\n');
    out
}

/// The three in-block cases of spec §4.2.
fn insert_into_block(
    lines: &[&str],
    block: &Block,
    path: &str,
    trailing_newline: bool,
) -> Result<String> {
    let indent = child_indent(lines, block);
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();

    // No `external_dirs:` key: it becomes the block's first entry.
    let Some(at) = find_child_key(lines, block, indent, EXTERNAL_DIRS_KEY) else {
        let head = block.key + 1;
        out.splice(
            head..head,
            [
                format!("{blank:indent$}{EXTERNAL_DIRS_KEY}:", blank = ""),
                item(indent + IND.len(), path),
            ],
        );
        return Ok(join(&out, trailing_newline));
    };

    let (value, comment) = split_comment(value_after_key(lines[at], EXTERNAL_DIRS_KEY));
    let comment = if comment.is_empty() {
        String::new()
    } else {
        format!("  {comment}")
    };

    match value.trim() {
        // `external_dirs: []`, which is what Hermes writes when the list is
        // empty: the flow sequence becomes a block sequence holding one item.
        "[]" => {
            out[at] = format!("{blank:indent$}{EXTERNAL_DIRS_KEY}:{comment}", blank = "");
            out.insert(at + 1, item(indent + IND.len(), path));
        }
        // Nothing but an optional comment: a block sequence, or an empty value.
        "" => match last_item(lines, at, indent) {
            Some((last, item_indent)) => out.insert(last + 1, item(item_indent, path)),
            None => out.insert(at + 1, item(indent + IND.len(), path)),
        },
        // A populated flow sequence, an anchor, an alias: not ours to rewrite.
        other => bail!(
            "`{SKILLS_KEY}.{EXTERNAL_DIRS_KEY}` is written as `{other}`, \
             which this tool will not rewrite; edit the file by hand"
        ),
    }
    Ok(join(&out, trailing_newline))
}

/// Proves the edit changed exactly one key (spec §4.4). This is what makes a
/// text edit as safe as a parse-and-re-emit round-trip, and it is a separate
/// function so it can be tested against edits [`insert`] would never produce.
///
/// 1. The edited text parses as YAML.
/// 2. Its `skills.external_dirs` lists `path`.
/// 3. Reverting only that key yields a document deep-equal to the original.
///
/// Check 3 is the load-bearing one: it rules out a changed neighboring value, a
/// changed nesting level, and a changed key type.
fn verify(original: &str, edited: &str, path: &str) -> Result<()> {
    let old_root = parse_root(original)?;
    let mut new_root = parse_root(edited)?;

    if !external_dirs(edited)?.iter().any(|dir| dir == path) {
        bail!("refusing to write: the edit did not add {path} (this is a bug; please report it)");
    }

    revert_external_dirs(&mut new_root, &old_root);
    if new_root != old_root {
        bail!(
            "refusing to write: the edit changed more than \
             {SKILLS_KEY}.{EXTERNAL_DIRS_KEY} (this is a bug; please report it)"
        );
    }
    Ok(())
}

/// Puts `old`'s `skills.external_dirs` back into `new`, removing the key — and
/// the `skills` block with it, if `old` had none — when `old` did not have it.
/// After this, `new` should equal `old` in every respect.
fn revert_external_dirs(new: &mut Mapping, old: &Mapping) {
    let (skills, dirs) = (key(SKILLS_KEY), key(EXTERNAL_DIRS_KEY));

    let old_skills = old.get(&skills);
    let old_dirs = old_skills
        .and_then(Value::as_mapping)
        .and_then(|map| map.get(&dirs))
        .cloned();

    let Some(Value::Mapping(new_skills)) = new.get_mut(&skills) else {
        return;
    };
    match old_dirs {
        Some(value) => {
            new_skills.insert(dirs, value);
        }
        None => {
            new_skills.remove(&dirs);
        }
    }
    if old_skills.is_none() && new_skills.is_empty() {
        new.remove(&skills);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small stand-in for a real Hermes configuration: machine-serialized keys,
    /// the `_config_version` Hermes maintains, an inline comment, and a trailing
    /// commented-out block. Enough to exercise the shape; not a copy of anyone's
    /// configuration.
    const CONFIG: &str = "\
# Hermes configuration.
model:
  default: claude-fable-5
agent:
  max_turns: 150
skills:
  external_dirs: []
  template_vars: true    # comment inside the block
_config_version: 38

# \u{2500}\u{2500} Fallback Model \u{2500}\u{2500}
# Uncomment to enable.
# fallback_model:
#   provider: openrouter
";

    const PATH: &str = "/home/dev/src/zl_zpr/zl-zpr-dev-context/skills";

    /// The edited document, asserting that an edit happened.
    fn edited(text: &str) -> String {
        add_external_dir(text, PATH)
            .expect("edit refused")
            .expect("no edit produced")
    }

    // ---- §4.2: the five cases -------------------------------------------

    #[test]
    fn empty_flow_sequence_becomes_a_block_sequence() {
        let out = edited(CONFIG);
        assert!(
            out.contains(&format!("  external_dirs:\n    - {PATH}\n")),
            "{out}"
        );
        assert!(!out.contains("external_dirs: []"), "{out}");
        assert_eq!(external_dirs(&out).unwrap(), vec![PATH.to_string()]);
    }

    #[test]
    fn missing_external_dirs_key_becomes_the_blocks_first_entry() {
        let text = CONFIG.replace("  external_dirs: []\n", "");
        let out = edited(&text);
        assert!(
            out.contains(&format!("skills:\n  external_dirs:\n    - {PATH}\n")),
            "{out}"
        );
        // The sibling key is still there, and still in the block.
        assert!(out.contains("  template_vars: true"), "{out}");
    }

    #[test]
    fn missing_skills_block_is_appended() {
        let text = CONFIG
            .replace("skills:\n", "")
            .replace("  external_dirs: []\n", "")
            .replace("  template_vars: true    # comment inside the block\n", "");
        let out = edited(&text);

        assert!(
            out.ends_with(&format!("skills:\n  external_dirs:\n    - {PATH}\n")),
            "{out}"
        );
        // Appended after the trailing comment block, which survives intact.
        assert!(out.contains("# Uncomment to enable."), "{out}");
        assert_eq!(external_dirs(&out).unwrap(), vec![PATH.to_string()]);
    }

    #[test]
    fn existing_item_gains_a_sibling_and_keeps_the_first() {
        let text = CONFIG.replace("  external_dirs: []", "  external_dirs:\n    - /opt/skills");
        let out = edited(&text);
        assert_eq!(
            external_dirs(&out).unwrap(),
            vec!["/opt/skills".to_string(), PATH.to_string()]
        );
        // Inserted after the last item, not before it, and not outside the key.
        assert!(
            out.contains(&format!("    - /opt/skills\n    - {PATH}\n  template_vars")),
            "{out}"
        );
    }

    #[test]
    fn a_path_already_present_is_a_no_op() {
        let out = edited(CONFIG);
        assert_eq!(add_external_dir(&out, PATH).unwrap(), None);
    }

    // ---- §4.2: indentation and comment handling --------------------------

    #[test]
    fn indentation_is_taken_from_the_block_not_assumed() {
        let text = "skills:\n    template_vars: true\n";
        let out = edited(text);
        assert_eq!(
            out,
            format!("skills:\n    external_dirs:\n      - {PATH}\n    template_vars: true\n")
        );
    }

    #[test]
    fn a_comment_on_the_external_dirs_line_survives() {
        let text = "skills:\n  external_dirs: []  # set by hand once\n";
        let out = edited(text);
        assert_eq!(
            out,
            format!("skills:\n  external_dirs:  # set by hand once\n    - {PATH}\n")
        );
    }

    #[test]
    fn an_empty_external_dirs_value_gains_an_item() {
        let text = "skills:\n  external_dirs:\n  template_vars: true\n";
        let out = edited(text);
        assert_eq!(
            out,
            format!("skills:\n  external_dirs:\n    - {PATH}\n  template_vars: true\n")
        );
    }

    #[test]
    fn every_comment_in_the_document_is_preserved_byte_for_byte() {
        let out = edited(CONFIG);
        for line in CONFIG.lines().filter(|l| l.trim_start().starts_with('#')) {
            assert!(out.contains(line), "lost comment {line:?} from\n{out}");
        }
        assert!(out.contains("    # comment inside the block"), "{out}");
        assert!(out.contains("_config_version: 38"), "{out}");
    }

    #[test]
    fn a_document_without_a_trailing_newline_does_not_gain_one() {
        let text = "skills:\n  external_dirs: []";
        let out = edited(text);
        assert_eq!(out, format!("skills:\n  external_dirs:\n    - {PATH}"));
    }

    // ---- §4.3: guards ---------------------------------------------------

    /// Every guard refuses, and every refusal names the remedy.
    #[track_caller]
    fn refuses(text: &str, expected: &str) {
        let err = add_external_dir(text, PATH).expect_err("expected a refusal");
        let message = format!("{err:#}");
        assert!(
            message.contains(expected),
            "message {message:?} does not mention {expected:?}"
        );
        assert!(
            message.contains("edit the file by hand"),
            "refusal without a remedy: {message}"
        );
    }

    #[test]
    fn a_tab_character_is_refused() {
        refuses("skills:\n\texternal_dirs: []\n", "tab character");
    }

    #[test]
    fn a_second_document_is_refused() {
        refuses(
            "skills:\n  external_dirs: []\n---\nother: 1\n",
            "more than one YAML document",
        );
    }

    #[test]
    fn a_populated_flow_sequence_is_refused() {
        refuses(
            "skills:\n  external_dirs: [/opt/skills]\n",
            "written as `[/opt/skills]`",
        );
    }

    #[test]
    fn a_flow_mapping_for_skills_is_refused() {
        refuses(
            "skills: {external_dirs: []}\n",
            "not written as a plain top-level block",
        );
    }

    #[test]
    fn a_non_mapping_root_is_refused() {
        assert!(add_external_dir("- one\n- two\n", PATH).is_err());
    }

    #[test]
    fn invalid_yaml_is_refused() {
        assert!(add_external_dir("skills: [\n", PATH).is_err());
    }

    #[test]
    fn a_non_sequence_external_dirs_is_refused() {
        assert!(add_external_dir("skills:\n  external_dirs: nope\n", PATH).is_err());
    }

    #[test]
    fn a_non_string_entry_is_refused() {
        assert!(add_external_dir("skills:\n  external_dirs:\n    - 7\n", PATH).is_err());
    }

    #[test]
    fn a_non_mapping_skills_is_refused() {
        assert!(add_external_dir("skills:\n  - one\n", PATH).is_err());
    }

    // ---- §4.4: verification ---------------------------------------------

    /// `verify` is what makes the text edit safe, so it is tested directly
    /// against edits `insert` would never produce. Reached no other way: by
    /// construction `add_external_dir` only ever offers it a correct edit.
    #[test]
    fn verification_rejects_an_edit_that_changes_anything_else() {
        let original = "model:\n  default: a\nskills:\n  external_dirs: []\n";
        let good = format!("model:\n  default: a\nskills:\n  external_dirs:\n    - {PATH}\n");
        assert!(verify(original, &good, PATH).is_ok());

        // A neighboring value changed as well.
        let bad = good.replace("default: a", "default: b");
        assert!(verify(original, &bad, PATH).is_err());

        // A key removed as well.
        let bad = good.replace("model:\n  default: a\n", "");
        assert!(verify(original, &bad, PATH).is_err());

        // A key's type changed as well.
        let bad = good.replace("  default: a", "  default: 7");
        assert!(verify(original, &bad, PATH).is_err());

        // A nesting level changed as well.
        let bad = good.replace("model:\n  default: a", "model:\n  inner:\n    default: a");
        assert!(verify(original, &bad, PATH).is_err());

        // The path not added at all.
        assert!(verify(original, original, PATH).is_err());
    }

    #[test]
    fn verification_accepts_an_appended_block() {
        let original = "model:\n  default: a\n";
        let good = format!("model:\n  default: a\n\nskills:\n  external_dirs:\n    - {PATH}\n");
        assert!(verify(original, &good, PATH).is_ok());
    }

    // ---- reading state --------------------------------------------------

    #[test]
    fn external_dirs_reports_every_empty_shape_as_empty() {
        for text in [
            "model:\n  default: a\n",
            "skills:\n  template_vars: true\n",
            "skills:\n  external_dirs: []\n",
            "skills:\n  external_dirs:\n",
            "",
        ] {
            assert_eq!(
                external_dirs(text).unwrap(),
                Vec::<String>::new(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn external_dirs_reports_block_items_in_order() {
        let text = "skills:\n  external_dirs:\n    - /a\n    - /b\n";
        assert_eq!(
            external_dirs(text).unwrap(),
            vec!["/a".to_string(), "/b".to_string()]
        );
    }

    #[test]
    fn config_path_is_under_the_given_home() {
        assert_eq!(
            config_path(Path::new("/home/dev")),
            PathBuf::from("/home/dev/.hermes/config.yaml")
        );
    }
}
