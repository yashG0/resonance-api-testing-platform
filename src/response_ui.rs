//! Response syntax highlighting, JSON tree construction, and formatting helpers.

use std::collections::HashSet;

use slint::{Color, ModelRc, VecModel};

use crate::{AutoPairAction, JsonLine, JsonToken, JsonTreeNode, MAX_STRUCTURED_VIEW_BYTES};

const MAX_VISIBLE_JSON_TREE_NODES: usize = 2_000;

#[derive(Clone, Copy)]
struct SyntaxColors {
    base: Color,
    key: Color,
    string: Color,
    number: Color,
    boolean: Color,
    punctuation: Color,
    comment: Color,
}

fn push_syntax_token(tokens: &mut Vec<JsonToken>, text: &str, color: Color) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = tokens.last_mut()
        && last.color == color
    {
        let mut merged = last.text.to_string();
        merged.push_str(text);
        last.text = merged.into();
    } else {
        tokens.push(JsonToken {
            text: text.into(),
            color,
        });
    }
}

fn tokenize_json_line(line: &str, colors: SyntaxColors) -> Vec<JsonToken> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let byte = bytes[i];
        let color;

        if byte == b'"' {
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => {
                        i += 1;
                        if i < bytes.len() {
                            i += line[i..].chars().next().unwrap().len_utf8();
                        }
                    }
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += line[i..].chars().next().unwrap().len_utf8(),
                }
            }
            let mut lookahead = i;
            while lookahead < bytes.len() {
                let character = line[lookahead..].chars().next().unwrap();
                if !character.is_whitespace() {
                    break;
                }
                lookahead += character.len_utf8();
            }
            color = if lookahead < bytes.len() && bytes[lookahead] == b':' {
                colors.key
            } else {
                colors.string
            };
        } else if byte.is_ascii_digit()
            || (byte == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                i += 1;
            }
            color = colors.number;
        } else if byte.is_ascii_alphabetic() {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            color = match &line[start..i] {
                "true" | "false" => colors.boolean,
                "null" => colors.comment,
                _ => colors.base,
            };
        } else {
            i += line[i..].chars().next().unwrap().len_utf8();
            color = if matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':') {
                colors.punctuation
            } else {
                colors.base
            };
        }
        push_syntax_token(&mut tokens, &line[start..i], color);
    }
    tokens
}

fn tokenize_markup_line(line: &str, colors: SyntaxColors) -> Vec<JsonToken> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|end| end + 3).unwrap_or(rest.len());
            push_syntax_token(&mut tokens, &rest[..end], colors.comment);
            i += end;
            continue;
        }
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            push_syntax_token(&mut tokens, &rest[..end], colors.base);
            i += end;
            continue;
        }

        push_syntax_token(&mut tokens, "<", colors.punctuation);
        i += 1;
        if i >= line.len() {
            break;
        }
        if line.as_bytes()[i] == b'/' || line.as_bytes()[i] == b'!' || line.as_bytes()[i] == b'?' {
            push_syntax_token(&mut tokens, &line[i..i + 1], colors.punctuation);
            i += 1;
        }
        let tag_start = i;
        while i < line.len() {
            let byte = line.as_bytes()[i];
            if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
                break;
            }
            i += line[i..].chars().next().unwrap().len_utf8();
        }
        push_syntax_token(&mut tokens, &line[tag_start..i], colors.key);

        while i < line.len() {
            let whitespace_start = i;
            while i < line.len() && line.as_bytes()[i].is_ascii_whitespace() {
                i += 1;
            }
            push_syntax_token(&mut tokens, &line[whitespace_start..i], colors.base);
            if i >= line.len() {
                break;
            }
            match line.as_bytes()[i] {
                b'>' => {
                    push_syntax_token(&mut tokens, ">", colors.punctuation);
                    i += 1;
                    break;
                }
                b'/' => {
                    push_syntax_token(&mut tokens, "/", colors.punctuation);
                    i += 1;
                }
                b'=' => {
                    push_syntax_token(&mut tokens, "=", colors.punctuation);
                    i += 1;
                    let space_start = i;
                    while i < line.len() && line.as_bytes()[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    push_syntax_token(&mut tokens, &line[space_start..i], colors.base);
                    if i < line.len() && matches!(line.as_bytes()[i], b'\'' | b'"') {
                        let quote = line.as_bytes()[i];
                        let value_start = i;
                        i += 1;
                        while i < line.len() && line.as_bytes()[i] != quote {
                            i += line[i..].chars().next().unwrap().len_utf8();
                        }
                        if i < line.len() {
                            i += 1;
                        }
                        push_syntax_token(&mut tokens, &line[value_start..i], colors.string);
                    }
                }
                _ => {
                    let attr_start = i;
                    while i < line.len()
                        && !line.as_bytes()[i].is_ascii_whitespace()
                        && !matches!(line.as_bytes()[i], b'=' | b'/' | b'>')
                    {
                        i += line[i..].chars().next().unwrap().len_utf8();
                    }
                    if attr_start == i {
                        break;
                    }
                    push_syntax_token(&mut tokens, &line[attr_start..i], colors.boolean);
                }
            }
        }
    }
    tokens
}

