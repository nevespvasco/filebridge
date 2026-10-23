//! `entry_thumbnail` against the engine: previews come from the local source, which is only read,
//! and land in the cache folder.

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use filebridge_core::{
    Engine, EngineOptions, EntryFilter, Profile, Registry, SendOptions, Store, TargetSpec,
};
use filebridge_host::EngineOperations;
use filebridge_ipc::Operations;
use filebridge_thumbnail::Thumbnails;
use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_fake::FakeTransport;

/// A 2×2 24-bit BMP: the simplest image to write by hand.
fn bmp() -> Vec<u8> {
    let row = [255u8, 0, 0, 0, 255, 0, 0, 0]; // two pixels, padded to 4 bytes
    let pixels: Vec<u8> = [row, row].concat();
    let mut out = b"BM".to_vec();
    out.extend((54 + pixels.len() as u32).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend(2i32.to_le_bytes());
    out.extend(2i32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0u8; 24]);
    out.extend(pixels);
    out
}

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = walk(dir).collect();
    v.sort();
    v
}

fn walk(dir: &Path) -> Box<dyn Iterator<Item = String>> {
    let entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    Box::new(entries.into_iter().flat_map(|e| {
        let p = e.path();
        let me = std::iter::once(p.display().to_string());
        if p.is_dir() {
            Box::new(me.chain(walk(&p))) as Box<dyn Iterator<Item = String>>
        } else {
            Box::new(me)
        }
    }))
}

#[tokio::test]
async fn a_source_image_gets_a_cached_preview_and_the_source_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("Fotos");
    std::fs::create_dir_all(src.join("2019")).unwrap();
    std::fs::write(src.join("2019/photo.bmp"), bmp()).unwrap();
    std::fs::write(src.join("notes.txt"), b"hello").unwrap();
    let dest = dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let cache = dir.path().join("cache/thumbnails");

    let mut registry = Registry::new();
    registry.add_transport(Arc::new(FakeTransport::default()));
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        registry,
        EngineOptions {
            inhibit_sleep: false,
            ..EngineOptions::default()
        },
    );
    let plain = EngineOperations::new(engine.clone());
    let ops = EngineOperations::new(engine)
        .with_thumbnails(Arc::new(Thumbnails::new(cache.clone(), 1 << 20)));
    let profile = ops
        .save_profile(Profile {
            id: String::new(),
            name: "dest".into(),
            destination: Destination {
                transport: FakeTransport::ID.into(),
                backend: Backend::Local,
                root: dest.to_string_lossy().into_owned(),
            },
            integration: None,
            integration_settings: serde_json::Value::Null,
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    let job = ops
        .create_job(
            vec![src.to_string_lossy().into_owned()],
            profile.id,
            SendOptions::default(),
        )
        .await
        .unwrap();
    ops.analyse(job.clone()).await.unwrap();
    let before = listing(&src);

    let page = ops
        .entries(
            job.clone(),
            EntryFilter {
                include_dirs: true,
                ..EntryFilter::default()
            },
            None,
            100,
        )
        .await
        .unwrap();
    let idx = |name: &str| page.items.iter().find(|e| e.name == name).unwrap().idx;

    let thumb = ops
        .entry_thumbnail(job.clone(), idx("photo.bmp"))
        .await
        .unwrap()
        .expect("a preview");
    assert_eq!(thumb.mime, "image/jpeg");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&thumb.base64)
        .unwrap();
    assert_eq!(&bytes[..2], [0xFF, 0xD8], "a JPEG");
    // Again: from the cache, the same bytes.
    let again = ops
        .entry_thumbnail(job.clone(), idx("photo.bmp"))
        .await
        .unwrap();
    assert_eq!(again, Some(thumb));

    for none in ["notes.txt", "2019"] {
        assert_eq!(
            ops.entry_thumbnail(job.clone(), idx(none)).await.unwrap(),
            None,
            "{none}"
        );
    }
    assert_eq!(listing(&src), before, "nothing written next to the source");
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);

    let err = ops.entry_thumbnail(job.clone(), 9999).await.unwrap_err();
    assert_eq!(err.code, "not_found");
    let err = plain.entry_thumbnail(job, 0).await.unwrap_err();
    assert_eq!(err.code, "unsupported");
}
