mod credentials;
mod http;
mod storage;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    hash::{Hash, Hasher},
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::http::HttpClient;
use crate::storage::{
    Collection, CollectionVariable, NewRequest, RequestHistory, RequestStore, SavedRequest,
};
use slint::{Color, ComponentHandle, Model, ModelRc, VecModel};

slint::include_modules!();

const MAX_STRUCTURED_VIEW_BYTES: usize = 256 * 1024;
const MAX_VISIBLE_JSON_TREE_NODES: usize = 2_000;

trait JoinHandleThen<T>: Sized {
    fn then<F, Fut>(self, continuation: F)
    where
        T: Send + 'static,
        F: FnOnce(Result<T, tokio::task::JoinError>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static;
}

impl<T: Send + 'static> JoinHandleThen<T> for tokio::task::JoinHandle<T> {
    fn then<F, Fut>(self, continuation: F)
    where
        F: FnOnce(Result<T, tokio::task::JoinError>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            continuation(self.await).await;
        });
    }
}

fn request_tab_index(app: &AppWindow, key: i32) -> Option<i32> {
    let tabs = app.get_request_tabs();
    (0..tabs.row_count()).find_map(|index| {
        tabs.row_data(index)
            .filter(|tab| tab.key == key)
            .map(|_| index as i32)
    })
}

