//! Desktop integration helpers: theme detection, client-side chrome, and clipboard access.

use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use slint::Color;

fn is_gnome_session(current_desktop: &str, session_desktop: &str) -> bool {
    [current_desktop, session_desktop]
        .into_iter()
        .any(|desktops| {
            desktops
                .split([':', ','])
                .any(|desktop| desktop.trim().eq_ignore_ascii_case("gnome"))
        })
}

pub(crate) fn use_gnome_custom_frame() -> bool {
    #[cfg(target_os = "linux")]
    {
        is_gnome_session(
            &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            &std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub(crate) fn copy_text_to_clipboard(
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
                copy_text_with_linux_clipboard_tools(text)
                    .map_err(|fallback_error| format!("{native_error}; {fallback_error}"))
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

pub(crate) fn detect_system_dark_mode(fallback: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            && output.status.success()
        {
            let preference = String::from_utf8_lossy(&output.stdout);
            if preference.contains("prefer-dark") {
                return true;
            }
            if preference.contains("prefer-light") {
                return false;
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

pub(crate) fn detect_system_accent_color(fallback: Color) -> Color {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "accent-color"])
            .output()
            && output.status.success()
        {
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

#[cfg(test)]
mod platform_window_tests {
    use super::is_gnome_session;

    #[test]
    fn selects_custom_frame_only_for_gnome_desktops() {
        assert!(is_gnome_session("GNOME", "gnome"));
        assert!(is_gnome_session("ubuntu:GNOME", "ubuntu"));
        assert!(!is_gnome_session("KDE", "plasma"));
        assert!(!is_gnome_session("", ""));
    }
}