fn append_visual_line(lines: &mut Vec<JsonLine>, number: i32, tokens: Vec<JsonToken>) {
    lines.push(JsonLine {
        number,
        tokens: ModelRc::new(VecModel::from(tokens)),
    });
}

fn clamp_char_boundary(text: &str, offset: i32) -> usize {
    let mut offset = (offset.max(0) as usize).min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn inside_json_string(text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string && escaped {
            escaped = false;
        } else if in_string && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            in_string = !in_string;
        }
    }
    in_string
}

pub(crate) fn auto_pair_request_body(
    text: &str,
    cursor: i32,
    anchor: i32,
    key: &str,
    format: &str,
) -> AutoPairAction {
    let cursor = clamp_char_boundary(text, cursor);
    let anchor = clamp_char_boundary(text, anchor);
    let start = cursor.min(anchor);
    let end = cursor.max(anchor);
    let selection = &text[start..end];

    if key == "Enter" {
        let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
        let line_prefix = &text[line_start..start];
        let mut indent: String = line_prefix
            .chars()
            .take_while(|ch| matches!(ch, ' ' | '\t'))
            .collect();
        if format == "JSON" && matches!(line_prefix.trim_end().chars().last(), Some('{' | '[')) {
            indent.push_str("    ");
        }
        let mut updated = String::with_capacity(text.len() + 1 + indent.len());
        updated.push_str(&text[..start]);
        updated.push('\n');
        updated.push_str(&indent);
        updated.push_str(&text[end..]);
        let next_cursor = start + 1 + indent.len();
        return AutoPairAction {
            handled: true,
            text: updated.into(),
            anchor: next_cursor as i32,
            cursor: next_cursor as i32,
        };
    }

    if key == "Tab" {
        if start != end {
            return AutoPairAction {
                handled: false,
                text: text.into(),
                anchor: anchor as i32,
                cursor: cursor as i32,
            };
        }
        let mut updated = String::with_capacity(text.len() + 4);
        updated.push_str(&text[..start]);
        updated.push_str("    ");
        updated.push_str(&text[start..]);
        let next_cursor = start + 4;
        return AutoPairAction {
            handled: true,
            text: updated.into(),
            anchor: next_cursor as i32,
            cursor: next_cursor as i32,
        };
    }

    if start == end && matches!(key, "}" | "]" | ")" | ">" | "\"" | "'") {
        let next_closer = if matches!(key, "\"" | "'") {
            text[start..]
                .chars()
                .next()
                .filter(|next| key.starts_with(*next))
                .map(|next| (0, next))
        } else {
            text[start..]
                .char_indices()
                .find(|(_, ch)| !ch.is_whitespace())
                .filter(|(_, next)| key.starts_with(*next))
        };
        if let Some((relative_offset, next)) = next_closer {
            let next_cursor = start + relative_offset + next.len_utf8();
            return AutoPairAction {
                handled: true,
                text: text.into(),
                anchor: next_cursor as i32,
                cursor: next_cursor as i32,
            };
        }
    }

    let (opener, closer) = match key {
        "{" => ("{", "}"),
        "[" => ("[", "]"),
        "(" if format != "JSON" => ("(", ")"),
        "\"" => ("\"", "\""),
        "'" if format != "JSON" => ("'", "'"),
        "<" if matches!(format, "HTML" | "XML") => ("<", ">"),
        _ => {
            return AutoPairAction {
                handled: false,
                text: text.into(),
                anchor: anchor as i32,
                cursor: cursor as i32,
            };
        }
    };

    if format == "JSON" && opener != "\"" && inside_json_string(&text[..start]) {
        return AutoPairAction {
            handled: false,
            text: text.into(),
            anchor: anchor as i32,
            cursor: cursor as i32,
        };
    }
    if format == "JSON" && opener == "\"" && inside_json_string(&text[..start]) {
        return AutoPairAction {
            handled: false,
            text: text.into(),
            anchor: anchor as i32,
            cursor: cursor as i32,
        };
    }

    let (updated, selection_start, selection_end) =
        if start == end && format == "JSON" && matches!(opener, "{" | "[") {
            let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
            let line_prefix = &text[line_start..start];
            let base_indent: String = line_prefix
                .chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .collect();
            let inner_indent = format!("{base_indent}    ");
            let insertion = format!("{opener}\n{inner_indent}\n{base_indent}{closer}");
            let new_cursor = start + opener.len() + 1 + inner_indent.len();
            let mut updated = String::with_capacity(text.len() + insertion.len());
            updated.push_str(&text[..start]);
            updated.push_str(&insertion);
            updated.push_str(&text[end..]);
            (updated, new_cursor, new_cursor)
        } else {
            let mut updated = String::with_capacity(text.len() + opener.len() + closer.len());
            updated.push_str(&text[..start]);
            updated.push_str(opener);
            updated.push_str(selection);
            updated.push_str(closer);
            updated.push_str(&text[end..]);
            let selected_start = start + opener.len();
            (updated, selected_start, selected_start + selection.len())
        };

    AutoPairAction {
        handled: true,
        text: updated.into(),
        anchor: selection_start as i32,
        cursor: selection_end as i32,
    }
}

