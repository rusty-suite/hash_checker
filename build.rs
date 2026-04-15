// build.rs — Script de compilation
//
// Exécuté automatiquement par cargo avant de compiler le projet.
// Lit assets/hash-checker-logo.png, génère un .ico multi-tailles,
// puis l'intègre dans le .exe (Windows seulement).
// Sur les autres OS : ne fait rien.

fn main() {
    // CARGO_MANIFEST_DIR = répertoire absolu du projet (où se trouve Cargo.toml)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    let png_path = format!("{manifest_dir}/assets/hash-checker-logo.png");
    let ico_path = format!("{manifest_dir}/assets/hash-checker-logo.ico");

    // Recompiler si le PNG source change
    println!("cargo:rerun-if-changed={png_path}");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // Lire le PNG avec un chemin absolu
        let png_bytes = std::fs::read(&png_path)
            .expect("Impossible de lire assets/hash-checker-logo.png");
        let img = image::load_from_memory(&png_bytes)
            .expect("Format PNG invalide");

        // Générer le .ico avec les tailles standard Windows
        let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
        for size in [16u32, 32, 48, 256] {
            let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
            let rgba = resized.to_rgba8();
            let ico_img = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
            icon_dir.add_entry(
                ico::IconDirEntry::encode(&ico_img)
                    .unwrap_or_else(|_| panic!("Impossible d'encoder la taille {size}px")),
            );
        }

        // Écrire le .ico généré avec un chemin absolu
        let mut ico_file = std::fs::File::create(&ico_path)
            .expect("Impossible de créer hash-checker-logo.ico");
        icon_dir.write(&mut ico_file).expect("Impossible d'écrire le .ico");

        // Intégrer dans le .exe avec un chemin absolu
        let mut res = winres::WindowsResource::new();
        res.set_icon(&ico_path);
        res.compile().expect("Impossible d'intégrer l'icône dans le .exe");
    }
}
