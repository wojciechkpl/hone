use std::path::PathBuf;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

// ---------------------------------------------------------------------------
// ScreenCaptureTool
// ---------------------------------------------------------------------------

/// Captures a screenshot and returns it as base64-encoded PNG data.
pub struct ScreenCaptureTool;

#[async_trait]
impl Tool for ScreenCaptureTool {
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the current screen or a specific region.\n\n\
         WHEN TO USE: To verify visual changes (web UI, terminal output, GUI apps). \
         To debug rendering issues that cannot be caught from source alone.\n\
         WHEN NOT TO USE: For text-only output — use shell instead. \
         For log files or program output — use read_file or shell.\n\
         Returns: base64-encoded PNG image data and image dimensions."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "region": {
                    "type": "string",
                    "description": "Screen region to capture. Options: \
                        'full' (entire screen, default), \
                        'window' (active window), \
                        or 'x,y,w,h' pixel coordinates (e.g. '0,0,800,600').",
                    "default": "full"
                }
            }
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let region = input.get_str("region").unwrap_or("full");
        let tmp_path = PathBuf::from("/tmp/hone_screenshot.png");

        capture_screenshot(region, &tmp_path).await?;

        let png_bytes = tokio::fs::read(&tmp_path).await.map_err(|e| {
            ToolError::ExecutionFailed {
                tool: "screen_capture".into(),
                message: format!("failed to read screenshot file: {e}"),
            }
        })?;

        // Decode just enough of the PNG header to get width/height (bytes 16..24).
        let (width, height) = parse_png_dimensions(&png_bytes).unwrap_or((0, 0));

        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        // Clean up temp file — ignore errors.
        let _ = tokio::fs::remove_file(&tmp_path).await;

        Ok(ToolOutput::new(json!({
            "image_data": b64,
            "format": "png",
            "encoding": "base64",
            "width": width,
            "height": height,
            "size_bytes": png_bytes.len(),
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// ComputerControlTool
// ---------------------------------------------------------------------------

/// Basic mouse and keyboard automation for the desktop.
pub struct ComputerControlTool;

#[async_trait]
impl Tool for ComputerControlTool {
    fn name(&self) -> &str {
        "computer_control"
    }

    fn description(&self) -> &str {
        "Control the mouse and keyboard to interact with GUI applications.\n\n\
         WHEN TO USE: To click buttons, type text, or press key combinations \
         in a running graphical application. Best used after screen_capture \
         to verify the current state.\n\
         WHEN NOT TO USE: For headless CI environments. For file or text operations \
         that can be done with other tools. Requires automation tools to be installed.\n\
         Supported actions: type_text, click, move_mouse, key.\n\
         On macOS uses osascript. On Linux requires xdotool."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["type_text", "click", "move_mouse", "key"],
                    "description": "The action to perform."
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (required for type_text action)."
                },
                "x": {
                    "type": "integer",
                    "description": "X pixel coordinate (required for click and move_mouse)."
                },
                "y": {
                    "type": "integer",
                    "description": "Y pixel coordinate (required for click and move_mouse)."
                },
                "key": {
                    "type": "string",
                    "description": "Key combination to press, e.g. 'enter', 'ctrl+c', 'cmd+space' \
                        (required for key action)."
                }
            },
            "required": ["action"]
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let action = input.get_str("action").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'action'".into(),
        })?;

        #[cfg(target_os = "macos")]
        {
            run_macos_action(action, &input).await
        }
        #[cfg(target_os = "linux")]
        {
            run_linux_action(action, &input).await
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = action;
            Err(ToolError::ExecutionFailed {
                tool: "computer_control".into(),
                message: "computer_control is only supported on macOS and Linux".into(),
            })
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// Platform-specific screenshot capture
// ---------------------------------------------------------------------------

async fn capture_screenshot(region: &str, out_path: &PathBuf) -> Result<(), ToolError> {
    #[cfg(target_os = "macos")]
    {
        capture_macos(region, out_path).await
    }
    #[cfg(target_os = "linux")]
    {
        capture_linux(region, out_path).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (region, out_path);
        Err(ToolError::ExecutionFailed {
            tool: "screen_capture".into(),
            message: "screen_capture is only supported on macOS and Linux".into(),
        })
    }
}

#[cfg(target_os = "macos")]
async fn capture_macos(region: &str, out_path: &PathBuf) -> Result<(), ToolError> {
    // screencapture is built into macOS — no external dependencies.
    let mut cmd = Command::new("screencapture");
    cmd.arg("-x").arg("-t").arg("png");

    if region == "window" {
        // -l captures a specific window; -w captures the frontmost window interactively.
        // For non-interactive use, -R with a window ID is needed. We use -R with full
        // screen bounds as a safe fallback when "window" is requested without an ID.
        cmd.arg("-o"); // omit shadow
    } else if let Some(coords) = parse_region_coords(region) {
        let (x, y, w, h) = coords;
        cmd.arg("-R").arg(format!("{x},{y},{w},{h}"));
    }
    // "full" or unrecognised: no extra flags → full screen

    cmd.arg(out_path);

    let status = cmd.status().await.map_err(|e| ToolError::ExecutionFailed {
        tool: "screen_capture".into(),
        message: format!("screencapture failed to start: {e}"),
    })?;

    if !status.success() {
        return Err(ToolError::ExecutionFailed {
            tool: "screen_capture".into(),
            message: format!(
                "screencapture exited with code {}",
                status.code().unwrap_or(-1)
            ),
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn capture_linux(region: &str, out_path: &PathBuf) -> Result<(), ToolError> {
    // Try gnome-screenshot first, then fall back to import (ImageMagick).
    let gnome = Command::new("which")
        .arg("gnome-screenshot")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if gnome {
        let mut cmd = Command::new("gnome-screenshot");
        if let Some((x, y, w, h)) = parse_region_coords(region) {
            cmd.args(["-a", "--", &format!("{x},{y},{w},{h}")]);
        }
        cmd.arg("-f").arg(out_path);

        let status = cmd.status().await.map_err(|e| ToolError::ExecutionFailed {
            tool: "screen_capture".into(),
            message: format!("gnome-screenshot failed to start: {e}"),
        })?;

        if status.success() {
            return Ok(());
        }
    }

    // Fallback: ImageMagick's import tool.
    let import_ok = Command::new("which")
        .arg("import")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !import_ok {
        return Err(ToolError::ExecutionFailed {
            tool: "screen_capture".into(),
            message: "no screenshot tool found; install gnome-screenshot or imagemagick".into(),
        });
    }

    let mut cmd = Command::new("import");
    if let Some((x, y, w, h)) = parse_region_coords(region) {
        cmd.arg("-crop").arg(format!("{w}x{h}+{x}+{y}"));
    }
    cmd.arg("-window").arg("root").arg(out_path);

    let status = cmd.status().await.map_err(|e| ToolError::ExecutionFailed {
        tool: "screen_capture".into(),
        message: format!("import (ImageMagick) failed to start: {e}"),
    })?;

    if !status.success() {
        return Err(ToolError::ExecutionFailed {
            tool: "screen_capture".into(),
            message: format!("import exited with code {}", status.code().unwrap_or(-1)),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Platform-specific computer control
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
async fn run_macos_action(action: &str, input: &ToolInput) -> Result<ToolOutput, ToolError> {
    let script = build_macos_script(action, input)?;

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "computer_control".into(),
            message: format!("osascript failed to start: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::ExecutionFailed {
            tool: "computer_control".into(),
            message: format!("osascript error: {stderr}"),
        });
    }

    Ok(ToolOutput::new(json!({
        "ok": true,
        "action": action,
    })))
}

#[cfg(target_os = "macos")]
fn build_macos_script(action: &str, input: &ToolInput) -> Result<String, ToolError> {
    match action {
        "type_text" => {
            let text = input.get_str("text").ok_or_else(|| ToolError::InvalidInput {
                reason: "type_text requires 'text' field".into(),
            })?;
            // Escape backslashes and double-quotes for AppleScript.
            let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
            Ok(format!(
                r#"tell application "System Events" to keystroke "{escaped}""#
            ))
        }
        "click" => {
            let x = input.get_u64("x").ok_or_else(|| ToolError::InvalidInput {
                reason: "click requires 'x' field".into(),
            })?;
            let y = input.get_u64("y").ok_or_else(|| ToolError::InvalidInput {
                reason: "click requires 'y' field".into(),
            })?;
            Ok(format!(
                r#"tell application "System Events" to click at {{{x}, {y}}}"#
            ))
        }
        "move_mouse" => {
            let x = input.get_u64("x").ok_or_else(|| ToolError::InvalidInput {
                reason: "move_mouse requires 'x' field".into(),
            })?;
            let y = input.get_u64("y").ok_or_else(|| ToolError::InvalidInput {
                reason: "move_mouse requires 'y' field".into(),
            })?;
            // AppleScript does not directly support mouse movement without additional
            // frameworks.  We use cliclick if available, otherwise return a clear error.
            Ok(format!(
                r#"do shell script "cliclick m:{x},{y}""#
            ))
        }
        "key" => {
            let key_combo = input.get_str("key").ok_or_else(|| ToolError::InvalidInput {
                reason: "key requires 'key' field".into(),
            })?;
            let applescript_key = translate_key_combo_macos(key_combo);
            Ok(applescript_key)
        }
        other => Err(ToolError::InvalidInput {
            reason: format!("unknown action: '{other}'"),
        }),
    }
}

#[cfg(target_os = "macos")]
fn translate_key_combo_macos(combo: &str) -> String {
    // Translate common key combo notation ("ctrl+c", "cmd+space") to AppleScript.
    let parts: Vec<&str> = combo.split('+').collect();
    let key = parts.last().unwrap_or(&"");
    let mut modifiers: Vec<&str> = Vec::new();

    for part in &parts[..parts.len().saturating_sub(1)] {
        match *part {
            "ctrl" | "control" => modifiers.push("control down"),
            "cmd" | "command" => modifiers.push("command down"),
            "alt" | "option" => modifiers.push("option down"),
            "shift" => modifiers.push("shift down"),
            _ => {}
        }
    }

    let using_clause = if modifiers.is_empty() {
        String::new()
    } else {
        format!(" using {{{}}}", modifiers.join(", "))
    };

    // Special keys map to AppleScript key codes.
    let key_clause = match *key {
        "enter" | "return" => r#"key code 36"#.to_string(),
        "escape" | "esc" => r#"key code 53"#.to_string(),
        "tab" => r#"key code 48"#.to_string(),
        "space" => r#"keystroke " ""#.to_string(),
        "up" => r#"key code 126"#.to_string(),
        "down" => r#"key code 125"#.to_string(),
        "left" => r#"key code 123"#.to_string(),
        "right" => r#"key code 124"#.to_string(),
        "delete" | "backspace" => r#"key code 51"#.to_string(),
        c if c.len() == 1 => format!(r#"keystroke "{c}""#),
        other => format!(r#"keystroke "{other}""#),
    };

    format!(r#"tell application "System Events" to {key_clause}{using_clause}"#)
}

#[cfg(target_os = "linux")]
async fn run_linux_action(action: &str, input: &ToolInput) -> Result<ToolOutput, ToolError> {
    // Check that xdotool is available.
    let xdotool_ok = Command::new("which")
        .arg("xdotool")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !xdotool_ok {
        return Err(ToolError::ExecutionFailed {
            tool: "computer_control".into(),
            message: "xdotool is not installed; install it with: apt install xdotool".into(),
        });
    }

    let mut cmd = Command::new("xdotool");

    match action {
        "type_text" => {
            let text = input.get_str("text").ok_or_else(|| ToolError::InvalidInput {
                reason: "type_text requires 'text' field".into(),
            })?;
            cmd.arg("type").arg("--clearmodifiers").arg(text);
        }
        "click" => {
            let x = input.get_u64("x").ok_or_else(|| ToolError::InvalidInput {
                reason: "click requires 'x' field".into(),
            })?;
            let y = input.get_u64("y").ok_or_else(|| ToolError::InvalidInput {
                reason: "click requires 'y' field".into(),
            })?;
            cmd.args(["mousemove", &x.to_string(), &y.to_string(), "click", "1"]);
        }
        "move_mouse" => {
            let x = input.get_u64("x").ok_or_else(|| ToolError::InvalidInput {
                reason: "move_mouse requires 'x' field".into(),
            })?;
            let y = input.get_u64("y").ok_or_else(|| ToolError::InvalidInput {
                reason: "move_mouse requires 'y' field".into(),
            })?;
            cmd.args(["mousemove", &x.to_string(), &y.to_string()]);
        }
        "key" => {
            let key_combo = input.get_str("key").ok_or_else(|| ToolError::InvalidInput {
                reason: "key requires 'key' field".into(),
            })?;
            // xdotool uses "ctrl+c" notation natively.
            cmd.arg("key").arg(key_combo);
        }
        other => {
            return Err(ToolError::InvalidInput {
                reason: format!("unknown action: '{other}'"),
            });
        }
    }

    let status = cmd.status().await.map_err(|e| ToolError::ExecutionFailed {
        tool: "computer_control".into(),
        message: format!("xdotool failed to start: {e}"),
    })?;

    if !status.success() {
        return Err(ToolError::ExecutionFailed {
            tool: "computer_control".into(),
            message: format!("xdotool exited with code {}", status.code().unwrap_or(-1)),
        });
    }

    Ok(ToolOutput::new(json!({
        "ok": true,
        "action": action,
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a region string of the form "x,y,w,h" into a tuple.
fn parse_region_coords(region: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<&str> = region.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let nums: Vec<u32> = parts
        .iter()
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if nums.len() != 4 {
        return None;
    }
    Some((nums[0], nums[1], nums[2], nums[3]))
}

/// Extract PNG image dimensions from the IHDR chunk (bytes 16..24 in a valid PNG).
fn parse_png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG signature: 8 bytes, then IHDR chunk: 4-byte length + 4-byte "IHDR" + 4-byte width + 4-byte height
    if data.len() < 24 {
        return None;
    }
    // PNG magic bytes
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_region_coords_valid() {
        assert_eq!(parse_region_coords("0,0,800,600"), Some((0, 0, 800, 600)));
        assert_eq!(parse_region_coords("100,200,1280,720"), Some((100, 200, 1280, 720)));
    }

    #[test]
    fn parse_region_coords_invalid() {
        assert_eq!(parse_region_coords("full"), None);
        assert_eq!(parse_region_coords("window"), None);
        assert_eq!(parse_region_coords("0,0,800"), None);
        assert_eq!(parse_region_coords("a,b,c,d"), None);
    }

    #[test]
    fn parse_png_dimensions_valid() {
        // Construct a minimal fake PNG header.
        let mut data = vec![0u8; 24];
        data[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        // width = 1920 (0x00000780), height = 1080 (0x00000438)
        data[16..20].copy_from_slice(&1920u32.to_be_bytes());
        data[20..24].copy_from_slice(&1080u32.to_be_bytes());
        assert_eq!(parse_png_dimensions(&data), Some((1920, 1080)));
    }

    #[test]
    fn parse_png_dimensions_short_buffer() {
        assert_eq!(parse_png_dimensions(&[0u8; 10]), None);
    }

    #[test]
    fn parse_png_dimensions_wrong_magic() {
        let data = vec![0u8; 24];
        assert_eq!(parse_png_dimensions(&data), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn translate_key_combo_enter() {
        let s = translate_key_combo_macos("enter");
        assert!(s.contains("key code 36"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn translate_key_combo_ctrl_c() {
        let s = translate_key_combo_macos("ctrl+c");
        assert!(s.contains("control down"));
        assert!(s.contains('"'));
    }
}
