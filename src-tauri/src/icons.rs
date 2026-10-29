use tauri::image::Image;

pub fn application() -> Image<'static> {
    tauri::include_image!("icons/128x128.png")
}

pub fn tray(dark: bool) -> Image<'static> {
    let source=tauri::include_image!("icons/tray/32x32.png");
    let mut rgba=source.rgba().to_vec();
    let value=if dark {239} else {38};
    for pixel in rgba.chunks_exact_mut(4) {pixel[..3].fill(value);}
    Image::new_owned(rgba,source.width(),source.height())
}