#[cfg(test)]
mod auto_pair_tests {
    use super::auto_pair_request_body;

    #[test]
    fn json_object_opener_places_caret_on_indented_middle_line() {
        let action = auto_pair_request_body("", 0, 0, "{", "JSON");
        assert!(action.handled);
        assert_eq!(action.text, "{\n    \n}");
        assert_eq!(action.cursor, 6);
        assert_eq!(action.anchor, action.cursor);
    }

    #[test]
    fn opening_bracket_wraps_a_selection() {
        let action = auto_pair_request_body("name", 4, 0, "[", "JSON");
        assert!(action.handled);
        assert_eq!(action.text, "[name]");
        assert_eq!((action.anchor, action.cursor), (1, 5));
    }

    #[test]
    fn quote_inside_json_string_is_not_auto_paired() {
        let action = auto_pair_request_body("\"name", 5, 5, "\"", "JSON");
        assert!(!action.handled);
    }

    #[test]
    fn closer_skips_existing_close_after_blank_inner_line() {
        let text = "{\n    \n}";
        let action = auto_pair_request_body(text, 6, 6, "}", "JSON");
        assert!(action.handled);
        assert_eq!(action.text, text);
        assert_eq!((action.anchor, action.cursor), (8, 8));
    }

    #[test]
    fn tab_inserts_four_spaces_and_advances_the_cursor() {
        let action = auto_pair_request_body("{}", 1, 1, "Tab", "JSON");
        assert!(action.handled);
        assert_eq!(action.text, "{    }");
        assert_eq!((action.anchor, action.cursor), (5, 5));
    }

    #[test]
    fn enter_keeps_the_existing_property_indent() {
        let text = "    \"name\": \"yash\",";
        let action =
            auto_pair_request_body(text, text.len() as i32, text.len() as i32, "Enter", "JSON");
        assert!(action.handled);
        assert_eq!(action.text, "    \"name\": \"yash\",\n    ");
        assert_eq!(action.cursor, action.text.len() as i32);
    }

    #[test]
    fn enter_increases_indent_after_an_open_json_bracket() {
        let text = "    {";
        let action =
            auto_pair_request_body(text, text.len() as i32, text.len() as i32, "Enter", "JSON");
        assert_eq!(action.text, "    {\n        ");
        assert_eq!(action.cursor, action.text.len() as i32);
    }
}

fn is_wrap_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | ':' | '/' | '-' | '+' | '.')
}

