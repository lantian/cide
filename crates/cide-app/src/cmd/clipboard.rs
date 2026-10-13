//! Copy a webview-rendered drawing to the native clipboard. WebKit's async Clipboard API
//! refuses Excalidraw's PNG writes after export has consumed transient activation. The browser
//! already owns the image decoder, so it sends RGBA through the same raw frame used for drawing
//! saves; the registered clipboard plugin writes the bitmap without creating resource handles.

use cide_core::{CoreError, Result};
use cide_ipc::clipboard::ClipboardImageWrite;
use tauri_plugin_clipboard_manager::ClipboardExt;

#[tauri::command]
pub async fn clipboard_write_image(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<()> {
    let image = image_request(request.body())?;
    tokio::task::spawn_blocking(move || {
        app.clipboard()
            .write_image(&image)
            .map_err(|e| CoreError::Io(format!("could not copy the image to the clipboard: {e}")))
    })
    .await
    .map_err(|e| CoreError::Io(format!("clipboard image worker failed: {e}")))?
}

/// Validate before constructing an image: the native clipboard assumes the dimensions agree
/// with the RGBA slice, and a short slice or overflowing size must never reach it.
fn image_request(body: &tauri::ipc::InvokeBody) -> Result<tauri::image::Image<'static>> {
    let tauri::ipc::InvokeBody::Raw(raw) = body else {
        return Err(CoreError::Io(
            "clipboard_write_image takes a raw body, never JSON pixels".into(),
        ));
    };
    let (head, rgba) = cide_ipc::frame::unpack(raw)
        .map_err(|e| CoreError::Io(format!("clipboard_write_image: {e}")))?;
    let ClipboardImageWrite { width, height } = serde_json::from_str(head)
        .map_err(|e| CoreError::Io(format!("clipboard_write_image: invalid dimensions: {e}")))?;
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| CoreError::Io("the clipboard image is too large".into()))?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(CoreError::Io(format!(
            "the clipboard image says {width}×{height} but carries {} RGBA bytes, expected {expected}",
            rgba.len()
        )));
    }
    Ok(tauri::image::Image::new_owned(rgba.to_vec(), width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(head: &str, rgba: &[u8]) -> tauri::ipc::InvokeBody {
        tauri::ipc::InvokeBody::Raw(cide_ipc::frame::pack(head, rgba).expect("frame"))
    }

    #[test]
    fn clipboard_image_preserves_rgba_and_dimensions() {
        let rgba = [255, 20, 10, 128, 0, 0, 0, 0];
        let image = image_request(&request(r#"{"width":2,"height":1}"#, &rgba)).unwrap();
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 1);
        assert_eq!(image.rgba(), rgba);
    }

    #[test]
    fn clipboard_image_rejects_invalid_frames_and_dimensions() {
        for body in [
            tauri::ipc::InvokeBody::Json(serde_json::json!({"width": 1, "height": 1})),
            tauri::ipc::InvokeBody::Raw(vec![20, 0, 0, 0, b'{']),
            request("not JSON", &[]),
            request(r#"{"width":1}"#, &[]),
            request(r#"{"width":-1,"height":1}"#, &[]),
            request(r#"{"width":1,"height":1,"extra":true}"#, &[0; 4]),
            request(r#"{"width":0,"height":1}"#, &[]),
            request(r#"{"width":1,"height":0}"#, &[]),
            request(r#"{"width":1,"height":1}"#, &[0; 3]),
            request(r#"{"width":1,"height":1}"#, &[0; 5]),
            request(r#"{"width":4294967295,"height":4294967295}"#, &[]),
        ] {
            assert!(image_request(&body).is_err(), "accepted {body:?}");
        }
    }
}
