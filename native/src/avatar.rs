//! GitHub avatars, fetched once per URL and cached for the app's lifetime.
//! Shows a neutral circle until the image arrives.
use crate::backend;
use crate::theme::{self, hsla};
use gpui::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

enum Slot {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

static CACHE: Mutex<Option<HashMap<String, Slot>>> = Mutex::new(None);

fn format_of(bytes: &[u8]) -> Option<ImageFormat> {
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => ImageFormat::Png,
        [0xFF, 0xD8, ..] => ImageFormat::Jpeg,
        [b'G', b'I', b'F', ..] => ImageFormat::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ImageFormat::Webp,
        _ => return None,
    })
}

/// The avatar at `size` px; starts the download on first use and refreshes the
/// window when it lands.
pub fn view(url: &str, size: f32, cx: &mut App) -> AnyElement {
    let placeholder = || div().size(px(size)).flex_shrink_0().rounded_full().bg(hsla(theme::BUTTON)).into_any_element();
    if url.is_empty() {
        return placeholder();
    }
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    match cache.get(url) {
        Some(Slot::Ready(image)) => img(ImageSource::Image(image.clone())).size(px(size)).flex_shrink_0().rounded_full().into_any_element(),
        Some(_) => placeholder(),
        None => {
            cache.insert(url.to_string(), Slot::Loading);
            // Ask GitHub for a size that's sharp on Retina displays.
            let sep = if url.contains('?') { '&' } else { '?' };
            let fetch_url = format!("{url}{sep}s={}", (size * 2.) as u32);
            let key = url.to_string();
            let fut = backend::task(async move {
                let bytes = reqwest::get(&fetch_url).await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
                Ok::<_, String>(bytes.to_vec())
            });
            cx.spawn(async move |cx| {
                let slot = match fut.await {
                    Ok(bytes) => match format_of(&bytes) {
                        Some(format) => Slot::Ready(Arc::new(Image::from_bytes(format, bytes))),
                        None => Slot::Failed,
                    },
                    Err(_) => Slot::Failed,
                };
                CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, slot);
                cx.update(|cx| cx.refresh_windows());
            })
            .detach();
            placeholder()
        }
    }
}