pub(crate) fn tokenize_response(
    source: &str,
    format: &str,
    dark: bool,
    max_columns: f32,
) -> ModelRc<JsonLine> {
    let colors = SyntaxColors {
        base: if dark {
            Color::from_rgb_u8(0xe6, 0xe6, 0xe6)
        } else {
            Color::from_rgb_u8(0x24, 0x1f, 0x31)
        },
        key: if dark {
            Color::from_rgb_u8(0x78, 0xae, 0xed)
        } else {
            Color::from_rgb_u8(0x1c, 0x71, 0xd8)
        },
        string: if dark {
            Color::from_rgb_u8(0x8f, 0xf0, 0xa4)
        } else {
            Color::from_rgb_u8(0x26, 0xa2, 0x69)
        },
        number: if dark {
            Color::from_rgb_u8(0xff, 0xbe, 0x6f)
        } else {
            Color::from_rgb_u8(0xc6, 0x46, 0x00)
        },
        boolean: if dark {
            Color::from_rgb_u8(0xdc, 0x8a, 0xdd)
        } else {
            Color::from_rgb_u8(0x81, 0x3d, 0x9c)
        },
        punctuation: if dark {
            Color::from_rgb_u8(0xc0, 0xbf, 0xbc)
        } else {
            Color::from_rgb_u8(0x5e, 0x5c, 0x64)
        },
        comment: if dark {
            Color::from_rgb_u8(0xa8, 0xa8, 0xa8)
        } else {
            Color::from_rgb_u8(0x6c, 0x6a, 0x73)
        },
    };
    let source_lines: Vec<&str> = if source.is_empty() {
        vec![""]
    } else {
        source.lines().collect()
    };
    let mut lines = Vec::new();
    let max_columns = max_columns.round().max(0.0) as usize;

    for (line_index, line) in source_lines.into_iter().enumerate() {
        let tokens = match format {
            "JSON" => tokenize_json_line(line, colors),
            "HTML" | "XML" => tokenize_markup_line(line, colors),
            _ => vec![JsonToken {
                text: line.into(),
                color: colors.base,
            }],
        };
        if max_columns == 0 {
            append_visual_line(&mut lines, (line_index + 1) as i32, tokens);
            continue;
        }

        let mut current = Vec::new();
        let mut column = 0usize;
        let mut first_visual_line = true;
        for token in tokens {
            // For every position, the previous implementation rescanned the
            // remaining token to discover whether a wrap boundary followed.
            // Finding the final boundary once makes this pass linear in token
            // length, including large unbroken strings and URLs.
            let last_boundary = token
                .text
                .char_indices()
                .rev()
                .find_map(|(offset, character)| is_wrap_boundary(character).then_some(offset));
            let mut segment_start = 0;
            let mut segment_end = 0;
            for (offset, character) in token.text.char_indices() {
                let boundary = is_wrap_boundary(character);
                let has_later_boundary = last_boundary.is_some_and(|boundary| boundary >= offset);

                if column >= max_columns && boundary {
                    segment_end = offset + character.len_utf8();
                    if segment_end > segment_start {
                        push_syntax_token(
                            &mut current,
                            &token.text[segment_start..segment_end],
                            token.color,
                        );
                    }
                    append_visual_line(
                        &mut lines,
                        if first_visual_line {
                            (line_index + 1) as i32
                        } else {
                            0
                        },
                        std::mem::take(&mut current),
                    );
                    first_visual_line = false;
                    column = 0;
                    segment_start = segment_end;
                    continue;
                }

                if column >= max_columns && !has_later_boundary {
                    if segment_end > segment_start {
                        push_syntax_token(
                            &mut current,
                            &token.text[segment_start..segment_end],
                            token.color,
                        );
                    }
                    append_visual_line(
                        &mut lines,
                        if first_visual_line {
                            (line_index + 1) as i32
                        } else {
                            0
                        },
                        std::mem::take(&mut current),
                    );
                    first_visual_line = false;
                    column = 0;
                    segment_start = offset;
                }
                segment_end = offset + character.len_utf8();
                column += 1;
            }
            if segment_end > segment_start {
                push_syntax_token(
                    &mut current,
                    &token.text[segment_start..segment_end],
                    token.color,
                );
            }
        }
        append_visual_line(
            &mut lines,
            if first_visual_line {
                (line_index + 1) as i32
            } else {
                0
            },
            current,
        );
    }
    ModelRc::new(VecModel::from(lines))
}

