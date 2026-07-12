fn main() {
    tauri_build::build();
    std::fs::create_dir_all(
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("icons"),
    )
    .ok();
    let icons_dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("icons");
    for &size in &[32, 128, 256] {
        let path = icons_dir.join(format!("{}x{}.png", size, size));
        if !path.exists() {
            let img = image::ImageBuffer::from_fn(size, size, |x, y| {
                let center = size as f32 / 2.0;
                let dx = x as f32 - center;
                let dy = y as f32 - center;
                let dist = (dx * dx + dy * dy).sqrt();
                let radius = center * 0.65;
                if dist <= radius {
                    let t = dist / radius;
                    image::Rgba([
                        (255.0 - t * 40.0) as u8,
                        (179.0 - t * 30.0) as u8,
                        (153.0 - t * 20.0) as u8,
                        255,
                    ])
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            });
            img.save(&path).ok();
        }
    }
}