fn copy_text_to_clipboard(
    clipboard: &Arc<Mutex<Option<arboard::Clipboard>>>,
    text: &str,
) -> Result<(), String> {
    let mut clipboard = clipboard
        .lock()
        .map_err(|_| "clipboard lock was poisoned".to_owned())?;

    let create_error = if clipboard.is_none() {
        match arboard::Clipboard::new() {
            Ok(instance) => {
                *clipboard = Some(instance);
                None
            }
            Err(error) => Some(error.to_string()),
        }
    } else {
        None
    };

    let native_result = match clipboard.as_mut() {
        Some(instance) => instance.set_text(text).map_err(|error| error.to_string()),
        None => Err(create_error.unwrap_or_else(|| "clipboard is unavailable".to_owned())),
    };

    match native_result {
        Ok(()) => Ok(()),
        Err(native_error) => {
            #[cfg(target_os = "linux")]
            {
                return copy_text_with_linux_clipboard_tools(text)
                    .map_err(|fallback_error| format!("{native_error}; {fallback_error}"));
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(native_error)
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn copy_text_with_linux_clipboard_tools(text: &str) -> Result<(), String> {
    use std::io::Write;

    for (program, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--input"][..]),
    ] {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };

        let write_result = child
            .stdin
            .take()
            .ok_or_else(|| "clipboard process has no input pipe".to_owned())
            .and_then(|mut stdin| {
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|error| error.to_string())
            });
        let status = child.wait().map_err(|error| error.to_string())?;
        if write_result.is_ok() && status.success() {
            return Ok(());
        }
    }

    Err("no working Linux clipboard backend was found".to_owned())
}

fn detect_system_dark_mode(fallback: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            if output.status.success() {
                let preference = String::from_utf8_lossy(&output.stdout);
                if preference.contains("prefer-dark") {
                    return true;
                }
                if preference.contains("prefer-light") {
                    return false;
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
        {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout);
                if let Some(hex) = value.split_whitespace().last()
                    && let Ok(light) = u32::from_str_radix(hex.trim_start_matches("0x"), 16)
                {
                    return light == 0;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
        {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .eq_ignore_ascii_case("dark");
            }
            // macOS omits this preference when using the default light appearance.
            return false;
        }
    }

    fallback
}

fn detect_system_accent_color(fallback: Color) -> Color {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "accent-color"])
            .output()
        {
            if output.status.success() {
                let accent = String::from_utf8_lossy(&output.stdout);
                let rgb = match accent.trim().trim_matches('\'') {
                    "blue" => Some((0x35, 0x84, 0xe4)),
                    "teal" => Some((0x21, 0x90, 0xa4)),
                    "green" => Some((0x3a, 0x94, 0x4a)),
                    "yellow" => Some((0xc8, 0x88, 0x00)),
                    "orange" => Some((0xed, 0x5b, 0x00)),
                    "red" => Some((0xe6, 0x2d, 0x42)),
                    "pink" => Some((0xd5, 0x61, 0x99)),
                    "purple" => Some((0x91, 0x41, 0xac)),
                    "slate" => Some((0x6f, 0x83, 0x96)),
                    _ => None,
                };
                if let Some((red, green, blue)) = rgb {
                    return Color::from_rgb_u8(red, green, blue);
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                "HKCU\\Software\\Microsoft\\Windows\\DWM",
                "/v",
                "AccentColor",
            ])
            .output()
        {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout);
                if let Some(hex) = value.split_whitespace().last()
                    && let Ok(abgr) = u32::from_str_radix(hex.trim_start_matches("0x"), 16)
                {
                    return Color::from_rgb_u8(
                        (abgr & 0xff) as u8,
                        ((abgr >> 8) & 0xff) as u8,
                        ((abgr >> 16) & 0xff) as u8,
                    );
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("defaults")
            .args(["read", "-g", "AppleAccentColor"])
            .output()
        {
            if output.status.success()
                && let Ok(index) = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<i32>()
            {
                let rgb = match index {
                    0 => Some((0xd7, 0x00, 0x15)),
                    1 => Some((0xf5, 0x63, 0x00)),
                    2 => Some((0xf2, 0xaa, 0x00)),
                    3 => Some((0x00, 0x8f, 0x5a)),
                    4 => Some((0x00, 0x7a, 0xcc)),
                    5 => Some((0x8e, 0x24, 0xaa)),
                    6 => Some((0xd4, 0x50, 0x87)),
                    _ => None,
                };
                if let Some((red, green, blue)) = rgb {
                    return Color::from_rgb_u8(red, green, blue);
                }
            }
        }
    }

    fallback
}

fn to_request_model(requests: Vec<SavedRequest>, query: &str) -> ModelRc<RequestItem> {
    let query = query.trim().to_ascii_lowercase();
    let items = requests
        .into_iter()
        .filter(|request| {
            query.is_empty()
                || request.name.to_ascii_lowercase().contains(&query)
                || request.method.to_ascii_lowercase().contains(&query)
                || request.url.to_ascii_lowercase().contains(&query)
        })
        .map(|request| RequestItem {
            id: request.id as i32,
            name: request.name.into(),
            method: request.method.into(),
            url: request.url.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_collection_model(collections: Vec<Collection>) -> ModelRc<CollectionItem> {
    let items = collections
        .into_iter()
        .map(|collection| CollectionItem {
            id: collection.id as i32,
            name: collection.name.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_variable_model(variables: Vec<CollectionVariable>) -> ModelRc<VariableItem> {
    let items = variables
        .into_iter()
        .map(|variable| VariableItem {
            key: variable.key.into(),
            value: variable.value.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_key_value_model(value: &str, separator: char) -> ModelRc<KeyValueRow> {
    let rows = value
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once(separator).unwrap_or((line, ""));
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(KeyValueRow {
                key: key.into(),
                value: value.trim().into(),
                enabled: true,
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn serialize_key_value_rows(rows: ModelRc<KeyValueRow>, separator: &str) -> String {
    rows.iter()
        .filter(|row| row.enabled && !row.key.trim().is_empty())
        .map(|row| format!("{}{}{}", row.key.trim(), separator, row.value.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn to_response_header_model(headers: &str) -> ModelRc<ResponseHeaderItem> {
    let items = headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some(ResponseHeaderItem {
                name: name.trim().into(),
                value: value.trim().into(),
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_response_cookie_model(cookies: &str) -> ModelRc<ResponseCookieItem> {
    let items = cookies
        .split("\n\n")
        .filter_map(|cookie| {
            let mut lines = cookie.lines();
            let heading = lines.next()?.trim();
            let (_, name) = heading.strip_prefix("Cookie ")?.split_once(" · ")?;
            let value = lines
                .next()
                .unwrap_or_default()
                .trim()
                .strip_prefix("Value:")
                .unwrap_or_default()
                .trim();
            let attributes = lines
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
            Some(ResponseCookieItem {
                name: name.trim().into(),
                value: value.into(),
                attributes: attributes.into(),
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_environment_model(items: Vec<(i32, String, CollectionVariable)>) -> ModelRc<EnvironmentItem> {
    let items = items
        .into_iter()
        .map(
            |(collection_id, collection_name, variable)| EnvironmentItem {
                collection_id,
                collection_name: collection_name.into(),
                key: variable.key.into(),
                value: variable.value.into(),
            },
        )
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_history_model(history: Vec<RequestHistory>) -> ModelRc<HistoryItem> {
    let items = history
        .into_iter()
        .map(|item| HistoryItem {
            request_id: item.request_id as i32,
            request_name: item.request_name.into(),
            collection_name: item.collection_name.into(),
            method: item.method.into(),
            url: item.url.into(),
            status: item.status.into(),
            duration: format!("{} ms", item.elapsed_ms).into(),
            date: item.created_at.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

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

fn tokenize_response(
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
            let mut segment_start = 0;
            let mut segment_end = 0;
            for (offset, character) in token.text.char_indices() {
                if column == max_columns {
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

fn build_json_tree(
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

    fn append_node(
        value: &serde_json::Value,
        path: String,
        depth: usize,
        key_label: String,
        expanded_paths: &HashSet<String>,
        colors: SyntaxColors,
        nodes: &mut Vec<JsonTreeNode>,
        truncated: &mut bool,
    ) {
        if nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
            *truncated = true;
            return;
        }
        let expanded = expanded_paths.contains(&path);
        let child_count = match value {
            serde_json::Value::Object(fields) => fields.len(),
            serde_json::Value::Array(items) => items.len(),
            _ => 0,
        };
        let has_children = child_count > 0;
        let (preview, value_color) = match value {
            serde_json::Value::Object(fields) => {
                (format!("object {{{}}}", fields.len()), colors.punctuation)
            }
            serde_json::Value::Array(items) => {
                (format!("array [{}]", items.len()), colors.punctuation)
            }
            serde_json::Value::String(_) => (
                truncate(serde_json::to_string(value).unwrap_or_default(), 120),
                colors.string,
            ),
            serde_json::Value::Number(_) => (value.to_string(), colors.number),
            serde_json::Value::Bool(_) => (value.to_string(), colors.boolean),
            serde_json::Value::Null => ("null".to_owned(), colors.comment),
        };
        nodes.push(JsonTreeNode {
            depth: depth as i32,
            key_label: key_label.into(),
            preview: preview.into(),
            path: path.clone().into(),
            has_children,
            expanded,
            key_color: if depth == 0 { colors.base } else { colors.key },
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
                        expanded_paths,
                        colors,
                        nodes,
                        truncated,
                    );
                    if nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
                        *truncated = true;
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
                        expanded_paths,
                        colors,
                        nodes,
                        truncated,
                    );
                    if nodes.len() >= MAX_VISIBLE_JSON_TREE_NODES {
                        *truncated = true;
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
    let mut nodes = Vec::new();
    let mut truncated = false;
    append_node(
        &value,
        "$".into(),
        0,
        String::new(),
        &expanded_paths,
        colors,
        &mut nodes,
        &mut truncated,
    );
    if truncated && let Some(last) = nodes.last_mut() {
        last.key_label = "".into();
        last.preview =
            "Display limited to 2,000 rows; use Pretty or Raw for the full response".into();
        last.path = "$limit".into();
        last.has_children = false;
        last.expanded = false;
        last.key_color = colors.comment;
        last.value_color = colors.comment;
    }
    ModelRc::new(VecModel::from(nodes))
}

fn format_response_body(body: &str, format: &str) -> String {
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

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let store = Arc::new(RequestStore::open_default().map_err(|error| {
        slint::PlatformError::from(format!(
            "Could not initialize Resonance local storage: {error}"
        ))
    })?);
    let client = Arc::new(HttpClient::new().map_err(|error| {
        slint::PlatformError::from(format!("Could not initialize the HTTP client: {error}"))
    })?);
    let app = AppWindow::new()?;
    app.set_shortcut_modifier(if cfg!(target_os = "macos") {
        "⌘".into()
    } else {
        "Ctrl".into()
    });
    app.set_shortcut_alt_modifier(if cfg!(target_os = "macos") {
        "⌥".into()
    } else {
        "Alt".into()
    });
    slint::set_xdg_app_id("io.github.yashG0.resonance-api-testing-platform")?;
    app.set_system_dark_mode(detect_system_dark_mode(app.get_system_dark_mode()));
    app.set_system_accent_color(detect_system_accent_color(app.get_system_accent_color()));
    let app_weak = app.as_weak();
    let tree_expansion_state =
        Arc::new(Mutex::new(HashMap::<String, (u64, HashSet<String>)>::new()));

    app.on_refresh_system_theme({
        let app_weak = app_weak.clone();
        move || {
            if let Some(ui) = app_weak.upgrade() {
                let fallback = ui.get_system_dark_mode();
                ui.set_system_dark_mode(detect_system_dark_mode(fallback));
                let accent_fallback = ui.get_system_accent_color();
                ui.set_system_accent_color(detect_system_accent_color(accent_fallback));
            }
        }
    });

    app.on_serialize_param_rows(|rows| serialize_key_value_rows(rows, "=").into());
    app.on_serialize_header_rows(|rows| serialize_key_value_rows(rows, ": ").into());

    app.on_format_request_body(|body, body_format| {
        if body.as_str().len() > MAX_STRUCTURED_VIEW_BYTES {
            return body;
        }
        let body_str: String = body.to_string();
        let format_str: String = body_format.to_string();
        let highlighted = match format_str.as_str() {
            "JSON" => match serde_json::from_str::<serde_json::Value>(&body_str) {
                Ok(json) => {
                    let pretty =
                        serde_json::to_string_pretty(&json).unwrap_or_else(|_| body_str.clone());
                    crate::http::client::highlight_json(&pretty)
                }
                Err(_) => crate::http::client::escape_html(&body_str),
            },
            "XML" | "HTML" => {
                crate::http::client::highlight_xml(&crate::http::client::format_html(&body_str))
            }
            _ => crate::http::client::escape_html(&body_str),
        };
        highlighted.into()
    });

    app.on_tokenize_response(|source, format, dark, max_columns| {
        tokenize_response(source.as_str(), format.as_str(), dark, max_columns)
    });

    app.on_build_json_tree({
        let tree_expansion_state = Arc::clone(&tree_expansion_state);
        move |scope, source, dark| {
            let source_text = source.to_string();
            let scope = scope.to_string();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            source_text.hash(&mut hasher);
            let source_hash = hasher.finish();
            let expanded_paths = if let Ok(mut state) = tree_expansion_state.lock() {
                let entry = state
                    .entry(scope)
                    .or_insert_with(|| (source_hash, HashSet::new()));
                if entry.0 != source_hash {
                    entry.0 = source_hash;
                    entry.1.clear();
                }
                entry
                    .1
                    .iter()
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            build_json_tree(source_text.as_str(), &expanded_paths, dark)
        }
    });

    app.on_toggle_json_tree_node({
        let tree_expansion_state = Arc::clone(&tree_expansion_state);
        move |scope, path| {
            if let Ok(mut state) = tree_expansion_state.lock() {
                let expanded = state
                    .entry(scope.to_string())
                    .or_insert_with(|| (0, HashSet::new()));
                let path = path.to_string();
                if !expanded.1.remove(&path) {
                    expanded.1.insert(path);
                }
            }
        }
    });

    // Keep one clipboard owner alive for the application lifetime. This is
    // required by X11/Wayland selection ownership and also avoids concurrent
    // platform clipboard access from the response controls.
    let clipboard = Arc::new(Mutex::new(None::<arboard::Clipboard>));
    app.on_copy_text({
        let clipboard = Arc::clone(&clipboard);
        move |text| {
            if let Err(error) = copy_text_to_clipboard(&clipboard, text.as_str()) {
                eprintln!("Resonance: could not copy response: {error}");
            }
        }
    });

    app.on_suggest_request_name({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let mut names = HashSet::new();
            if let Ok(requests) = store.list_for_collection(collection_id as i64) {
                names.extend(requests.into_iter().map(|request| request.name));
            }
            if let Some(ui) = app_weak.upgrade() {
                let tabs = ui.get_request_tabs();
                for index in 0..tabs.row_count() {
                    if let Some(tab) = tabs.row_data(index) {
                        if tab.collection_id == collection_id {
                            names.insert(tab.name.to_string());
                        }
                    }
                }
            }
            let mut suffix = 1_u64;
            loop {
                let candidate = format!("New Request {suffix}");
                if !names.contains(&candidate) {
                    return candidate.into();
                }
                suffix += 1;
            }
        }
    });

    // Load the selected collection's variables off the UI thread, then send the
    // fully resolved request without blocking Slint's event loop.
    app.on_send_request({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);

        move |method,
              url,
              body,
              headers,
              params,
              body_format,
              auth_type,
              auth_value,
              auth_secret,
              collection_id,
              request_id,
              name,
              tab_key| {
            let app_handle = app_weak.clone();
            let client = Arc::clone(&client);
            let store = Arc::clone(&store);
            let method = method.to_string();
            let url = url.to_string();
            let body = body.to_string();
            let headers = headers.to_string();
            let params = params.to_string();
            let body_format = body_format.to_string();
            let auth_type = auth_type.to_string();
            let auth_value = auth_value.to_string();
            let auth_secret = auth_secret.to_string();
            let name = name.to_string();
            let collection_id = collection_id as i64;
            let tab_key = tab_key;

            tokio::spawn(async move {
                // Save first so each request is available in its collection as soon
                // as the response arrives. Existing requests are updated in place.
                let mut request = NewRequest {
                    collection_id,
                    name: name.clone(),
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.clone(),
                    query_params: params.clone(),
                    body: body.clone(),
                    body_format: body_format.clone(),
                    auth_type: auth_type.clone(),
                    auth_value: auth_value.clone(),
                    auth_secret: auth_secret.clone(),
                };
                let save_store = Arc::clone(&store);
                let save_result = tokio::task::spawn_blocking(move || {
                    // New drafts start with the same friendly title. Give each
                    // one a collection-local name before inserting it.
                    if request_id <= 0 {
                        let existing = save_store.list_for_collection(collection_id)?;
                        if request.name.trim().starts_with("New Request ") {
                            let mut suffix =
                                request.name.trim()[12..].parse::<u64>().unwrap_or(1).max(1);
                            while existing.iter().any(|item| item.name == request.name) {
                                suffix += 1;
                                request.name = format!("New Request {suffix}");
                            }
                        }
                    }
                    let saved_name = request.name.clone();
                    let saved_id =
                        if request_id > 0 && save_store.update(request_id as i64, &request)? {
                            request_id as i64
                        } else {
                            save_store.create(&request)?.id
                        };
                    let requests = save_store.list_for_collection(collection_id)?;
                    let variables = save_store.variables_map(collection_id)?;
                    Ok::<_, crate::storage::StoreError>((saved_id, saved_name, requests, variables))
                })
                .await;

                let (saved_id, saved_name, requests, variables) = match save_result {
                    Ok(Ok(saved)) => saved,
                    Ok(Err(error)) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                                    return;
                                };
                                ui.invoke_set_request_tab_response(
                                    tab_index,
                                    tab_key,
                                    "Auto-save failed".into(),
                                    message.into(),
                                    "".into(),
                                    "---".into(),
                                    "error".into(),
                                    "Text".into(),
                                    "".into(),
                                    "".into(),
                                    false,
                                    ModelRc::default(),
                                    ModelRc::default(),
                                );
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                                    return;
                                };
                                ui.invoke_set_request_tab_response(
                                    tab_index,
                                    tab_key,
                                    "Auto-save failed".into(),
                                    message.into(),
                                    "".into(),
                                    "---".into(),
                                    "error".into(),
                                    "Text".into(),
                                    "".into(),
                                    "".into(),
                                    false,
                                    ModelRc::default(),
                                    ModelRc::default(),
                                );
                            }
                        });
                        return;
                    }
                };

                let list_handle = app_handle.clone();
                let persisted_name = saved_name.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = list_handle.upgrade() {
                        if ui.get_expanded_collection_id() as i64 == collection_id {
                            ui.set_saved_requests(to_request_model(
                                requests,
                                ui.get_request_search().as_str(),
                            ));
                        }
                        if let Some(tab_index) = request_tab_index(&ui, tab_key) {
                            ui.invoke_set_request_tab_identity(
                                tab_index,
                                tab_key,
                                saved_id as i32,
                                persisted_name.into(),
                            );
                        }
                    }
                });

                let result = client
                    .send(
                        &method,
                        &url,
                        &body,
                        &headers,
                        &params,
                        &body_format,
                        &auth_type,
                        &auth_value,
                        &auth_secret,
                        &variables,
                    )
                    .await;
                let history_store = Arc::clone(&store);
                let history_status = result.status.clone();
                let history_method = method.clone();
                let history_url = url.clone();
                let history_name = saved_name.clone();
                let elapsed_ms = (result.elapsed.as_secs_f64() * 1000.0) as i64;
                let history_handle = app_handle.clone();
                tokio::task::spawn_blocking(move || {
                    history_store.record_history(
                        saved_id,
                        collection_id,
                        &history_name,
                        &history_method,
                        &history_url,
                        &history_status,
                        elapsed_ms,
                    )?;
                    history_store.list_history()
                })
                .then(move |history_result| async move {
                    if let Ok(Ok(history)) = history_result {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = history_handle.upgrade() {
                                if ui.get_active_view() == "History" {
                                    ui.set_history_items(to_history_model(history));
                                }
                            }
                        });
                    }
                });
                let response_state = result.state.clone();
                let elapsed = format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0);
                let response_status = result.status.clone();
                let response_body = result.body;
                let response_format = result.format.clone();
                let response_headers = result.headers.clone();
                let response_cookies = result.cookies.clone();
                let response_large = response_body.len() > MAX_STRUCTURED_VIEW_BYTES;
                let response_body_formatted = if response_large
                    || !matches!(response_format.as_str(), "JSON" | "HTML" | "XML")
                {
                    String::new()
                } else {
                    let formatted_body_source = response_body.clone();
                    let formatted_body_format = response_format.clone();
                    tokio::task::spawn_blocking(move || {
                        format_response_body(&formatted_body_source, &formatted_body_format)
                    })
                    .await
                    .unwrap_or_default()
                };

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                            return;
                        };
                        let header_items = to_response_header_model(&response_headers);
                        let cookie_items = to_response_cookie_model(&response_cookies);
                        ui.invoke_set_request_tab_response(
                            tab_index,
                            tab_key,
                            response_status.into(),
                            response_body.into(),
                            response_body_formatted.into(),
                            elapsed.into(),
                            response_state.into(),
                            response_format.into(),
                            response_headers.into(),
                            response_cookies.into(),
                            response_large,
                            header_items,
                            cookie_items,
                        );
                    }
                });
            });
        }
    });

    app.on_run_collection({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);
        move |collection_id| {
            if let Some(ui) = app_weak.upgrade() {
                ui.set_response_tab("Body".into());
                ui.set_response_headers("".into());
                ui.set_response_cookies("".into());
                ui.set_response_header_items(to_response_header_model(""));
                ui.set_response_cookie_items(to_response_cookie_model(""));
            }
            let app_handle = app_weak.clone();
            let client = Arc::clone(&client);
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let snapshot_store = Arc::clone(&store);
                let snapshot = tokio::task::spawn_blocking(move || {
                    Ok::<_, crate::storage::StoreError>((
                        snapshot_store.list_for_collection(collection_id as i64)?,
                        snapshot_store.variables_map(collection_id as i64)?,
                    ))
                })
                .await;
                let (requests, variables) = match snapshot {
                    Ok(Ok(snapshot)) => snapshot,
                    Ok(Err(error)) => {
                        let message = format!("Could not start collection run: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                ui.set_response_status("Collection run failed".into());
                                ui.set_response_state("error".into());
                                ui.set_response_body(message.into());
                                ui.set_is_loading(false);
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        let message = format!("Could not start collection run: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                ui.set_response_status("Collection run failed".into());
                                ui.set_response_state("error".into());
                                ui.set_response_body(message.into());
                                ui.set_is_loading(false);
                            }
                        });
                        return;
                    }
                };

                let start_handle = app_handle.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = start_handle.upgrade() {
                        ui.set_response_large(false);
                        ui.set_is_loading(true);
                        ui.set_response_status("Running collection…".into());
                        ui.set_response_state("loading".into());
                    }
                });

                let mut passed = 0usize;
                let mut lines = Vec::new();
                let mut history_entries: Vec<(i64, i64, String, String, String, String, i64)> =
                    Vec::new();

                for request in requests {
                    let result = client
                        .send(
                            &request.method,
                            &request.url,
                            &request.body,
                            &request.headers,
                            &request.query_params,
                            &request.body_format,
                            &request.auth_type,
                            &request.auth_value,
                            &request.auth_secret,
                            &variables,
                        )
                        .await;
                    if result.status.starts_with('2') || result.status.starts_with('3') {
                        passed += 1;
                    }
                    lines.push(format!(
                        "{} {} — {} ({:.2} ms)",
                        request.method,
                        request.name,
                        result.status,
                        result.elapsed.as_secs_f64() * 1000.0
                    ));
                    history_entries.push((
                        request.id,
                        request.collection_id,
                        request.name.clone(),
                        request.method.clone(),
                        request.url.clone(),
                        result.status.clone(),
                        (result.elapsed.as_secs_f64() * 1000.0) as i64,
                    ));
                }
                let history_store = Arc::clone(&store);
                let _ = tokio::task::spawn_blocking(move || {
                    history_store.record_history_batch(&history_entries)
                })
                .await;

                let total = lines.len();
                let state = if passed == total {
                    "success"
                } else {
                    "http-error"
                };
                let status = format!("Collection run · {passed}/{total} passed");
                let body = if lines.is_empty() {
                    "This collection has no saved requests.".to_owned()
                } else {
                    lines.join("\n")
                };
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() == collection_id {
                            ui.set_response_status(status.into());
                            ui.set_response_state(state.into());
                            ui.set_response_format("Text".into());
                            ui.set_response_body(body.into());
                            ui.set_response_time("".into());
                            ui.set_is_loading(false);
                            if ui.get_active_view() == "History" {
                                ui.invoke_load_history(ui.get_history_sort());
                            }
                        }
                    }
                });
            });
        }
    });

    app.on_save_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |id,
              collection_id,
              name,
              method,
              url,
              headers,
              params,
              body,
              body_format,
              auth_type,
              auth_value,
              auth_secret,
              tab_key| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let mut request = NewRequest {
                collection_id: collection_id as i64,
                name: name.to_string(),
                method: method.to_string(),
                url: url.to_string(),
                headers: headers.to_string(),
                query_params: params.to_string(),
                body: body.to_string(),
                body_format: body_format.to_string(),
                auth_type: auth_type.to_string(),
                auth_value: auth_value.to_string(),
                auth_secret: auth_secret.to_string(),
            };
            let tab_key = tab_key;

            tokio::task::spawn_blocking(move || {
                if id <= 0 && request.name.trim().starts_with("New Request ") {
                    if let Ok(existing) = store.list_for_collection(collection_id as i64) {
                        let mut suffix =
                            request.name.trim()[12..].parse::<u64>().unwrap_or(1).max(1);
                        while existing.iter().any(|item| item.name == request.name) {
                            suffix += 1;
                            request.name = format!("New Request {suffix}");
                        }
                    }
                }
                let saved_name = request.name.clone();
                let result = if id > 0 {
                    store
                        .update(id as i64, &request)
                        .map(|updated| if updated { id as i64 } else { 0 })
                } else {
                    store.create(&request).map(|saved| saved.id)
                };
                let list_result = store.list_for_collection(collection_id as i64);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(saved_id) if saved_id > 0 => {
                                if ui.get_active_request_tab_key() == tab_key {
                                    ui.invoke_store_active_request_tab();
                                }
                                if let Some(tab_index) = request_tab_index(&ui, tab_key) {
                                    ui.invoke_set_request_tab_identity(
                                        tab_index,
                                        tab_key,
                                        saved_id as i32,
                                        saved_name.clone().into(),
                                    );
                                    if ui.get_active_request_tab_key() == tab_key {
                                        ui.set_response_status(
                                            format!("Saved: {saved_name}").into(),
                                        );
                                    }
                                }
                            }
                            Ok(_) => ui.set_response_status(
                                "Save failed: request no longer exists".into(),
                            ),
                            Err(error) => {
                                ui.set_response_status(format!("Save failed: {error}").into())
                            }
                        }
                        if let Ok(requests) = list_result {
                            ui.set_saved_requests(to_request_model(
                                requests,
                                ui.get_request_search().as_str(),
                            ));
                        }
                    }
                });
            });
        }
    });

    app.on_load_collections({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move || {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_collections()).then(
                move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(collections)) => {
                                    let selected = collections
                                        .iter()
                                        .find(|item| {
                                            item.id == ui.get_selected_collection_id() as i64
                                        })
                                        .or_else(|| {
                                            collections.iter().find(|item| {
                                                item.name.eq_ignore_ascii_case("Public API")
                                            })
                                        })
                                        .or_else(|| collections.first())
                                        .cloned();
                                    ui.set_collections(to_collection_model(collections));
                                    if let Some(selected) = selected {
                                        let id = selected.id as i32;
                                        ui.set_selected_collection_id(id);
                                        ui.set_expanded_collection_id(id);
                                        ui.set_selected_collection_name(
                                            selected.name.clone().into(),
                                        );
                                        ui.invoke_load_requests(id);
                                        ui.invoke_load_collection_variables(id);
                                    }
                                }
                                Ok(Err(error)) => ui.set_variables_message(
                                    format!("Could not load collections: {error}").into(),
                                ),
                                Err(error) => ui.set_variables_message(
                                    format!("Could not load collections: {error}").into(),
                                ),
                            }
                        }
                    });
                },
            );
        }
    });

    app.on_load_requests({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_for_collection(collection_id as i64))
                .then(move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(requests)) => {
                                    if ui.get_expanded_collection_id() == collection_id {
                                        ui.set_saved_requests(to_request_model(
                                            requests,
                                            ui.get_request_search().as_str(),
                                        ));
                                    }
                                }
                                Ok(Err(error)) => {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
                                Err(error) => {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
                            }
                        }
                    });
                });
        }
    });

    app.on_search_requests({
        let app_weak = app_weak.clone();
        move |_query| {
            if let Some(ui) = app_weak.upgrade() {
                let collection_id = ui.get_expanded_collection_id();
                if collection_id > 0 {
                    ui.invoke_load_requests(collection_id);
                }
            }
        }
    });

    app.on_load_history({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |sort_order| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let mut history = store.list_history()?;
                if sort_order == "Oldest first" {
                    history.reverse();
                }
                Ok::<_, crate::storage::StoreError>(history)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if let Ok(Ok(history)) = result {
                            ui.set_history_items(to_history_model(history));
                        }
                    }
                });
            });
        }
    });

    app.on_load_environments({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move || {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let mut items = Vec::new();
                for collection in store.list_collections()? {
                    for variable in store.list_variables(collection.id)? {
                        items.push((collection.id as i32, collection.name.clone(), variable));
                    }
                }
                Ok::<_, crate::storage::StoreError>(items)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(items)) => ui.set_environment_items(to_environment_model(items)),
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not load environments: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not load environments: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_load_collection_variables({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_variables(collection_id as i64)).then(
                move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(variables)) => {
                                    ui.set_collection_variables(to_variable_model(variables));
                                    ui.set_variables_message("".into());
                                }
                                Ok(Err(error)) => ui.set_variables_message(
                                    format!("Could not load variables: {error}").into(),
                                ),
                                Err(error) => ui.set_variables_message(
                                    format!("Could not load variables: {error}").into(),
                                ),
                            }
                        }
                    });
                },
            );
        }
    });

    app.on_create_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |name| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let name = name.to_string();
            tokio::task::spawn_blocking(move || {
                let created = store.create_collection(&name)?;
                let collections = store.list_collections()?;
                Ok::<_, crate::storage::StoreError>((created, collections))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok((created, collections))) => {
                                let id = created.id as i32;
                                ui.invoke_store_active_request_tab();
                                ui.set_collections(to_collection_model(collections));
                                ui.set_selected_collection_id(id);
                                ui.set_expanded_collection_id(id);
                                ui.set_selected_collection_name(created.name.into());
                                ui.set_active_view("Request".into());
                                ui.set_active_request_id(-1);
                                ui.set_active_request("".into());
                                ui.set_request_name_draft("".into());
                                ui.set_editing_request_name(false);
                                ui.set_is_loading(false);
                                ui.set_active_method("GET".into());
                                ui.set_request_url("".into());
                                ui.set_request_headers("Accept: application/json".into());
                                ui.set_request_header_rows(to_key_value_model(
                                    "Accept: application/json",
                                    ':',
                                ));
                                ui.set_request_params("".into());
                                ui.set_request_param_rows(to_key_value_model("", '='));
                                ui.set_request_body("".into());
                                ui.set_body_format("JSON".into());
                                ui.set_auth_type("None".into());
                                ui.set_auth_value("".into());
                                ui.set_auth_secret("".into());
                                ui.set_active_tab("Body".into());
                                ui.set_response_status("No response yet".into());
                                ui.set_response_body(
                                    "Send a request to inspect its response.".into(),
                                );
                                ui.set_response_time("---".into());
                                ui.set_response_state("idle".into());
                                ui.set_response_format("Text".into());
                                ui.invoke_register_active_request_tab();
                                ui.invoke_load_requests(id);
                                ui.invoke_load_collection_variables(id);
                                ui.set_variables_message("Collection created.".into());
                                ui.set_new_collection_name("".into());
                                ui.set_show_new_collection(false);
                                ui.invoke_focus_request_url();
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not create collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not create collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_rename_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, name| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let name = name.to_string();
            tokio::task::spawn_blocking(move || {
                let renamed = store.rename_collection(collection_id as i64, &name)?;
                let collections = store.list_collections()?;
                Ok::<_, crate::storage::StoreError>((renamed, collections))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok((renamed, collections))) => {
                                ui.set_collections(to_collection_model(collections));
                                if ui.get_selected_collection_id() == renamed.id as i32 {
                                    ui.set_selected_collection_name(renamed.name.into());
                                }
                                ui.set_editing_collection_id(-1);
                                ui.set_variables_message("Collection renamed.".into());
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not rename collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not rename collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_delete_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let selected_before = app_weak.upgrade().map(|ui| ui.get_selected_collection_id());
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store.delete_collection(collection_id as i64)?;
                store.list_collections()
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(collections)) => {
                                let selected_id = selected_before
                                    .filter(|selected_id| {
                                        *selected_id != collection_id
                                            && collections.iter().any(|collection| {
                                                collection.id as i32 == *selected_id
                                            })
                                    })
                                    .or_else(|| {
                                        collections.first().map(|collection| collection.id as i32)
                                    });
                                let Some(selected_id) = selected_id else {
                                    ui.set_variables_message(
                                        "Could not delete collection: no collections remain."
                                            .into(),
                                    );
                                    return;
                                };
                                let selection_changed = selected_before != Some(selected_id);
                                let selected_name = collections
                                    .iter()
                                    .find(|collection| collection.id as i32 == selected_id)
                                    .map(|collection| collection.name.clone());
                                ui.set_collections(to_collection_model(collections));
                                ui.set_confirm_delete_collection_id(-1);
                                if selection_changed {
                                    if let Some(selected_name) = selected_name {
                                        ui.set_selected_collection_id(selected_id);
                                        ui.set_expanded_collection_id(selected_id);
                                        ui.set_selected_collection_name(selected_name.into());
                                    }
                                    ui.set_active_request_id(-1);
                                    ui.set_active_request("".into());
                                    ui.set_editing_request_name(false);
                                    ui.set_active_method("GET".into());
                                    ui.set_request_url("".into());
                                    ui.set_request_params("".into());
                                    ui.set_request_param_rows(to_key_value_model("", '='));
                                    ui.set_request_body("".into());
                                    ui.set_body_format("JSON".into());
                                    ui.set_auth_type("None".into());
                                    ui.set_auth_value("".into());
                                    ui.set_auth_secret("".into());
                                    ui.set_request_headers("Accept: application/json".into());
                                    ui.set_request_header_rows(to_key_value_model(
                                        "Accept: application/json",
                                        ':',
                                    ));
                                    ui.set_response_status("No response yet".into());
                                    ui.set_response_body(
                                        "Send a request to inspect its response.".into(),
                                    );
                                    ui.set_response_time("---".into());
                                    ui.set_response_state("idle".into());
                                    ui.set_response_format("Text".into());
                                    ui.set_response_headers("".into());
                                    ui.set_response_cookies("".into());
                                    ui.set_response_header_items(to_response_header_model(""));
                                    ui.set_response_cookie_items(to_response_cookie_model(""));
                                    ui.set_response_tab("Body".into());
                                    ui.invoke_load_requests(selected_id);
                                    ui.invoke_load_collection_variables(selected_id);
                                }
                                ui.set_variables_message(
                                    "Collection and its saved data deleted.".into(),
                                );
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not delete collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not delete collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_save_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, key, value| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let key = key.to_string();
            let value = value.to_string();
            let submitted_key = key.clone();
            let submitted_value = value.clone();
            tokio::task::spawn_blocking(move || {
                store.upsert_variable(collection_id as i64, &key, &value)?;
                let variables = store.list_variables(collection_id as i64)?;
                Ok::<_, crate::storage::StoreError>(variables)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                if ui.get_variable_key() == submitted_key
                                    && ui.get_variable_value() == submitted_value
                                {
                                    ui.set_variable_key("".into());
                                    ui.set_variable_value("".into());
                                }
                                ui.set_variables_message("Variable saved.".into());
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not save variable: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not save variable: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_delete_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, key| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let key = key.to_string();
            tokio::task::spawn_blocking(move || {
                store.delete_variable(collection_id as i64, &key)?;
                store.list_variables(collection_id as i64)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                ui.set_variables_message("Variable deleted.".into());
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not delete variable: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not delete variable: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_select_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |id| {
            if id <= 0 {
                return;
            }
            if let Some(ui) = app_weak.upgrade() {
                let tabs = ui.get_request_tabs();
                for index in 0..tabs.row_count() {
                    if tabs.row_data(index).is_some_and(|tab| tab.request_id == id) {
                        ui.invoke_activate_request_tab(index as i32);
                        return;
                    }
                }
                ui.invoke_store_active_request_tab();
            }
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store.get(id as i64).and_then(|request| {
                    let Some(request) = request else {
                        return Ok(None);
                    };
                    let collections = store.list_collections()?;
                    let collection_name = collections
                        .into_iter()
                        .find(|collection| collection.id == request.collection_id)
                        .map(|collection| collection.name)
                        .unwrap_or_default();
                    let requests = store.list_for_collection(request.collection_id)?;
                    Ok(Some((request, collection_name, requests)))
                })
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(Some((request, collection_name, requests)))) => {
                                let collection_id = request.collection_id as i32;
                                ui.set_selected_collection_id(collection_id);
                                ui.set_expanded_collection_id(collection_id);
                                ui.set_selected_collection_name(collection_name.into());
                                let search = ui.get_request_search().to_string();
                                ui.set_saved_requests(to_request_model(requests, &search));
                                ui.invoke_load_collection_variables(collection_id);
                                ui.set_active_view("Request".into());
                                ui.set_is_loading(false);
                                ui.set_active_request_id(request.id as i32);
                                ui.set_active_request(request.name.into());
                                ui.set_editing_request_name(false);
                                ui.set_active_method(request.method.into());
                                ui.set_request_url(request.url.into());
                                ui.set_request_header_rows(to_key_value_model(
                                    &request.headers,
                                    ':',
                                ));
                                ui.set_request_param_rows(to_key_value_model(
                                    &request.query_params,
                                    '=',
                                ));
                                ui.set_request_headers(request.headers.into());
                                ui.set_request_params(request.query_params.into());
                                ui.set_request_body(request.body.into());
                                ui.set_body_format(request.body_format.into());
                                ui.set_auth_type(request.auth_type.into());
                                ui.set_auth_value(request.auth_value.into());
                                ui.set_auth_secret(request.auth_secret.into());
                                ui.set_response_status("No response yet".into());
                                ui.set_response_body(
                                    "Send a request to inspect its response.".into(),
                                );
                                ui.set_response_time("---".into());
                                ui.set_response_state("idle".into());
                                ui.set_response_format("Text".into());
                                ui.invoke_register_active_request_tab();
                            }
                            Ok(Ok(None)) => {
                                ui.set_response_status("Request not found; refreshing list".into());
                                ui.invoke_load_requests(ui.get_expanded_collection_id());
                            }
                            Ok(Err(error)) => ui.set_response_status(
                                format!("Could not load request: {error}").into(),
                            ),
                            Err(error) => ui.set_response_status(
                                format!("Could not load request: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_delete_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |id, collection_id| {
            if id <= 0 {
                return;
            }
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let result = store.delete(id as i64);
                let list_result = store.list_for_collection(collection_id as i64);
                (result, list_result)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok((Ok(true), requests)) => {
                                if ui.get_active_request_id() == id {
                                    ui.set_active_request_id(-1);
                                    ui.set_active_request("".into());
                                    ui.set_editing_request_name(false);
                                    ui.set_active_method("GET".into());
                                    ui.set_request_url("".into());
                                    ui.set_request_headers("Accept: application/json".into());
                                    ui.set_request_header_rows(to_key_value_model(
                                        "Accept: application/json",
                                        ':',
                                    ));
                                    ui.set_request_params("".into());
                                    ui.set_request_param_rows(to_key_value_model("", '='));
                                    ui.set_request_body("".into());
                                    ui.set_body_format("JSON".into());
                                    ui.set_auth_type("None".into());
                                    ui.set_auth_value("".into());
                                    ui.set_auth_secret("".into());
                                }
                                ui.set_response_status("Request deleted".into());
                                if let Ok(requests) = requests {
                                    ui.set_saved_requests(to_request_model(
                                        requests,
                                        ui.get_request_search().as_str(),
                                    ));
                                }
                            }
                            Ok((Ok(false), requests)) => {
                                ui.set_response_status("Request was already deleted".into());
                                if let Ok(requests) = requests {
                                    ui.set_saved_requests(to_request_model(
                                        requests,
                                        ui.get_request_search().as_str(),
                                    ));
                                }
                            }
                            Ok((Err(error), _)) => {
                                ui.set_response_status(format!("Delete failed: {error}").into())
                            }
                            Err(error) => {
                                ui.set_response_status(format!("Delete failed: {error}").into())
                            }
                        }
                    }
                });
            });
        }
    });

    app.on_open_latest_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store
                    .list_for_collection(collection_id as i64)
                    .map(|requests| requests.into_iter().max_by_key(|request| request.id))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() != collection_id {
                            return;
                        }
                        match result {
                            Ok(Ok(Some(request))) => {
                                ui.invoke_select_request(request.id as i32);
                            }
                            Ok(Ok(None)) => ui.invoke_show_empty_collection(),
                            Ok(Err(error)) => {
                                ui.invoke_show_empty_collection();
                                ui.set_variables_message(
                                    format!("Could not open the latest request: {error}").into(),
                                );
                            }
                            Err(error) => {
                                ui.invoke_show_empty_collection();
                                ui.set_variables_message(
                                    format!("Could not open the latest request: {error}").into(),
                                );
                            }
                        }
                    }
                });
            });
        }
    });

    let theme_weak = app.as_weak();
    let theme_fallback = app.get_system_dark_mode();
    let accent_fallback = app.get_system_accent_color();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let system_dark = detect_system_dark_mode(theme_fallback);
            let system_accent = detect_system_accent_color(accent_fallback);
            let app_handle = theme_weak.clone();
            if slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    if ui.get_theme_mode() == "System" && ui.get_system_dark_mode() != system_dark {
                        ui.set_system_dark_mode(system_dark);
                    }
                    if ui.get_system_accent_color() != system_accent {
                        ui.set_system_accent_color(system_accent);
                    }
                }
            })
            .is_err()
            {
                break;
            }
        }
    });

    app.invoke_load_collections();
    app.run()
}