pub(crate) fn build_json_tree(
    source: &str,
    expanded_paths: &[slint::SharedString],
    dark: bool,
) -> ModelRc<JsonTreeNode> {
    fn truncate(value: String, max_chars: usize) -> String {
        let mut chars = value.chars();
        let prefix = chars.by_ref().take(max_chars).collect::<String>();
        if chars.next().is_some() {
            format!("{prefix}…")
        } else {
            prefix
        }
    }

    fn escaped_pointer_segment(value: &str) -> String {
        value.replace('~', "~0").replace('/', "~1")
    }

    struct TreeBuildContext<'a> {
        expanded_paths: &'a HashSet<String>,
        colors: SyntaxColors,
        nodes: Vec<JsonTreeNode>,
        truncated: bool,
    }

    fn append_node(
        value: &serde_json::Value,
        path: String,
        depth: usize,
        key_label: String,
        context: &mut TreeBuildContext<'_>,
    ) {
        if context.nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
            context.truncated = true;
            return;
        }
        let expanded = context.expanded_paths.contains(&path);
        let child_count = match value {
            serde_json::Value::Object(fields) => fields.len(),
            serde_json::Value::Array(items) => items.len(),
            _ => 0,
        };
        let has_children = child_count > 0;
        let (preview, value_color) = match value {
            serde_json::Value::Object(fields) => (
                format!("object {{{}}}", fields.len()),
                context.colors.punctuation,
            ),
            serde_json::Value::Array(items) => (
                format!("array [{}]", items.len()),
                context.colors.punctuation,
            ),
            serde_json::Value::String(_) => (
                truncate(serde_json::to_string(value).unwrap_or_default(), 120),
                context.colors.string,
            ),
            serde_json::Value::Number(_) => (value.to_string(), context.colors.number),
            serde_json::Value::Bool(_) => (value.to_string(), context.colors.boolean),
            serde_json::Value::Null => ("null".to_owned(), context.colors.comment),
        };
        context.nodes.push(JsonTreeNode {
            depth: depth as i32,
            key_label: key_label.into(),
            preview: preview.into(),
            path: path.clone().into(),
            has_children,
            expanded,
            key_color: if depth == 0 {
                context.colors.base
            } else {
                context.colors.key
            },
            value_color,
        });

        if !expanded {
            return;
        }
        match value {
            serde_json::Value::Object(fields) => {
                for (key, child) in fields {
                    let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into());
                    append_node(
                        child,
                        format!("{path}/{}", escaped_pointer_segment(key)),
                        depth + 1,
                        format!("{key_json}: "),
                        context,
                    );
                    if context.nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
                        context.truncated = true;
                        break;
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    append_node(
                        child,
                        format!("{path}/{index}"),
                        depth + 1,
                        format!("[{index}] "),
                        context,
                    );
                    if context.nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
                        context.truncated = true;
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(source) else {
        return ModelRc::new(VecModel::from(Vec::new()));
    };
    let expanded_paths = expanded_paths
        .iter()
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    let colors = SyntaxColors {
        base: if dark {
            Color::from_rgb_u8(0xe6, 0xe6, 0xe6)
        } else {
            Color::from_rgb_u8(0x24, 0x1f, 0x31)
        },
        key: if dark {
            Color::from_rgb_u8(0x78, 0xae, 0xed)
        } else {
            Color::from_rgb_u8(0x1c, 0x71, 0xd8)
        },
        string: if dark {
            Color::from_rgb_u8(0x8f, 0xf0, 0xa4)
        } else {
            Color::from_rgb_u8(0x26, 0xa2, 0x69)
        },
        number: if dark {
            Color::from_rgb_u8(0xff, 0xbe, 0x6f)
        } else {
            Color::from_rgb_u8(0xc6, 0x46, 0x00)
        },
        boolean: if dark {
            Color::from_rgb_u8(0xdc, 0x8a, 0xdd)
        } else {
            Color::from_rgb_u8(0x81, 0x3d, 0x9c)
        },
        punctuation: if dark {
            Color::from_rgb_u8(0xc0, 0xbf, 0xbc)
        } else {
            Color::from_rgb_u8(0x5e, 0x5c, 0x64)
        },
        comment: if dark {
            Color::from_rgb_u8(0xa8, 0xa8, 0xa8)
        } else {
            Color::from_rgb_u8(0x6c, 0x6a, 0x73)
        },
    };
    let mut context = TreeBuildContext {
        expanded_paths: &expanded_paths,
        colors,
        nodes: Vec::new(),
        truncated: false,
    };
    append_node(&value, "$".into(), 0, String::new(), &mut context);
    if context.truncated
        && let Some(last) = context.nodes.last_mut()
    {
        last.key_label = "".into();
        last.preview =
            "Display limited to 2,000 rows; use Pretty or Raw for the full response".into();
        last.path = "$limit".into();
        last.has_children = false;
        last.expanded = false;
        last.key_color = context.colors.comment;
        last.value_color = context.colors.comment;
    }
    ModelRc::new(VecModel::from(context.nodes))
}

pub(crate) fn format_response_body(body: &str, format: &str) -> String {
    if body.len() > MAX_STRUCTURED_VIEW_BYTES {
        return String::new();
    }
    match format {
        "JSON" => serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .filter(|pretty| pretty != body)
            .unwrap_or_default(),
        "HTML" | "XML" => {
            let formatted = crate::http::client::format_html(body);
            if formatted == body {
                String::new()
            } else {
                formatted
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod response_tokenizer_tests {
    use super::*;
    use crate::{RequestItem, filter_request_model};
    use slint::Model;

    fn rendered_text(lines: &ModelRc<JsonLine>) -> String {
        let mut output = String::new();
        for line_index in 0..lines.row_count() {
            if let Some(line) = lines.row_data(line_index) {
                for token_index in 0..line.tokens.row_count() {
                    if let Some(token) = line.tokens.row_data(token_index) {
                        output.push_str(token.text.as_str());
                    }
                }
            }
        }
        output
    }

    #[test]
    fn wrapped_json_keeps_its_text_colors_and_line_numbers() {
        let source = r#"{"message":"a long value that must wrap","ok":true}"#;
        let lines = tokenize_response(source, "JSON", true, 12.0);
        assert!(lines.row_count() > 1);
        assert_eq!(rendered_text(&lines), source);
        assert_eq!(lines.row_data(0).unwrap().number, 1);
        assert_eq!(lines.row_data(1).unwrap().number, 0);

        let first = lines.row_data(0).unwrap();
        let mut colors = Vec::new();
        for index in 0..first.tokens.row_count() {
            if let Some(token) = first.tokens.row_data(index)
                && !colors.contains(&token.color)
            {
                colors.push(token.color);
            }
        }
        assert!(
            colors.len() > 1,
            "JSON keys and punctuation retain syntax colors"
        );
    }

    #[test]
    fn long_unbroken_response_wraps_without_losing_text() {
        let source = "x".repeat(64 * 1024);
        let lines = tokenize_response(&source, "Text", true, 120.0);

        assert!(lines.row_count() > 500);
        assert_eq!(rendered_text(&lines), source);
    }

    #[test]
    fn request_search_filters_the_display_model_without_mutating_the_source() {
        let all = ModelRc::new(VecModel::from(vec![
            RequestItem {
                id: 1,
                name: "Health check".into(),
                method: "GET".into(),
                url: "https://api.example.test/health".into(),
            },
            RequestItem {
                id: 2,
                name: "Create item".into(),
                method: "POST".into(),
                url: "https://api.example.test/items".into(),
            },
        ]));

        let filtered = filter_request_model(&all, "ITEMS");
        assert_eq!(filtered.row_count(), 1);
        assert_eq!(filtered.row_data(0).unwrap().id, 2);
        assert_eq!(all.row_count(), 2);
    }

    #[test]
    fn html_tokens_tags_attributes_and_text_with_optional_wrapping() {
        let source = r#"<p class="greeting">Hello world</p>"#;
        let lines = tokenize_response(source, "HTML", true, 10.0);
        assert_eq!(rendered_text(&lines), source);
        assert!(lines.row_count() > 1);
        let mut colors = Vec::new();
        for line_index in 0..lines.row_count() {
            if let Some(line) = lines.row_data(line_index) {
                for token_index in 0..line.tokens.row_count() {
                    if let Some(token) = line.tokens.row_data(token_index)
                        && !colors.contains(&token.color)
                    {
                        colors.push(token.color);
                    }
                }
            }
        }
        assert!(
            colors.len() >= 3,
            "HTML syntax elements use distinct colors"
        );
    }
}
